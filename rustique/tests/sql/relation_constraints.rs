//! Relation-level `scope`, `latest_of_many` / `oldest_of_many`.

use chrono::NaiveDateTime;
use rustique::{Dialect, Model, Query, QueryBuilder, Rel, Value};

use crate::{assert_sql, binds, error};

#[rustique::model(table = "authors")]
#[derive(Debug, Clone, Default)]
struct Author {
    id: i64,
    #[rustique(has_many(Article, scope = visible))]
    visible_articles: Rel<Vec<Article>>,
    #[rustique(has_one(Article, latest_of_many))]
    latest_article: Rel<Option<Article>>,
    #[rustique(has_one(Article, oldest_of_many = "published_at", scope = |q| q.where_("articles.published", "=", true)))]
    first_published: Rel<Option<Article>>,
    #[rustique(has_one(Article, of_many(published_at = "max", id = "min")))]
    newest_first_id: Rel<Option<Article>>,
}

#[rustique::model(table = "articles", soft_deletes)]
#[derive(Debug, Clone, Default)]
struct Article {
    id: i64,
    author_id: i64,
    published: bool,
    featured: bool,
    published_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

fn visible(q: Query<Article>) -> Query<Article> {
    q.where_("articles.published", "=", true)
        .or_where("articles.featured", "=", true)
}

fn author() -> Author {
    Author {
        id: 7,
        ..Default::default()
    }
}

#[test]
fn scope_constrains_the_relation_query() {
    let query = author().visible_articles().where_("id", ">", 1);
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `articles` WHERE (`articles`.`published` = ? OR `articles`.`featured` = ?) \
         AND `articles`.`author_id` = ? AND `id` > ? AND `articles`.`deleted_at` IS NULL",
    );
    assert_eq!(
        binds(&query.into_builder()),
        [
            Value::Bool(true),
            Value::Bool(true),
            Value::I64(7),
            Value::I64(1)
        ]
    );

    // `where_has` and counts see the scope too.
    let has = Author::query().where_has("visible_articles", |q| q);
    assert_eq!(
        has.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `authors` WHERE EXISTS (SELECT * FROM `articles` WHERE \
         (`articles`.`published` = ? OR `articles`.`featured` = ?) AND `articles`.`deleted_at` IS NULL \
         AND `articles`.`author_id` = `authors`.`id`)",
    );
}

#[test]
fn latest_of_many_keeps_one_row_per_parent() {
    let query = author().latest_article();
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `articles` WHERE `articles`.`id` IN (SELECT `orm_ranked`.`orm_key` FROM \
         (SELECT `articles`.`id` AS `orm_key`, ROW_NUMBER() OVER (PARTITION BY `articles`.`author_id` \
         ORDER BY `articles`.`id` DESC) AS `orm_rank` FROM `articles` WHERE `articles`.`deleted_at` IS NULL) \
         `orm_ranked` WHERE `orm_ranked`.`orm_rank` = ?) AND `articles`.`author_id` = ? \
         AND `articles`.`deleted_at` IS NULL",
    );
    assert_eq!(
        query.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM "ARTICLES" WHERE "ARTICLES"."ID" IN (SELECT "ORM_RANKED"."ORM_KEY" FROM (SELECT "ARTICLES"."ID" AS "ORM_KEY", ROW_NUMBER() OVER (PARTITION BY "ARTICLES"."AUTHOR_ID" ORDER BY "ARTICLES"."ID" DESC) AS "ORM_RANK" FROM "ARTICLES" WHERE "ARTICLES"."DELETED_AT" IS NULL) "ORM_RANKED" WHERE "ORM_RANKED"."ORM_RANK" = :1) AND "ARTICLES"."AUTHOR_ID" = :2 AND "ARTICLES"."DELETED_AT" IS NULL"#,
    );
}

