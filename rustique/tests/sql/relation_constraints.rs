//! Relation-level `scope`, `latest_of_many` / `oldest_of_many`.

use chrono::NaiveDateTime;
use rustique::{Dialect, Model, Query, Rel, Value};

use crate::binds;

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
