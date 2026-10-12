use chrono::NaiveDate;
use rustique::{Dialect, QueryBuilder, Value};

use crate::{assert_sql, binds, error};

#[test]
fn when_applies_only_on_true() {
    let query = |admin| QueryBuilder::table("users").when(admin, |q| q.where_("admin", "=", true));
    assert_sql(
        &query(true),
        "SELECT * FROM `users` WHERE `admin` = ?",
        r#"SELECT * FROM "USERS" WHERE "ADMIN" = :1"#,
    );
    assert_sql(
        &query(false),
        "SELECT * FROM `users`",
        r#"SELECT * FROM "USERS""#,
    );
}

#[test]
fn latest_and_oldest_order() {
    let query = QueryBuilder::table("posts")
        .latest("created_at")
        .oldest("id");
    assert_sql(
        &query,
        "SELECT * FROM `posts` ORDER BY `created_at` DESC, `id` ASC",
        r#"SELECT * FROM "POSTS" ORDER BY "CREATED_AT" DESC, "ID" ASC"#,
    );
}

#[test]
fn where_date_compiles_to_a_range() {
    let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
    let midnight = |d: NaiveDate| Value::DateTime(d.and_hms_opt(0, 0, 0).unwrap());
    let next = day.succ_opt().unwrap();

    let query = QueryBuilder::table("posts").where_date("created_at", "=", day);
    assert_sql(
        &query,
        "SELECT * FROM `posts` WHERE (`created_at` >= ? AND `created_at` < ?)",
        r#"SELECT * FROM "POSTS" WHERE ("CREATED_AT" >= :1 AND "CREATED_AT" < :2)"#,
    );
    assert_eq!(binds(&query), [midnight(day), midnight(next)]);

    let not = QueryBuilder::table("posts").where_date("created_at", "<>", day);
    assert_sql(
        &not,
        "SELECT * FROM `posts` WHERE (`created_at` < ? OR `created_at` >= ?)",
        r#"SELECT * FROM "POSTS" WHERE ("CREATED_AT" < :1 OR "CREATED_AT" >= :2)"#,
    );
    let after = QueryBuilder::table("posts").where_date("created_at", ">", day);
    assert_eq!(binds(&after), [midnight(next)]);
    let until = QueryBuilder::table("posts").where_date("created_at", "<=", day);
    assert_sql(
        &until,
        "SELECT * FROM `posts` WHERE `created_at` < ?",
        r#"SELECT * FROM "POSTS" WHERE "CREATED_AT" < :1"#,
    );
    assert_eq!(binds(&until), [midnight(next)]);

    let like = QueryBuilder::table("posts").where_date("created_at", "like", day);
    assert!(error(&like, Dialect::MySql).contains("cannot compare dates"));
}

#[test]
fn where_json_reads_a_path() {
    let query = QueryBuilder::table("users")
        .where_json("settings->theme", "=", "dark")
        .or_where_json("users.settings->sizes->0", ">", 10)
        .where_json("settings->beta-flag", "=", true);
    assert_sql(
        &query,
        "SELECT * FROM `users` WHERE JSON_UNQUOTE(JSON_EXTRACT(`settings`, '$.\"theme\"')) = ? \
         OR JSON_UNQUOTE(JSON_EXTRACT(`users`.`settings`, '$.\"sizes\"[0]')) > ? \
         AND JSON_UNQUOTE(JSON_EXTRACT(`settings`, '$.\"beta-flag\"')) = ?",
        r#"SELECT * FROM "USERS" WHERE JSON_VALUE("SETTINGS", '$."theme"') = :1 OR JSON_VALUE("USERS"."SETTINGS", '$."sizes"[0]' RETURNING NUMBER) > :2 AND JSON_VALUE("SETTINGS", '$."beta-flag"') = :3"#,
    );
    assert_eq!(binds(&query), ["dark".into(), 10.into(), "true".into()]);
}

#[test]
fn where_json_rejects_unsafe_paths() {
    for path in ["settings", "settings->a'b", "settings->", "settings->a b"] {
        let query = QueryBuilder::table("users").where_json(path, "=", 1);
        error(&query, Dialect::MySql);
    }
    let null = QueryBuilder::table("users").where_json("settings->a", "=", Value::Null);
    assert!(error(&null, Dialect::Oracle).contains("NULL"));
}

#[test]
fn union_wraps_parts_and_orders_the_whole() {
    let admins = QueryBuilder::table("admins")
        .select(["id", "name"])
        .where_("active", "=", true);
    let query = QueryBuilder::table("users")
        .select(["id", "name"])
        .where_("age", ">", 18)
        .union(admins)
        .union_all(QueryBuilder::table("guests").select(["id", "name"]))
        .order_by("name")
        .limit(10);
    assert_sql(
        &query,
        "(SELECT `id`, `name` FROM `users` WHERE `age` > ?) \
         UNION (SELECT `id`, `name` FROM `admins` WHERE `active` = ?) \
         UNION ALL (SELECT `id`, `name` FROM `guests`) ORDER BY `name` ASC LIMIT 10",
        // Oracle orders the union as an inline view: `SELECT *` parts cannot be ordered by name.
        r#"SELECT * FROM ((SELECT "ID", "NAME" FROM "USERS" WHERE "AGE" > :1) UNION (SELECT "ID", "NAME" FROM "ADMINS" WHERE "ACTIVE" = :2) UNION ALL (SELECT "ID", "NAME" FROM "GUESTS")) ORDER BY "NAME" ASC FETCH NEXT 10 ROWS ONLY"#,
    );
    assert_eq!(binds(&query), [18.into(), true.into()]);

    let locked = QueryBuilder::table("a")
        .union(QueryBuilder::table("b"))
        .lock_for_update();
    assert!(error(&locked, Dialect::MySql).contains("union"));
}
