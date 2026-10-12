use rustique::{Dialect, Model, Query, QueryBuilder, Value};

use crate::{assert_sql, error};

#[test]
fn upsert_one_row() {
    let users = QueryBuilder::table("users");
    let row = || vec![vec![Value::from("a@x.io"), Value::from("Ann")]];
    let columns = ["email", "name"];

    let mysql = users
        .to_upsert_sql(Dialect::MySql, &columns, row(), &["email"], &["name"])
        .unwrap();
    assert_eq!(
        mysql.sql,
        "INSERT INTO `users` (`email`, `name`) VALUES (?, ?) AS `new` \
         ON DUPLICATE KEY UPDATE `name` = `new`.`name`"
    );
    assert_eq!(mysql.binds, row()[0]);

    let oracle = users
        .to_upsert_sql(Dialect::Oracle, &columns, row(), &["email"], &["name"])
        .unwrap();
    assert_eq!(
        oracle.sql,
        r#"MERGE INTO "USERS" "T" USING (SELECT :1 AS "EMAIL", :2 AS "NAME" FROM DUAL) "S" ON ("T"."EMAIL" = "S"."EMAIL") WHEN MATCHED THEN UPDATE SET "T"."NAME" = "S"."NAME" WHEN NOT MATCHED THEN INSERT ("EMAIL", "NAME") VALUES ("S"."EMAIL", "S"."NAME")"#
    );
    assert_eq!(oracle.binds, row()[0]);
    assert_eq!(oracle.batch, None);
}

#[test]
fn upsert_many_rows_without_update() {
    let users = QueryBuilder::table("users");
    let rows = || {
        vec![
            vec![Value::from(1), Value::from(2)],
            vec![Value::from(3), Value::from(4)],
        ]
    };
    // Updating a matched column is dropped: Oracle rejects it and it changes nothing.
    let (unique, update) = (&["a", "b"][..], &["a"][..]);

    let mysql = users
        .to_upsert_sql(Dialect::MySql, &["a", "b"], rows(), unique, update)
        .unwrap();
    assert_eq!(
        mysql.sql,
        "INSERT INTO `users` (`a`, `b`) VALUES (?, ?), (?, ?) AS `new` \
         ON DUPLICATE KEY UPDATE `users`.`a` = `users`.`a`"
    );

    let oracle = users
        .to_upsert_sql(Dialect::Oracle, &["a", "b"], rows(), unique, update)
        .unwrap();
    assert_eq!(
        oracle.sql,
        r#"MERGE INTO "USERS" "T" USING (SELECT :1 AS "A", :2 AS "B" FROM DUAL) "S" ON ("T"."A" = "S"."A" AND "T"."B" = "S"."B") WHEN NOT MATCHED THEN INSERT ("A", "B") VALUES ("S"."A", "S"."B")"#
    );
    assert_eq!(oracle.batch, Some(2));
    assert_eq!(oracle.binds, rows().concat());
}

#[test]
fn upsert_rejects_unknown_columns() {
    let users = QueryBuilder::table("users");
    let row = || vec![vec![Value::from(1)]];
    for (unique, update) in [(&[][..], &[][..]), (&["b"], &[]), (&["a"], &["b"])] {
        let error = users
            .to_upsert_sql(Dialect::MySql, &["a"], row(), unique, update)
            .unwrap_err();
        assert!(error.to_string().contains("unique_by"), "{error}");
    }
}

#[rustique::model(table = "users", soft_deletes)]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    kind: String,
    deleted_at: Option<chrono::NaiveDateTime>,
}

#[rustique::scopes]
impl User {
    /// Admins only.
    fn scope_admins(q: Query<Self>) -> Query<Self> {
        q.where_(User::KIND, "=", "admin")
    }

    fn scope_of_kind(q: Query<Self>, kind: impl Into<Value>) -> Query<Self> {
        q.where_(User::KIND, "=", kind)
    }

    fn scope_type(q: Query<Self>) -> Query<Self> {
        q.where_not_null(User::KIND)
    }

    fn scope_admin_or(q: Query<Self>, id: i64) -> Query<Self> {
        q.where_(User::KIND, "=", "admin").or_where("id", "=", id)
    }

    fn not_a_scope() -> i32 {
        1
    }
}

#[test]
fn scopes_become_query_methods() {
    let query = User::query().admins().or_where("id", "=", 1).of_kind("x");
    assert_sql(
        &query.into_builder(),
        "SELECT * FROM `users` WHERE (`kind` = ? OR `id` = ?) AND `kind` = ? \
         AND `users`.`deleted_at` IS NULL",
        r#"SELECT * FROM "USERS" WHERE ("KIND" = :1 OR "ID" = :2) AND "KIND" = :3 AND "USERS"."DELETED_AT" IS NULL"#,
    );
    assert_eq!(User::not_a_scope(), 1);
}

#[test]
fn scope_or_stays_inside_the_scope() {
    let sql = |query: Query<User>| query.into_builder().to_sql(Dialect::MySql).unwrap().sql;
    assert_eq!(
        sql(User::query().where_("id", ">", 10).admin_or(5)),
        "SELECT * FROM `users` WHERE `id` > ? AND (`kind` = ? OR `id` = ?) \
         AND `users`.`deleted_at` IS NULL"
    );
    assert_eq!(
        sql(User::admin_or(5).where_("id", ">", 10)),
        "SELECT * FROM `users` WHERE (`kind` = ? OR `id` = ?) AND `id` > ? \
         AND `users`.`deleted_at` IS NULL"
    );
}

#[test]
fn scopes_start_queries_and_allow_keywords() {
    assert_eq!(
        User::admins()
            .r#type()
            .into_builder()
            .to_sql(Dialect::MySql)
            .unwrap()
            .sql,
        "SELECT * FROM `users` WHERE `kind` = ? AND `kind` IS NOT NULL \
         AND `users`.`deleted_at` IS NULL"
    );
    let binds = User::of_kind("x").to_sql(Dialect::MySql).unwrap().binds;
    assert_eq!(binds, [Value::from("x")]);
}

#[test]
fn shared_lock_is_mysql_only() {
    let query = QueryBuilder::table("jobs")
        .where_("id", "=", 1)
        .shared_lock();
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `jobs` WHERE `id` = ? FOR SHARE"
    );
    assert_eq!(
        query
            .clone()
            .skip_locked()
            .to_sql(Dialect::MySql)
            .unwrap()
            .sql,
        "SELECT * FROM `jobs` WHERE `id` = ? FOR SHARE SKIP LOCKED"
    );
    // lock_for_update switches back to an exclusive lock.
    assert_eq!(
        query
            .clone()
            .lock_for_update()
            .to_sql(Dialect::MySql)
            .unwrap()
            .sql,
        "SELECT * FROM `jobs` WHERE `id` = ? FOR UPDATE"
    );
    assert!(error(&query, Dialect::Oracle).contains("shared locks"));
}