#[test]
fn oldest_of_many_by_column_ranks_inside_the_scope() {
    let sql = author()
        .first_published()
        .to_sql(Dialect::MySql)
        .unwrap()
        .sql;
    assert_eq!(
        sql,
        "SELECT * FROM `articles` WHERE `articles`.`id` IN \
         (SELECT `orm_ranked`.`orm_key` FROM (SELECT `articles`.`id` AS `orm_key`, ROW_NUMBER() OVER \
         (PARTITION BY `articles`.`author_id` ORDER BY `articles`.`published_at` ASC, `articles`.`id` ASC) \
         AS `orm_rank` FROM `articles` WHERE `articles`.`published` = ? AND `articles`.`deleted_at` IS NULL \
         AND `articles`.`published_at` IS NOT NULL) `orm_ranked` WHERE `orm_ranked`.`orm_rank` = ?) \
         AND `articles`.`published` = ? AND `articles`.`author_id` = ? AND `articles`.`deleted_at` IS NULL",
    );
    let count = Author::query().with_count("first_published");
    assert!(
        count
            .to_sql(Dialect::Oracle)
            .unwrap()
            .sql
            .contains(r#"ROW_NUMBER() OVER (PARTITION BY "ARTICLES"."AUTHOR_ID" ORDER BY "ARTICLES"."PUBLISHED_AT" ASC"#)
    );
}

#[test]
fn of_many_ranks_by_several_columns() {
    let query = author().newest_first_id().into_builder();
    assert_sql(
        &query,
        "SELECT * FROM `articles` WHERE `articles`.`id` IN (SELECT `orm_ranked`.`orm_key` FROM \
         (SELECT `articles`.`id` AS `orm_key`, ROW_NUMBER() OVER (PARTITION BY `articles`.`author_id` \
         ORDER BY `articles`.`published_at` DESC, `articles`.`id` ASC) AS `orm_rank` FROM `articles` \
         WHERE `articles`.`deleted_at` IS NULL AND `articles`.`published_at` IS NOT NULL) `orm_ranked` \
         WHERE `orm_ranked`.`orm_rank` = ?) AND `articles`.`author_id` = ? AND `articles`.`deleted_at` IS NULL",
        r#"SELECT * FROM "ARTICLES" WHERE "ARTICLES"."ID" IN (SELECT "ORM_RANKED"."ORM_KEY" FROM (SELECT "ARTICLES"."ID" AS "ORM_KEY", ROW_NUMBER() OVER (PARTITION BY "ARTICLES"."AUTHOR_ID" ORDER BY "ARTICLES"."PUBLISHED_AT" DESC, "ARTICLES"."ID" ASC) AS "ORM_RANK" FROM "ARTICLES" WHERE "ARTICLES"."DELETED_AT" IS NULL AND "ARTICLES"."PUBLISHED_AT" IS NOT NULL) "ORM_RANKED" WHERE "ORM_RANKED"."ORM_RANK" = :1) AND "ARTICLES"."AUTHOR_ID" = :2 AND "ARTICLES"."DELETED_AT" IS NULL"#,
    );
}

#[test]
fn limit_per_numbers_rows_per_group() {
    let query = QueryBuilder::table("articles")
        .where_in("articles.author_id", [1, 2])
        .order_by_desc("articles.published_at")
        .offset(1)
        .limit(2)
        .limit_per(["articles.author_id"]);
    assert_sql(
        &query,
        "SELECT * FROM (SELECT `articles`.*, ROW_NUMBER() OVER (PARTITION BY `articles`.`author_id` \
         ORDER BY `articles`.`published_at` DESC) AS `orm_row` FROM `articles` WHERE `articles`.`author_id` IN (?, ?)) \
         `orm_limited` WHERE `orm_limited`.`orm_row` > ? AND `orm_limited`.`orm_row` <= ? ORDER BY `orm_limited`.`orm_row` ASC",
        r#"SELECT * FROM (SELECT "ARTICLES".*, ROW_NUMBER() OVER (PARTITION BY "ARTICLES"."AUTHOR_ID" ORDER BY "ARTICLES"."PUBLISHED_AT" DESC) AS "ORM_ROW" FROM "ARTICLES" WHERE "ARTICLES"."AUTHOR_ID" IN (:1, :2)) "ORM_LIMITED" WHERE "ORM_LIMITED"."ORM_ROW" > :3 AND "ORM_LIMITED"."ORM_ROW" <= :4 ORDER BY "ORM_LIMITED"."ORM_ROW" ASC"#,
    );
    assert_eq!(
        binds(&query),
        [Value::I64(1), Value::I64(2), Value::U64(1), Value::U64(3)]
    );

    // Without a limit it changes nothing; a limit needs an order.
    let plain = QueryBuilder::table("articles").limit_per(["author_id"]);
    assert_sql(
        &plain,
        "SELECT * FROM `articles`",
        r#"SELECT * FROM "ARTICLES""#,
    );
    let unordered = plain.limit(2);
    assert!(error(&unordered, Dialect::MySql).contains("needs an order"));
}
