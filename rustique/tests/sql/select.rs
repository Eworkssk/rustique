use std::time::Duration;

use rustique::{Dialect, QueryBuilder, Value};

use crate::{assert_sql, binds, error};

#[test]
fn selects_all_columns_by_default() {
    assert_sql(
        &QueryBuilder::table("users"),
        "SELECT * FROM `users`",
        r#"SELECT * FROM "USERS""#,
    );
}

#[test]
fn quotes_qualified_and_aliased_columns() {
    let query = QueryBuilder::table("users").select(["id", "users.email as mail", "posts.*"]);
    assert_sql(
        &query,
        "SELECT `id`, `users`.`email` AS `mail`, `posts`.* FROM `users`",
        r#"SELECT "ID", "USERS"."EMAIL" AS "MAIL", "POSTS".* FROM "USERS""#,
    );
}

#[test]
fn oracle_table_alias_has_no_as() {
    let query = QueryBuilder::table("users as u")
        .distinct()
        .select(["u.id"]);
    assert_sql(
        &query,
        "SELECT DISTINCT `u`.`id` FROM `users` AS `u`",
        r#"SELECT DISTINCT "U"."ID" FROM "USERS" "U""#,
    );
}

#[test]
fn raw_select_binds_get_dialect_placeholders() {
    let query =
        QueryBuilder::table("users").select_raw("COALESCE(name, ?) AS name", [Value::from("n/a")]);
    assert_sql(
        &query,
        "SELECT COALESCE(name, ?) AS name FROM `users`",
        r#"SELECT COALESCE(name, :1) AS name FROM "USERS""#,
    );
    assert_eq!(binds(&query), [Value::from("n/a")]);
}

#[test]
fn groups_filters_groups_and_orders() {
    let query = QueryBuilder::table("users")
        .select(["role"])
        .select_raw("COUNT(*) AS total", [])
        .group_by(["role"])
        .having("role", "<>", "guest")
        .order_by("role")
        .order_by_desc("total");
    assert_sql(
        &query,
        "SELECT `role`, COUNT(*) AS total FROM `users` GROUP BY `role` HAVING `role` <> ? ORDER BY `role` ASC, `total` DESC",
        r#"SELECT "ROLE", COUNT(*) AS total FROM "USERS" GROUP BY "ROLE" HAVING "ROLE" <> :1 ORDER BY "ROLE" ASC, "TOTAL" DESC"#,
    );
}

#[test]
fn limit_and_offset() {
    assert_sql(
        &QueryBuilder::table("users").limit(10).offset(20),
        "SELECT * FROM `users` LIMIT 10 OFFSET 20",
        r#"SELECT * FROM "USERS" OFFSET 20 ROWS FETCH NEXT 10 ROWS ONLY"#,
    );
}

#[test]
fn offset_without_limit() {
    assert_sql(
        &QueryBuilder::table("users").offset(5),
        "SELECT * FROM `users` LIMIT 18446744073709551615 OFFSET 5",
        r#"SELECT * FROM "USERS" OFFSET 5 ROWS"#,
    );
}

#[test]
fn for_page_is_one_based() {
    assert_sql(
        &QueryBuilder::table("users").for_page(3, 15),
        "SELECT * FROM `users` LIMIT 15 OFFSET 30",
        r#"SELECT * FROM "USERS" OFFSET 30 ROWS FETCH NEXT 15 ROWS ONLY"#,
    );
}

#[test]
fn lock_for_update_skip_locked() {
    let query = QueryBuilder::table("jobs")
        .where_("id", "=", 1)
        .lock_for_update()
        .skip_locked();
    assert_sql(
        &query,
        "SELECT * FROM `jobs` WHERE `id` = ? FOR UPDATE SKIP LOCKED",
        r#"SELECT * FROM "JOBS" WHERE "ID" = :1 FOR UPDATE SKIP LOCKED"#,
    );
}

