//! Eloquent features added in the parity pass: aggregates of relations,
//! `has_one_through`, polymorphic many-to-many, self-relations, `where_keys`.

#![allow(dead_code)] // aggregate fields are only filled, never read, in these SQL tests

use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use rustique::{Dialect, Model, Rel, Value};

use crate::{assert_sql, binds};

#[rustique::model(table = "users", soft_deletes)]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    parent_id: Option<i64>,
    deleted_at: Option<NaiveDateTime>,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
    #[rustique(has_many(User, foreign_key = "parent_id"))]
    children: Rel<Vec<User>>,
    #[rustique(belongs_to_many(User, table = "friends", related_pivot_key = "friend_id"))]
    friends: Rel<Vec<User>>,
    #[rustique(aggregate)]
    posts_sum_votes: Rel<Option<Decimal>>,
    #[rustique(aggregate)]
    posts_exists: Rel<bool>,
    #[rustique(count)]
    children_count: Rel<u64>,
}

#[rustique::model(table = "posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    user_id: i64,
    votes: i64,
    #[rustique(morph_to_many(Tag, name = "taggable"))]
    tags: Rel<Vec<Tag>>,
}

#[rustique::model(table = "videos", morph_alias = "App\\Models\\Video")]
#[derive(Debug, Clone, Default)]
struct Video {
    id: i64,
    #[rustique(morph_to_many(Tag, name = "taggable", pivot = ["note"], pivot_timestamps))]
    tags: Rel<Vec<Tag>>,
}

#[rustique::model(table = "tags")]
#[derive(Debug, Clone, Default)]
struct Tag {
    id: i64,
    #[rustique(morphed_by_many(Post, name = "taggable"))]
    posts: Rel<Vec<Post>>,
    #[rustique(morphed_by_many(Video, name = "taggable"))]
    videos: Rel<Vec<Video>>,
}

#[rustique::model(table = "mechanics")]
#[derive(Debug, Clone, Default)]
struct Mechanic {
    id: i64,
    #[rustique(has_one_through(Owner, through = Car))]
    car_owner: Rel<Option<Owner>>,
}

#[rustique::model(table = "cars")]
#[derive(Debug, Clone, Default)]
struct Car {
    id: i64,
    mechanic_id: i64,
}

#[rustique::model(table = "owners")]
#[derive(Debug, Clone, Default)]
struct Owner {
    id: i64,
    car_id: i64,
}

#[test]
fn relation_aggregates() {
    let query = User::query()
        .with_sum("posts", "posts.votes")
        .with_max("posts", "votes")
        .with_exists("posts")
        .into_builder();
    assert_sql(
        &query,
        "SELECT `users`.*, (SELECT SUM(`posts`.`votes`) FROM `posts` WHERE `posts`.`user_id` = `users`.`id`) AS `posts_sum_votes`, \
         (SELECT MAX(`votes`) FROM `posts` WHERE `posts`.`user_id` = `users`.`id`) AS `posts_max_votes`, \
         CASE WHEN EXISTS (SELECT * FROM `posts` WHERE `posts`.`user_id` = `users`.`id`) THEN 1 ELSE 0 END AS `posts_exists` \
         FROM `users` WHERE `users`.`deleted_at` IS NULL",
        r#"SELECT "USERS".*, (SELECT SUM("POSTS"."VOTES") FROM "POSTS" WHERE "POSTS"."USER_ID" = "USERS"."ID") AS "POSTS_SUM_VOTES", (SELECT MAX("VOTES") FROM "POSTS" WHERE "POSTS"."USER_ID" = "USERS"."ID") AS "POSTS_MAX_VOTES", CASE WHEN EXISTS (SELECT * FROM "POSTS" WHERE "POSTS"."USER_ID" = "USERS"."ID") THEN 1 ELSE 0 END AS "POSTS_EXISTS" FROM "USERS" WHERE "USERS"."DELETED_AT" IS NULL"#,
    );
    let filtered = User::query()
        .with_exists_where("posts", |q| q.where_("votes", ">", 10))
        .into_builder();
    assert_eq!(binds(&filtered), [Value::I64(10)]);
    let summed = User::query()
        .with_sum_where("posts", "votes", |q| q.where_("votes", ">", 2))
        .into_builder();
    assert_eq!(
        summed.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT `users`.*, (SELECT SUM(`votes`) FROM `posts` WHERE `posts`.`user_id` = `users`.`id` \
         AND (`votes` > ?)) AS `posts_sum_votes` FROM `users` WHERE `users`.`deleted_at` IS NULL"
    );
    assert_eq!(binds(&summed), [Value::I64(2)]);
}

#[test]
fn self_relations_alias_the_inner_table() {
    let query = User::query()
        .where_has("children", |q| q.where_("orm_self.id", ">", 1))
        .with_count("children")
        .into_builder();
    assert_sql(
        &query,
        "SELECT `users`.*, (SELECT COUNT(*) FROM `users` AS `orm_self` WHERE `orm_self`.`deleted_at` IS NULL \
         AND `orm_self`.`parent_id` = `users`.`id`) AS `children_count` FROM `users` \
         WHERE EXISTS (SELECT * FROM `users` AS `orm_self` WHERE `orm_self`.`deleted_at` IS NULL \
         AND `orm_self`.`parent_id` = `users`.`id` AND (`orm_self`.`id` > ?)) AND `users`.`deleted_at` IS NULL",
        r#"SELECT "USERS".*, (SELECT COUNT(*) FROM "USERS" "ORM_SELF" WHERE "ORM_SELF"."DELETED_AT" IS NULL AND "ORM_SELF"."PARENT_ID" = "USERS"."ID") AS "CHILDREN_COUNT" FROM "USERS" WHERE EXISTS (SELECT * FROM "USERS" "ORM_SELF" WHERE "ORM_SELF"."DELETED_AT" IS NULL AND "ORM_SELF"."PARENT_ID" = "USERS"."ID" AND ("ORM_SELF"."ID" > :1)) AND "USERS"."DELETED_AT" IS NULL"#,
    );

    let friends = User::query().where_has("friends", |q| q).into_builder();
    assert_eq!(
        friends.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `users` WHERE EXISTS (SELECT `orm_self`.* FROM `users` AS `orm_self` \
         INNER JOIN `friends` ON `friends`.`friend_id` = `orm_self`.`id` \
         WHERE `orm_self`.`deleted_at` IS NULL AND `friends`.`user_id` = `users`.`id`) \
         AND `users`.`deleted_at` IS NULL"
    );
}

