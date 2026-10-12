use rustique::{Dialect, QueryBuilder, Value};

use crate::{assert_sql, binds, error};

#[test]
fn and_or_conditions() {
    let query = QueryBuilder::table("users")
        .where_("age", ">=", 18)
        .or_where("role", "=", "admin");
    assert_sql(
        &query,
        "SELECT * FROM `users` WHERE `age` >= ? OR `role` = ?",
        r#"SELECT * FROM "USERS" WHERE "AGE" >= :1 OR "ROLE" = :2"#,
    );
    assert_eq!(binds(&query), [Value::I64(18), Value::from("admin")]);
}

#[test]
fn operators_are_normalised() {
    let query = QueryBuilder::table("t")
        .where_("a", "!=", 1)
        .where_("b", "LIKE", "x%")
        .where_("c", " not like ", "y");
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE `a` <> ? AND `b` LIKE ? AND `c` NOT LIKE ?",
        r#"SELECT * FROM "T" WHERE "A" <> :1 AND "B" LIKE :2 AND "C" NOT LIKE :3"#,
    );
}

#[test]
fn null_comparisons_become_is_null() {
    let query = QueryBuilder::table("t")
        .where_("deleted_at", "=", Value::Null)
        .where_("x", "is not", None::<i64>);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE `deleted_at` IS NULL AND `x` IS NOT NULL",
        r#"SELECT * FROM "T" WHERE "DELETED_AT" IS NULL AND "X" IS NOT NULL"#,
    );
    assert!(binds(&query).is_empty());
}

#[test]
fn unknown_operators_are_rejected() {
    let query = QueryBuilder::table("t").where_("a", "= 1; DROP TABLE t; --", 1);
    assert!(error(&query, Dialect::MySql).contains("unknown operator"));

    let query = QueryBuilder::table("t").where_("a", "<", Value::Null);
    assert!(error(&query, Dialect::Oracle).contains("cannot compare with NULL"));
}

#[test]
fn in_and_not_in() {
    let query = QueryBuilder::table("t")
        .where_in("id", [1, 2, 3])
        .where_not_in("role", ["x"]);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE `id` IN (?, ?, ?) AND `role` NOT IN (?)",
        r#"SELECT * FROM "T" WHERE "ID" IN (:1, :2, :3) AND "ROLE" NOT IN (:4)"#,
    );
}

#[test]
fn empty_in_lists_are_constant_conditions() {
    let query = QueryBuilder::table("t")
        .where_in("id", Vec::<i64>::new())
        .or_where_not_in("id", Vec::<i64>::new());
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE 0 = 1 OR 1 = 1",
        r#"SELECT * FROM "T" WHERE 0 = 1 OR 1 = 1"#,
    );
}

