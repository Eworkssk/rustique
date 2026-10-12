use chrono::NaiveDateTime;
use rustique::{Dialect, Model, QueryBuilder, Rel, Value};

use crate::{assert_sql, binds};

#[rustique::model(table = "countries")]
#[derive(Debug, Clone, Default)]
struct Country {
    id: i64,
    #[rustique(has_many_through(Post, through = User))]
    posts: Rel<Vec<Post>>,
}

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    country_id: i64,
    #[rustique(belongs_to(Country))]
    country: Rel<Option<Country>>,
    #[rustique(has_one(Profile))]
    profile: Rel<Option<Profile>>,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
    #[rustique(belongs_to_many(Role))]
    roles: Rel<Vec<Role>>,
    #[rustique(belongs_to_many(Role, pivot_timestamps))]
    stamped_roles: Rel<Vec<Role>>,
    #[rustique(count)]
    posts_count: Rel<u64>,
}

#[rustique::model(table = "profiles")]
#[derive(Debug, Clone, Default)]
struct Profile {
    id: i64,
    user_id: i64,
}

#[rustique::model(table = "posts", soft_deletes)]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    user_id: i64,
    deleted_at: Option<NaiveDateTime>,
    #[rustique(belongs_to(User, foreign_key = "user_id"))]
    author: Rel<Option<Box<User>>>,
}

#[rustique::model(table = "roles")]
#[derive(Debug, Clone, Default)]
struct Role {
    id: i64,
}

fn user() -> User {
    User {
        id: 7,
        country_id: 3,
        ..Default::default()
    }
}

#[test]
fn has_one_and_has_many() {
    assert_sql(
        &user().profile().into_builder(),
        "SELECT * FROM `profiles` WHERE `profiles`.`user_id` = ?",
        r#"SELECT * FROM "PROFILES" WHERE "PROFILES"."USER_ID" = :1"#,
    );
    let posts = user().posts().where_("id", ">", 1).into_builder();
    assert_sql(
        &posts,
        "SELECT * FROM `posts` WHERE `posts`.`user_id` = ? AND `id` > ? AND `posts`.`deleted_at` IS NULL",
        r#"SELECT * FROM "POSTS" WHERE "POSTS"."USER_ID" = :1 AND "ID" > :2 AND "POSTS"."DELETED_AT" IS NULL"#,
    );
    assert_eq!(binds(&posts), [Value::I64(7), Value::I64(1)]);
}

#[test]
fn belongs_to() {
    assert_sql(
        &user().country().into_builder(),
        "SELECT * FROM `countries` WHERE `countries`.`id` = ?",
        r#"SELECT * FROM "COUNTRIES" WHERE "COUNTRIES"."ID" = :1"#,
    );
    let post = Post {
        user_id: 7,
        ..Default::default()
    };
    assert_eq!(binds(&post.author().into_builder()), [Value::I64(7)]);
}

#[test]
fn belongs_to_many_joins_the_pivot_table() {
    assert_sql(
        &user().roles().into_builder(),
        "SELECT `roles`.* FROM `roles` INNER JOIN `role_user` ON `role_user`.`role_id` = `roles`.`id` \
         WHERE `role_user`.`user_id` = ?",
        r#"SELECT "ROLES".* FROM "ROLES" INNER JOIN "ROLE_USER" ON "ROLE_USER"."ROLE_ID" = "ROLES"."ID" WHERE "ROLE_USER"."USER_ID" = :1"#,
    );
}

#[test]
fn pivot_timestamps_default_to_created_at_and_updated_at() {
    let through = |name| User::relation(name).unwrap().through.unwrap().timestamps;
    assert_eq!(through("roles"), None);
    assert_eq!(through("stamped_roles"), Some(["created_at", "updated_at"]));
}

#[test]
fn has_many_through_joins_the_middle_table() {
    let country = Country {
        id: 3,
        ..Default::default()
    };
    assert_sql(
        &country.posts().into_builder(),
        "SELECT `posts`.* FROM `posts` INNER JOIN `users` ON `users`.`id` = `posts`.`user_id` \
         WHERE `users`.`country_id` = ? AND `posts`.`deleted_at` IS NULL",
        r#"SELECT "POSTS".* FROM "POSTS" INNER JOIN "USERS" ON "USERS"."ID" = "POSTS"."USER_ID" WHERE "USERS"."COUNTRY_ID" = :1 AND "POSTS"."DELETED_AT" IS NULL"#,
    );
}

