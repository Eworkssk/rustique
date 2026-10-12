use rustique::{Dialect, QueryBuilder, Value};

#[test]
fn insert_one_row() {
    let users = QueryBuilder::table("users");
    let row = || vec![vec![Value::from("Ann"), Value::from("a@x.io")]];

    let mysql = users
        .to_insert_sql(Dialect::MySql, &["name", "email"], row())
        .unwrap();
    assert_eq!(
        mysql.sql,
        "INSERT INTO `users` (`name`, `email`) VALUES (?, ?)"
    );
    assert_eq!(mysql.binds, row()[0]);

    let oracle = users
        .to_insert_sql(Dialect::Oracle, &["name", "email"], row())
        .unwrap();
    assert_eq!(
        oracle.sql,
        r#"INSERT INTO "USERS" ("NAME", "EMAIL") VALUES (:1, :2)"#
    );
}

#[test]
fn insert_many_rows() {
    let users = QueryBuilder::table("users");
    let rows = || vec![vec![Value::from("Ann")], vec![Value::from("Bo")]];

    let mysql = users
        .to_insert_sql(Dialect::MySql, &["name"], rows())
        .unwrap();
    assert_eq!(mysql.sql, "INSERT INTO `users` (`name`) VALUES (?), (?)");

    let oracle = users
        .to_insert_sql(Dialect::Oracle, &["name"], rows())
        .unwrap();
    assert_eq!(mysql.batch, None);

    // Oracle: one single-row statement, run with array binding over both rows.
    assert_eq!(oracle.sql, r#"INSERT INTO "USERS" ("NAME") VALUES (:1)"#);
    assert_eq!(oracle.binds, [Value::from("Ann"), Value::from("Bo")]);
    assert_eq!(oracle.batch, Some(2));
}

#[test]
fn insert_rejects_bad_rows() {
    let users = QueryBuilder::table("users");
    let short = users.to_insert_sql(Dialect::MySql, &["a", "b"], vec![vec![Value::from(1)]]);
    assert!(
        short
            .unwrap_err()
            .to_string()
            .contains("1 values for 2 columns")
    );
    assert!(users.to_insert_sql(Dialect::MySql, &["a"], vec![]).is_err());
}

#[test]
fn update_sets_then_filters() {
    let query = QueryBuilder::table("users").where_("id", "=", 5);
    let values = || vec![("name", Value::from("Bo")), ("deleted_at", Value::Null)];

    let mysql = query.to_update_sql(Dialect::MySql, values()).unwrap();
    assert_eq!(
        mysql.sql,
        "UPDATE `users` SET `name` = ?, `deleted_at` = ? WHERE `id` = ?"
    );
    assert_eq!(mysql.binds, [Value::from("Bo"), Value::Null, Value::I64(5)]);

    let oracle = query.to_update_sql(Dialect::Oracle, values()).unwrap();
    assert_eq!(
        oracle.sql,
        r#"UPDATE "USERS" SET "NAME" = :1, "DELETED_AT" = :2 WHERE "ID" = :3"#
    );
}

#[test]
fn delete_with_filter() {
    let query = QueryBuilder::table("users").where_in("id", [1, 2]);
    assert_eq!(
        query.to_delete_sql(Dialect::MySql).unwrap().sql,
        "DELETE FROM `users` WHERE `id` IN (?, ?)"
    );
    assert_eq!(
        query.to_delete_sql(Dialect::Oracle).unwrap().sql,
        r#"DELETE FROM "USERS" WHERE "ID" IN (:1, :2)"#
    );
}

#[test]
fn update_and_delete_reject_limits_and_joins() {
    let limited = QueryBuilder::table("users").limit(1);
    assert!(
        limited
            .to_update_sql(Dialect::MySql, vec![("a", Value::from(1))])
            .is_err()
    );

    let joined = QueryBuilder::table("users").join("posts", "posts.user_id", "=", "users.id");
    assert!(joined.to_delete_sql(Dialect::Oracle).is_err());
}