#[test]
fn oracle_splits_in_lists_over_1000_items() {
    let ids = || (1..=1001).map(|i| i.to_string());
    let query = QueryBuilder::table("t").where_in("id", ids());
    let first: Vec<String> = (1..=1000).map(|i| format!(":{i}")).collect();
    let oracle = format!(
        r#"SELECT * FROM "T" WHERE ("ID" IN ({}) OR "ID" IN (:1001))"#,
        first.join(", ")
    );
    assert_eq!(query.to_sql(Dialect::Oracle).unwrap().sql, oracle);

    let mysql = query.to_sql(Dialect::MySql).unwrap().sql;
    assert_eq!(mysql.matches('?').count(), 1001);
    assert!(!mysql.contains(" OR "));

    let negated = QueryBuilder::table("t")
        .where_not_in("id", ids())
        .to_sql(Dialect::Oracle)
        .unwrap()
        .sql;
    assert!(negated.ends_with(r#") AND "ID" NOT IN (:1001))"#));
}

#[test]
fn null_and_between() {
    let query = QueryBuilder::table("t")
        .where_null("a")
        .or_where_not_null("b")
        .where_between("age", 18, 65)
        .where_not_between("score", 1.5, 2.5);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE `a` IS NULL OR `b` IS NOT NULL AND `age` BETWEEN ? AND ? AND `score` NOT BETWEEN ? AND ?",
        r#"SELECT * FROM "T" WHERE "A" IS NULL OR "B" IS NOT NULL AND "AGE" BETWEEN :1 AND :2 AND "SCORE" NOT BETWEEN :3 AND :4"#,
    );
}

#[test]
fn grouped_conditions() {
    let query = QueryBuilder::table("users")
        .where_("active", "=", true)
        .where_group(|q| {
            q.where_("role", "=", "admin")
                .or_where("role", "=", "owner")
        });
    assert_sql(
        &query,
        "SELECT * FROM `users` WHERE `active` = ? AND (`role` = ? OR `role` = ?)",
        r#"SELECT * FROM "USERS" WHERE "ACTIVE" = :1 AND ("ROLE" = :2 OR "ROLE" = :3)"#,
    );
}

#[test]
fn errors_inside_groups_propagate() {
    let query = QueryBuilder::table("t").where_group(|q| q.where_("a", "~", 1));
    assert!(error(&query, Dialect::MySql).contains("unknown operator `~`"));
}

#[test]
fn column_comparison() {
    assert_sql(
        &QueryBuilder::table("t").where_column("updated_at", ">", "created_at"),
        "SELECT * FROM `t` WHERE `updated_at` > `created_at`",
        r#"SELECT * FROM "T" WHERE "UPDATED_AT" > "CREATED_AT""#,
    );
}

#[test]
fn subqueries_share_placeholder_numbering() {
    let query = QueryBuilder::table("users")
        .where_in_sub(
            "id",
            QueryBuilder::table("posts")
                .select(["user_id"])
                .where_("published", "=", true),
        )
        .where_exists(
            QueryBuilder::table("bans")
                .select_raw("1", [])
                .where_column("bans.user_id", "=", "users.id"),
        )
        .where_("active", "=", true);
    assert_sql(
        &query,
        "SELECT * FROM `users` WHERE `id` IN (SELECT `user_id` FROM `posts` WHERE `published` = ?) \
         AND EXISTS (SELECT 1 FROM `bans` WHERE `bans`.`user_id` = `users`.`id`) AND `active` = ?",
        r#"SELECT * FROM "USERS" WHERE "ID" IN (SELECT "USER_ID" FROM "POSTS" WHERE "PUBLISHED" = :1) AND EXISTS (SELECT 1 FROM "BANS" WHERE "BANS"."USER_ID" = "USERS"."ID") AND "ACTIVE" = :2"#,
    );
}

#[test]
fn raw_conditions_skip_question_marks_in_quotes() {
    let query =
        QueryBuilder::table("t").where_raw("LOWER(email) = ? AND note <> '?'", ["a@b.c".into()]);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE LOWER(email) = ? AND note <> '?'",
        r#"SELECT * FROM "T" WHERE LOWER(email) = :1 AND note <> '?'"#,
    );
}

#[test]
fn raw_conditions_skip_question_marks_in_comments_and_escapes() {
    let raw = "a = ? -- why?\nAND b = ? /* or? */ AND c = 5--1";
    let query = QueryBuilder::table("t").where_raw(raw, [Value::from(1), Value::from(2)]);
    assert_sql(
        &query,
        "SELECT * FROM `t` WHERE a = ? -- why?\nAND b = ? /* or? */ AND c = 5--1",
        // On Oracle `--1` is a comment, so a line break ends it.
        "SELECT * FROM \"T\" WHERE a = :1 -- why?\nAND b = :2 /* or? */ AND c = 5--1\n",
    );

    // MySQL: `#` comments and backslash escapes; Oracle has neither.
    let query =
        QueryBuilder::table("t").where_raw(r"a = 'it\'s ?' AND b = ? # c = ?", [Value::from(1)]);
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `t` WHERE a = 'it\\'s ?' AND b = ? # c = ?\n"
    );
    let query = QueryBuilder::table("t").where_raw(r"a = 'C:\' AND b = ?", [Value::from(1)]);
    assert_eq!(
        query.to_sql(Dialect::Oracle).unwrap().sql,
        r#"SELECT * FROM "T" WHERE a = 'C:\' AND b = :1"#
    );
    // `/*! ... */` runs on MySQL, so its `?` is a placeholder.
    let query = QueryBuilder::table("t").where_raw("a = 1 /*! AND b = ? */", [Value::from(1)]);
    assert!(query.to_sql(Dialect::MySql).is_ok());
}