#[test]
fn lock_for_update_wait_and_no_wait() {
    let no_wait = QueryBuilder::table("jobs")
        .where_("id", "=", 1)
        .lock_for_update_no_wait();
    assert_sql(
        &no_wait,
        "SELECT * FROM `jobs` WHERE `id` = ? FOR UPDATE NOWAIT",
        r#"SELECT * FROM "JOBS" WHERE "ID" = :1 FOR UPDATE NOWAIT"#,
    );

    // Rounded up to whole seconds; MySQL has no per-statement wait.
    let wait = QueryBuilder::table("jobs").lock_for_update_wait(Duration::from_millis(2500));
    assert_eq!(
        wait.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM "JOBS" FOR UPDATE WAIT 3"#
    );
    assert!(error(&wait, Dialect::MySql).contains("does not support FOR UPDATE WAIT"));

    // A limited window keeps the wait on the outer, locking query.
    let window = QueryBuilder::table("jobs")
        .order_by("id")
        .limit(1)
        .lock_for_update_wait(Duration::from_secs(5));
    assert!(
        (window.to_sql(Dialect::Oracle).unwrap().sql)
            .ends_with(r#"ORDER BY "ID" ASC FOR UPDATE WAIT 5"#)
    );

    let both = QueryBuilder::table("jobs")
        .lock_for_update_no_wait()
        .skip_locked();
    for dialect in [Dialect::MySql, Dialect::Oracle] {
        assert!(error(&both, dialect).contains("SKIP LOCKED cannot be combined"));
    }
}

#[test]
fn oracle_locks_a_limited_window_through_rowids() {
    let query = QueryBuilder::table("jobs as j")
        .select_raw("j.*, ? AS tag", ["t".into()])
        .where_("j.state", "=", "new")
        .order_by_raw("j.priority * ?", [2.into()])
        .order_by("j.id")
        .offset(2)
        .limit(1)
        .lock_for_update();
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT j.*, ? AS tag FROM `jobs` AS `j` WHERE `j`.`state` = ? \
         ORDER BY j.priority * ?, `j`.`id` ASC LIMIT 1 OFFSET 2 FOR UPDATE"
    );
    let oracle = query.to_sql(Dialect::Oracle).unwrap();
    assert_eq!(
        oracle.sql,
        r#"SELECT j.*, :1 AS tag FROM "JOBS" "J" WHERE ROWID IN (SELECT ROWID FROM "JOBS" "J" WHERE "J"."STATE" = :2 ORDER BY j.priority * :3, "J"."ID" ASC OFFSET 2 ROWS FETCH NEXT 1 ROWS ONLY) ORDER BY j.priority * :4, "J"."ID" ASC FOR UPDATE"#
    );
    assert_eq!(oracle.binds, ["t".into(), "new".into(), 2.into(), 2.into()]);
}

#[test]
fn oracle_rejects_lock_with_limit_it_cannot_rewrite() {
    let skip = QueryBuilder::table("jobs")
        .limit(1)
        .lock_for_update()
        .skip_locked();
    assert!(error(&skip, Dialect::Oracle).contains("ORA-02014"));
    let joined = QueryBuilder::table("jobs")
        .join("queues", "queues.id", "=", "jobs.queue_id")
        .limit(1)
        .lock_for_update();
    assert!(error(&joined, Dialect::Oracle).contains("ORA-02014"));
    let grouped = QueryBuilder::table("jobs")
        .group_by(["queue_id"])
        .limit(1)
        .lock_for_update();
    assert!(error(&grouped, Dialect::Oracle).contains("ORA-02014"));
}

#[test]
fn identifiers_cannot_break_out_of_quotes() {
    let query = QueryBuilder::table("users").order_by("a`b");
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `users` ORDER BY `a``b` ASC"
    );

    let query = QueryBuilder::table("users").order_by(r#"x"; DROP TABLE users; --"#);
    assert!(error(&query, Dialect::Oracle).contains("cannot contain"));
}

#[test]
fn missing_table_is_an_error() {
    assert!(error(&QueryBuilder::default(), Dialect::MySql).contains("no table"));
}