#[test]
fn has_one_through() {
    let mechanic = Mechanic {
        id: 4,
        ..Default::default()
    };
    assert_eq!(
        mechanic.car_owner().to_sql(Dialect::MySql).unwrap().sql,
        "SELECT `owners`.* FROM `owners` INNER JOIN `cars` ON `cars`.`id` = `owners`.`car_id` \
         WHERE `cars`.`mechanic_id` = ?"
    );
}

#[test]
fn morph_to_many_and_morphed_by_many() {
    let post = Post {
        id: 7,
        ..Default::default()
    };
    let tags = post.tags().into_builder();
    assert_eq!(
        tags.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT `tags`.* FROM `tags` INNER JOIN `taggables` ON `taggables`.`tag_id` = `tags`.`id` \
         WHERE `taggables`.`taggable_type` = ? AND `taggables`.`taggable_id` = ?"
    );
    assert_eq!(binds(&tags), [Value::from("posts"), Value::I64(7)]);

    let video = Video {
        id: 2,
        ..Default::default()
    };
    let video_tags = video.tags().into_builder();
    assert_eq!(
        video_tags.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT `tags`.*, `taggables`.`note` AS `orm_pivot_note`, \
         `taggables`.`created_at` AS `orm_pivot_created_at`, `taggables`.`updated_at` AS `orm_pivot_updated_at` \
         FROM `tags` INNER JOIN `taggables` ON `taggables`.`tag_id` = `tags`.`id` \
         WHERE `taggables`.`taggable_type` = ? AND `taggables`.`taggable_id` = ?"
    );
    assert_eq!(
        binds(&video_tags),
        [Value::from("App\\Models\\Video"), Value::I64(2)]
    );

    let tag = Tag {
        id: 3,
        ..Default::default()
    };
    let videos = tag.videos().into_builder();
    assert_eq!(
        videos.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT `videos`.* FROM `videos` INNER JOIN `taggables` ON `taggables`.`taggable_id` = `videos`.`id` \
         WHERE `taggables`.`taggable_type` = ? AND `taggables`.`tag_id` = ?"
    );
    assert_eq!(
        binds(&videos),
        [Value::from("App\\Models\\Video"), Value::I64(3)]
    );

    let with_posts = Tag::query().where_has("posts", |q| q).into_builder();
    assert_eq!(
        with_posts.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `tags` WHERE EXISTS (SELECT `posts`.* FROM `posts` \
         INNER JOIN `taggables` ON `taggables`.`taggable_id` = `posts`.`id` \
         WHERE `taggables`.`taggable_type` = ? AND `taggables`.`tag_id` = `tags`.`id`)"
    );
}

#[test]
fn where_keys() {
    let query = Post::query().where_keys([1, 2, 3]).into_builder();
    assert_sql(
        &query,
        "SELECT * FROM `posts` WHERE `posts`.`id` IN (?, ?, ?)",
        r#"SELECT * FROM "POSTS" WHERE "POSTS"."ID" IN (:1, :2, :3)"#,
    );
}

#[derive(Debug, Clone, rustique::MorphTo)]
enum Taggable {
    Post(Post),
    Video(Video),
}

#[rustique::model(table = "notes")]
#[derive(Debug, Clone, Default)]
struct Note {
    id: i64,
    noteable_type: String,
    noteable_id: i64,
    #[rustique(morph_to)]
    noteable: Rel<Option<Taggable>>,
}

#[test]
fn where_has_morph() {
    let query = Note::query()
        .where_("id", ">", 0)
        .where_has_morph("noteable", &[], |q, alias| {
            if alias == "posts" {
                q.where_("votes", ">", 5)
            } else {
                q
            }
        })
        .into_builder();
    assert_eq!(
        query.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `notes` WHERE `id` > ? AND ((`notes`.`noteable_type` = ? AND EXISTS (SELECT * FROM `posts` \
         WHERE `posts`.`id` = `notes`.`noteable_id` AND (`votes` > ?))) OR (`notes`.`noteable_type` = ? \
         AND EXISTS (SELECT * FROM `videos` WHERE `videos`.`id` = `notes`.`noteable_id`)))"
    );
    assert_eq!(
        binds(&query),
        [
            Value::I64(0),
            Value::from("posts"),
            Value::I64(5),
            Value::from("App\\Models\\Video")
        ]
    );
    let only_videos = Note::query()
        .where_doesnt_have_morph("noteable", &["App\\Models\\Video"], |q, _| q)
        .into_builder();
    assert_eq!(
        only_videos.to_sql(Dialect::MySql).unwrap().sql,
        "SELECT * FROM `notes` WHERE ((`notes`.`noteable_type` = ? AND NOT EXISTS (SELECT * FROM `videos` \
         WHERE `videos`.`id` = `notes`.`noteable_id`)))"
    );
}