#[test]
fn raw_bind_count_must_match() {
    let query = QueryBuilder::table("t").where_raw("a = ? AND b = ?", [Value::from(1)]);
    assert!(error(&query, Dialect::MySql).contains("more `?` placeholders"));

    let query = QueryBuilder::table("t").where_raw("a = 1", [Value::from(1)]);
    assert!(error(&query, Dialect::Oracle).contains("fewer `?` placeholders"));
}

#[test]
fn too_many_bindings_fail_to_compile() {
    let query = QueryBuilder::table("t").where_in("code", (0..70_000).map(|i| i.to_string()));
    for dialect in [Dialect::MySql, Dialect::Oracle] {
        assert!(
            error(&query, dialect).contains("70000 bindings"),
            "{dialect}"
        );
    }
    let fits = QueryBuilder::table("t").where_in("code", (0..65_535).map(|i| i.to_string()));
    assert!(fits.to_sql(Dialect::MySql).is_ok());
}

#[test]
fn long_integer_in_lists_are_written_as_literals() {
    let short = QueryBuilder::table("t").where_in("id", 0..1000);
    assert_eq!(binds(&short).len(), 1000, "up to 1000 items stay bound");

    let query = QueryBuilder::table("t")
        .where_("a", "=", 1)
        .where_in("id", 0..70_000_u64)
        .where_("b", "=", 2);
    let mysql = query.to_sql(Dialect::MySql).unwrap();
    assert!(
        mysql
            .sql
            .starts_with("SELECT * FROM `t` WHERE `a` = ? AND `id` IN (0, 1, 2, ")
    );
    assert!(mysql.sql.ends_with(", 69999) AND `b` = ?"));
    assert_eq!(mysql.binds, [Value::I64(1), Value::I64(2)]);

    // Oracle allows 65,535 list items even as literals: one JSON array bind.
    let oracle = query.to_sql(Dialect::Oracle).unwrap();
    assert_eq!(
        oracle.sql,
        r#"SELECT * FROM "T" WHERE "A" = :1 AND "ID" IN (SELECT c0 FROM JSON_TABLE(:2, '$[*]' COLUMNS (c0 NUMBER PATH '$[0]'))) AND "B" = :3"#
    );
    let Value::String(json) = &oracle.binds[1] else {
        panic!("JSON text")
    };
    assert!(json.starts_with("[[0],[1],[2],") && json.ends_with(",[69999]]"));

    // A list with anything but integers is bound as a whole.
    let mixed = QueryBuilder::table("t")
        .where_in("id", (0..1500).map(Value::from).chain([Value::from("x")]));
    assert_eq!(binds(&mixed).len(), 1501);

    let rows = QueryBuilder::table("t")
        .where_rows_in(["a", "b"], (0..1001).map(|i| vec![i.into(), (-i).into()]));
    let sql = rows.to_sql(Dialect::MySql).unwrap();
    assert!(sql.sql.ends_with("(999, -999), (1000, -1000))"));
    assert!(sql.binds.is_empty());
    let oracle = rows.to_sql(Dialect::Oracle).unwrap();
    assert_eq!(
        oracle.sql,
        r#"SELECT * FROM "T" WHERE ("A", "B") IN (SELECT c0, c1 FROM JSON_TABLE(:1, '$[*]' COLUMNS (c0 NUMBER PATH '$[0]', c1 NUMBER PATH '$[1]')))"#
    );
}
