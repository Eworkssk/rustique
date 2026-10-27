use chrono::NaiveDateTime;
use rustique::{Dialect, Model, Value};

use crate::assert_sql;

#[rustique::model(table = "users", timestamps, soft_deletes)]
#[derive(Debug, Clone, Default, PartialEq)]
struct User {
    id: i64,
    name: String,
    created_at: Option<NaiveDateTime>,
    updated_at: Option<NaiveDateTime>,
    deleted_at: Option<NaiveDateTime>,
}

#[rustique::model(table = "memberships")]
#[derive(Debug, Clone, Default)]
struct Membership {
    #[rustique(primary_key)]
    team_id: i64,
    #[rustique(primary_key)]
    r#type: String,
}

#[rustique::model(table = "items")]
#[derive(Debug, Clone, Default)]
struct Item {
    #[rustique(primary_key, sequence = "items_seq")]
    item_no: i64,
}

#[test]
fn macro_generates_metadata() {
    assert_eq!(User::NAME, "name");
    assert_eq!(User::TABLE, "users");
    assert_eq!(
        User::COLUMNS,
        ["id", "name", "created_at", "updated_at", "deleted_at"]
    );
    assert_eq!(User::PRIMARY_KEY, ["id"]);
    const { assert!(User::INCREMENTING && User::TIMESTAMPS && User::SOFT_DELETES) };

    assert_eq!(Membership::TYPE, "type");
    assert_eq!(Membership::PRIMARY_KEY, ["team_id", "type"]);
    const { assert!(!Membership::INCREMENTING) };

    assert_eq!(Item::PRIMARY_KEY, ["item_no"]);
    assert_eq!(Item::SEQUENCE, Some("items_seq"));
}

#[test]
fn soft_deleted_rows_are_hidden() {
    assert_sql(
        &User::query().where_(User::NAME, "=", "Ann").into_builder(),
        "SELECT * FROM `users` WHERE `name` = ? AND `users`.`deleted_at` IS NULL",
        r#"SELECT * FROM "USERS" WHERE "NAME" = :1 AND "USERS"."DELETED_AT" IS NULL"#,
    );
    assert_sql(
        &User::query().only_trashed().into_builder(),
        "SELECT * FROM `users` WHERE `users`.`deleted_at` IS NOT NULL",
        r#"SELECT * FROM "USERS" WHERE "USERS"."DELETED_AT" IS NOT NULL"#,
    );
    assert_sql(
        &User::query().with_trashed().into_builder(),
        "SELECT * FROM `users`",
        r#"SELECT * FROM "USERS""#,
    );
}

#[test]
fn limit_per_numbers_models_inside_the_soft_delete_filter() {
    assert_sql(
        &User::query()
            .order_by("id")
            .limit(2)
            .limit_per(["name"])
            .into_builder(),
        "SELECT * FROM (SELECT `users`.*, ROW_NUMBER() OVER (PARTITION BY `name` ORDER BY `id` ASC) AS `orm_row` \
         FROM `users` WHERE `users`.`deleted_at` IS NULL) `orm_limited` WHERE `orm_limited`.`orm_row` <= ? \
         ORDER BY `orm_limited`.`orm_row` ASC",
        r#"SELECT * FROM (SELECT "USERS".*, ROW_NUMBER() OVER (PARTITION BY "NAME" ORDER BY "ID" ASC) AS "ORM_ROW" FROM "USERS" WHERE "USERS"."DELETED_AT" IS NULL) "ORM_LIMITED" WHERE "ORM_LIMITED"."ORM_ROW" <= :1 ORDER BY "ORM_LIMITED"."ORM_ROW" ASC"#,
    );
}

#[test]
fn soft_delete_filter_wraps_or_conditions() {
    let query = User::query().where_("id", "=", 1).or_where("id", "=", 2);
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `users` WHERE (`id` = ? OR `id` = ?) AND `users`.`deleted_at` IS NULL"
    );
}

#[test]
fn where_key_covers_every_key_column() {
    assert_sql(
        &Membership::query()
            .where_key((7, "admin".to_owned()))
            .into_builder(),
        "SELECT * FROM `memberships` WHERE `memberships`.`team_id` = ? AND `memberships`.`type` = ?",
        r#"SELECT * FROM "MEMBERSHIPS" WHERE "MEMBERSHIPS"."TEAM_ID" = :1 AND "MEMBERSHIPS"."TYPE" = :2"#,
    );
}