#[test]
fn where_has_correlates_the_related_query() {
    let query = User::query().where_has("posts", |q| q.where_("id", "=", 1).or_where("id", "=", 2));
    assert_sql(
        &query.into_builder(),
        "SELECT * FROM `users` WHERE EXISTS (SELECT * FROM `posts` WHERE `posts`.`deleted_at` IS NULL \
         AND `posts`.`user_id` = `users`.`id` AND (`id` = ? OR `id` = ?))",
        r#"SELECT * FROM "USERS" WHERE EXISTS (SELECT * FROM "POSTS" WHERE "POSTS"."DELETED_AT" IS NULL AND "POSTS"."USER_ID" = "USERS"."ID" AND ("ID" = :1 OR "ID" = :2))"#,
    );
    assert_sql(
        &User::query()
            .where_doesnt_have("roles", |q| q)
            .into_builder(),
        "SELECT * FROM `users` WHERE NOT EXISTS (SELECT `roles`.* FROM `roles` INNER JOIN `role_user` \
         ON `role_user`.`role_id` = `roles`.`id` WHERE `role_user`.`user_id` = `users`.`id`)",
        r#"SELECT * FROM "USERS" WHERE NOT EXISTS (SELECT "ROLES".* FROM "ROLES" INNER JOIN "ROLE_USER" ON "ROLE_USER"."ROLE_ID" = "ROLES"."ID" WHERE "ROLE_USER"."USER_ID" = "USERS"."ID")"#,
    );
}

#[test]
fn with_count_adds_a_subquery() {
    assert_sql(
        &User::query().with_count("posts").into_builder(),
        "SELECT `users`.*, (SELECT COUNT(*) FROM `posts` WHERE `posts`.`deleted_at` IS NULL \
         AND `posts`.`user_id` = `users`.`id`) AS `posts_count` FROM `users`",
        r#"SELECT "USERS".*, (SELECT COUNT(*) FROM "POSTS" WHERE "POSTS"."DELETED_AT" IS NULL AND "POSTS"."USER_ID" = "USERS"."ID") AS "POSTS_COUNT" FROM "USERS""#,
    );
}

#[test]
fn unknown_relation_is_reported() {
    let error = User::query()
        .where_has("nope", |q| q)
        .to_sql(Dialect::MySql)
        .unwrap_err();
    assert!(error.to_string().contains("no relation `nope`"), "{error}");
    assert!(User::relation("posts").is_some());
}

#[test]
fn rel_field_access() {
    let user = user();
    assert!(user.posts.loaded().is_err());
    assert_eq!(user.posts_count, Rel::NotLoaded);
    assert_eq!(Rel::Loaded(3).loaded().unwrap(), &3);
}

#[test]
fn row_values_in() {
    let query = QueryBuilder::table("lines").where_rows_in(
        ["order_no", "line_no"],
        [vec![1.into(), 2.into()], vec![1.into(), 3.into()]],
    );
    assert_sql(
        &query,
        "SELECT * FROM `lines` WHERE (`order_no`, `line_no`) IN ((?, ?), (?, ?))",
        r#"SELECT * FROM "LINES" WHERE ("ORDER_NO", "LINE_NO") IN ((:1, :2), (:3, :4))"#,
    );

    let many = QueryBuilder::table("t").where_rows_in(
        ["a", "b"],
        (0..1001).map(|i| vec![i.to_string().into(), i.into()]),
    );
    let oracle = many.to_sql(Dialect::Oracle).unwrap().sql;
    assert!(
        oracle.contains(r#") OR ("A", "B") IN ("#),
        "Oracle splits at 1000 rows"
    );

    let empty = QueryBuilder::table("t").where_rows_in(["a", "b"], []);
    assert_eq!(
        empty.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `t` WHERE 0 = 1"
    );

    let bad = QueryBuilder::table("t").where_rows_in(["a", "b"], [vec![Value::from(1)]]);
    assert!(bad.to_sql(Dialect::MySql).is_err());
}
