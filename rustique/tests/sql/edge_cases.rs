//! SQL for edge cases found in a bug hunt.

use chrono::NaiveDateTime;
use rustique::{Dialect, Model, QueryBuilder};

use crate::{assert_sql, binds};

#[rustique::model(table = "users", soft_deletes)]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    name: String,
    deleted_at: Option<NaiveDateTime>,
}

#[test]
fn raw_or_stays_inside_its_condition() {
    // Without parentheses the soft-delete filter would apply to `b` only.
    let query = User::query().where_raw("a = ? OR b = ?", [1.into(), 2.into()]);
    assert_sql(
        &query.clone().into_builder(),
        "SELECT * FROM `users` WHERE (a = ? OR b = ?) AND `users`.`deleted_at` IS NULL",
        r#"SELECT * FROM "USERS" WHERE (a = :1 OR b = :2) AND "USERS"."DELETED_AT" IS NULL"#,
    );
    // Alone, a raw condition is written as it is.
    assert_sql(
        &QueryBuilder::table("t").where_raw("a = 1 OR b = 2", []),
        "SELECT * FROM `t` WHERE a = 1 OR b = 2",
        r#"SELECT * FROM "T" WHERE a = 1 OR b = 2"#,
    );
    let having = QueryBuilder::table("t")
        .group_by(["a"])
        .having("a", ">", 1)
        .having_raw("COUNT(*) > 1 OR SUM(b) > 2", []);
    assert_sql(
        &having,
        "SELECT * FROM `t` GROUP BY `a` HAVING `a` > ? AND (COUNT(*) > 1 OR SUM(b) > 2)",
        r#"SELECT * FROM "T" GROUP BY "A" HAVING "A" > :1 AND (COUNT(*) > 1 OR SUM(b) > 2)"#,
    );
}

#[test]
fn raw_sql_ending_in_a_line_comment_does_not_swallow_the_rest() {
    let query = QueryBuilder::table("t")
        .where_raw("a = ? -- the first", [1.into()])
        .where_("b", "=", 2)
        .order_by_raw("a -- by a", [])
        .limit(1);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE (a = ? -- the first\n) AND `b` = ? ORDER BY a -- by a\n LIMIT 1",
        "SELECT * FROM \"T\" WHERE (a = :1 -- the first\n) AND \"B\" = :2 ORDER BY a -- by a\n FETCH NEXT 1 ROWS ONLY",
    );
    assert_eq!(binds(&query), [1.into(), 2.into()]);

    // Alone in its list, as the only ON condition here, too: the soft-delete
    // filter after it must not become part of the comment.
    let joined = User::query()
        .join_with("posts", |j| {
            j.where_raw("posts.user_id = users.id -- author", [])
        })
        .into_builder();
    assert_sql(
        &joined,
        "SELECT `users`.* FROM `users` INNER JOIN `posts` ON posts.user_id = users.id -- author\n WHERE `users`.`deleted_at` IS NULL",
        "SELECT \"USERS\".* FROM \"USERS\" INNER JOIN \"POSTS\" ON posts.user_id = users.id -- author\n WHERE \"USERS\".\"DELETED_AT\" IS NULL",
    );
}

#[test]
fn ordered_model_unions_are_inline_views_on_oracle() {
    let query = User::query()
        .where_("id", "<", 5)
        .union_all(User::query().where_("id", ">", 10))
        .order_by("name")
        .into_builder();
    assert_eq!(
        query.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM ((SELECT * FROM "USERS" WHERE "ID" < :1 AND "USERS"."DELETED_AT" IS NULL) UNION ALL (SELECT * FROM "USERS" WHERE "ID" > :2 AND "USERS"."DELETED_AT" IS NULL)) ORDER BY "NAME" ASC"#
    );
    // Without ordering nothing needs the view.
    let plain = QueryBuilder::table("a").union(QueryBuilder::table("b"));
    assert_eq!(
        plain.to_sql(Dialect::Oracle).unwrap().sql,
        r#"(SELECT * FROM "A") UNION (SELECT * FROM "B")"#
    );
}

#[test]
fn locked_model_lookups_compile_on_oracle() {
    let query = User::query()
        .where_key(7)
        .lock_for_update()
        .limit(1)
        .into_builder();
    assert_eq!(
        query.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM "USERS" WHERE ROWID IN (SELECT ROWID FROM "USERS" WHERE "USERS"."ID" = :1 AND "USERS"."DELETED_AT" IS NULL FETCH NEXT 1 ROWS ONLY) FOR UPDATE"#
    );
}

#[test]
fn oracle_alternative_quotes_hide_question_marks() {
    let query =
        QueryBuilder::table("t").where_raw("a = q'[it's ?]' AND b = ? AND c = Q'{?}'", [1.into()]);
    assert_eq!(
        query.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM "T" WHERE a = q'[it's ?]' AND b = :1 AND c = Q'{?}'"#
    );
    // MySQL has no such literal: `q` is an identifier there.
    let mysql = QueryBuilder::table("t").where_raw("q = ?", [1.into()]);
    assert_eq!(
        mysql.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `t` WHERE q = ?"
    );
}