#[test]
fn new_model_is_dirty_everywhere() {
    let mut user = User {
        name: "Ann".into(),
        ..Default::default()
    };
    assert!(!user.exists());
    assert_eq!(user.dirty().len(), User::COLUMNS.len());
    assert_eq!(user.get_original("name"), None);

    user.set_value("deleted_at", Value::Null).unwrap();
    assert!(user.set_value("nope", Value::Null).is_err());
    assert!(user.set_value("id", Value::from("x")).is_err());
    assert!(!user.trashed());
}

#[rustique::model(table = "posts")]
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Post {
    id: i64,
    title: String,
}

#[rustique::model(table = "tags")]
#[derive(Debug, Default)]
#[cfg_attr(all(), derive(serde::Serialize))]
struct Tag {
    id: i64,
}

#[test]
fn serde_skips_meta() {
    let post = Post {
        id: 1,
        title: "Hi".into(),
        ..Default::default()
    };
    let json = serde_json::to_string(&post).unwrap();
    assert_eq!(json, r#"{"id":1,"title":"Hi"}"#);
    assert_eq!(serde_json::from_str::<Post>(&json).unwrap(), post);

    let tag = serde_json::to_value(Tag::default()).unwrap();
    assert_eq!(tag, serde_json::json!({ "id": 0 }));
}

#[rustique::model(table = "authors")]
#[derive(Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
struct Author {
    id: i64,
    #[rustique(has_many(Post))]
    posts: rustique::Rel<Vec<Post>>,
    #[rustique(count)]
    posts_count: rustique::Rel<u64>,
}

#[test]
fn serde_skips_unloaded_relations() {
    let mut author = Author::default();
    assert_eq!(serde_json::to_string(&author).unwrap(), r#"{"id":0}"#);

    author.posts_count = rustique::Rel::Loaded(2);
    author.posts = rustique::Rel::Loaded(vec![Post::default()]);
    let json = serde_json::to_string(&author).unwrap();
    assert_eq!(
        json,
        r#"{"id":0,"posts":[{"id":0,"title":""}],"posts_count":2}"#
    );
    assert_eq!(serde_json::from_str::<Author>(&json).unwrap(), author);
    assert_eq!(
        serde_json::from_str::<Author>(r#"{"id":1}"#).unwrap().posts,
        rustique::Rel::NotLoaded
    );
}

#[rustique::model(table = "members")]
#[derive(Debug, Default, serde::Deserialize)]
struct Member {
    id: i64,
    #[rustique(fillable)]
    name: String,
    #[rustique(fillable)]
    bio: Option<String>,
    is_admin: bool,
    tenant_id: i64,
}

#[test]
fn fill_only_sets_fillable_fields() {
    assert_eq!(Member::FILLABLE, ["name", "bio"]);

    let mut member = Member {
        id: 1,
        name: "Ann".into(),
        bio: Some("Hi".into()),
        ..Default::default()
    };
    member.fill(MemberFill {
        name: Some("Bea".into()),
        ..Default::default()
    });
    assert_eq!(
        (member.name.as_str(), member.bio.as_deref()),
        ("Bea", Some("Hi"))
    );

    let fill: MemberFill = serde_json::from_str(r#"{"bio":null}"#).unwrap();
    member.fill(fill);
    assert_eq!((member.name.as_str(), member.bio), ("Bea", None));

    for guarded in ["id", "is_admin", "tenant_id"] {
        let json = format!(r#"{{"{guarded}":1}}"#);
        assert!(
            serde_json::from_str::<MemberFill>(&json).is_err(),
            "{guarded}"
        );
    }
    assert!(serde_json::from_str::<MemberFill>(r#"{"name":null}"#).is_err());

    // The model itself deserializes every field, for trusted data such as
    // a cache; untrusted input goes through `MemberFill`.
    let json = r#"{"id":1,"name":"Eve","bio":null,"is_admin":true,"tenant_id":2}"#;
    let member: Member = serde_json::from_str(json).unwrap();
    assert_eq!((member.id, member.is_admin, member.tenant_id), (1, true, 2));
}

#[rustique::model(table = "documents", uuid)]
#[derive(Debug, Default, serde::Deserialize)]
struct Document {
    id: String,
    #[rustique(fillable)]
    title: String,
}

#[test]
fn fill_guards_keys_that_are_not_incrementing() {
    // A request body must not pick a new row's key or re-key an existing one.
    assert_eq!(Document::FILLABLE, ["title"]);
    assert!(serde_json::from_str::<DocumentFill>(r#"{"id":"chosen","title":"x"}"#).is_err());
}
