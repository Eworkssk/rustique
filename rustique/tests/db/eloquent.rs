//! Eloquent features of the parity pass: `sole`, `value`, `find_many`,
//! `simple_paginate`, `chunk`, relation aggregates, `sync_without_detaching`
//! / `toggle`, `has_one_through`, polymorphic many-to-many and self-relations.

use chrono::NaiveDateTime;
use rustique::{Db, Dialect, Error, Executor, Model, Prunable, Query, QueryBuilder, Rel, Schema};

use crate::{create_table, databases};

#[rustique::model(table = "orm_e_users")]
#[derive(Debug, Clone, Default, PartialEq)]
struct User {
    id: i64,
    parent_id: Option<i64>,
    name: String,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
    #[rustique(has_many(User, foreign_key = "parent_id"))]
    children: Rel<Vec<User>>,
    #[rustique(has_one_through(Comment, through = Post))]
    first_comment: Rel<Option<Comment>>,
    #[rustique(aggregate)]
    posts_sum_votes: Rel<Option<i64>>,
    #[rustique(aggregate)]
    posts_avg_votes: Rel<Option<f64>>,
    #[rustique(aggregate)]
    posts_exists: Rel<bool>,
    #[rustique(count)]
    children_count: Rel<u64>,
}

#[rustique::model(table = "orm_e_posts")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Post {
    id: i64,
    user_id: i64,
    votes: i64,
    #[rustique(morph_to_many(Tag, name = "taggable", table = "orm_e_taggables"))]
    tags: Rel<Vec<Tag>>,
}

#[rustique::model(table = "orm_e_comments")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Comment {
    id: i64,
    post_id: i64,
    body: String,
}

#[rustique::model(table = "orm_e_videos")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Video {
    id: i64,
    title: String,
    #[rustique(morph_to_many(Tag, name = "taggable", table = "orm_e_taggables"))]
    tags: Rel<Vec<Tag>>,
}

#[rustique::model(table = "orm_e_tags")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Tag {
    id: i64,
    name: String,
    #[rustique(morphed_by_many(Post, name = "taggable", table = "orm_e_taggables"))]
    posts: Rel<Vec<Post>>,
    #[rustique(morphed_by_many(Video, name = "taggable", table = "orm_e_taggables"))]
    videos: Rel<Vec<Video>>,
}

#[rustique::model(table = "orm_e_tokens", uuid)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Token {
    id: String,
    name: String,
}

#[rustique::model(table = "orm_e_events", ulid, soft_deletes)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Event {
    id: String,
    name: String,
    happened: NaiveDateTime,
    deleted_at: Option<NaiveDateTime>,
}

impl Prunable for Event {
    fn prunable() -> Query<Self> {
        Event::query().where_("name", "like", "old%")
    }
}

const TEXT: (&str, &str) = ("VARCHAR(20)", "VARCHAR2(20 CHAR)");
const ID: (&str, &str) = ("BIGINT", "NUMBER(19)");

/// Users Ann (posts with 3 and 5 votes, children Bo and Cy) and Bo (one
/// post, 1 vote), Cy without posts. Comments on Ann's first post.
async fn seed(db: &Db) {
    create_table(
        db,
        "orm_e_users",
        &[("parent_id", ID.0, ID.1), ("name", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(
        db,
        "orm_e_posts",
        &[("user_id", ID.0, ID.1), ("votes", ID.0, ID.1)],
    )
    .await;
    create_table(
        db,
        "orm_e_comments",
        &[("post_id", ID.0, ID.1), ("body", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(db, "orm_e_videos", &[("title", TEXT.0, TEXT.1)]).await;
    create_table(db, "orm_e_tags", &[("name", TEXT.0, TEXT.1)]).await;
    create_table(
        db,
        "orm_e_taggables",
        &[
            ("tag_id", ID.0, ID.1),
            ("taggable_id", ID.0, ID.1),
            ("taggable_type", TEXT.0, TEXT.1),
        ],
    )
    .await;
    let users = QueryBuilder::table("orm_e_users");
    users
        .insert(
            db,
            &["parent_id", "name"],
            vec![
                vec![rustique::Value::Null, "Ann".into()],
                vec![1.into(), "Bo".into()],
                vec![1.into(), "Cy".into()],
            ],
        )
        .await
        .unwrap();
    QueryBuilder::table("orm_e_posts")
        .insert(
            db,
            &["user_id", "votes"],
            vec![
                vec![1.into(), 3.into()],
                vec![1.into(), 5.into()],
                vec![2.into(), 1.into()],
            ],
        )
        .await
        .unwrap();
    QueryBuilder::table("orm_e_comments")
        .insert(
            db,
            &["post_id", "body"],
            vec![vec![1.into(), "nice".into()]],
        )
        .await
        .unwrap();
    QueryBuilder::table("orm_e_videos")
        .insert(db, &["title"], vec![vec!["v".into()]])
        .await
        .unwrap();
    QueryBuilder::table("orm_e_tags")
        .insert(
            db,
            &["name"],
            vec![
                vec!["red".into()],
                vec!["blue".into()],
                vec!["green".into()],
            ],
        )
        .await
        .unwrap();
}

fn names(users: &[User]) -> Vec<&str> {
    users.iter().map(|u| u.name.as_str()).collect()
}

/// One test: the parts share their tables.
#[tokio::test]
async fn eloquent_features() {
    for db in databases().await {
        seed(&db).await;
        query_helpers(&db).await;
        seed(&db).await;
        relation_aggregates_and_self_relations(&db).await;
        seed(&db).await;
        polymorphic_many_to_many(&db).await;
        unique_ids_defaults_and_pruning(&db).await;
    }
}

async fn unique_ids_defaults_and_pruning(db: &Db) {
    for table in ["orm_e_tokens", "orm_e_events"] {
        Schema::drop_if_exists(db, table).await.unwrap();
    }
    Schema::create(db, "orm_e_tokens", |t| {
        t.uuid("id").primary();
        t.string("name", 20);
    })
    .await
    .unwrap();
    Schema::create(db, "orm_e_events", |t| {
        t.ulid("id").primary();
        t.string("name", 20).comment("what happened");
        t.datetime("happened").use_current();
        t.soft_deletes();
        t.comment("Events");
    })
    .await
    .unwrap();

    let token = Token::create(
        db,
        Token {
            name: "a".into(),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(token.id.len(), 36);
    let kept = Token {
        id: "fixed".into(),
        name: "b".into(),
        ..Default::default()
    };
    assert_eq!(Token::create(db, kept).await.unwrap().id, "fixed");
    assert_eq!(
        Token::find_or_fail(db, token.id.clone())
            .await
            .unwrap()
            .name,
        "a"
    );

    let mut events: Vec<Event> = ["old-1", "old-2", "new"]
        .into_iter()
        .map(|name| Event {
            name: name.into(),
            ..Default::default()
        })
        .collect();
    Event::insert_many(db, &mut events).await.unwrap();
    assert!(events.iter().all(|e| e.id.len() == 26));
    // The database fills `happened` (UTC now) when the insert leaves it out.
    const MANUAL: &str = "01arz3ndektsv4rrffq69g5fav";
    QueryBuilder::table("orm_e_events")
        .insert(
            db,
            &["id", "name"],
            vec![vec![MANUAL.into(), "old-3".into()]],
        )
        .await
        .unwrap();
    let happened: NaiveDateTime = QueryBuilder::table("orm_e_events")
        .where_("id", "=", MANUAL)
        .value(db, "happened")
        .await
        .unwrap();
    let age = chrono::Utc::now().naive_utc() - happened;
    assert!(age.num_minutes().abs() < 5, "{happened}");

    events[0].delete(db).await.unwrap(); // soft-deleted rows are pruned too
    assert_eq!(Event::prune(db).await.unwrap(), 3);
    assert_eq!(
        Event::query()
            .with_trashed()
            .pluck::<String>(db, "name")
            .await
            .unwrap(),
        ["new"]
    );
    assert_eq!(Event::mass_prune(db).await.unwrap(), 0);
}

async fn query_helpers(db: &Db) {
    {
        let ann = User::query()
            .where_("name", "=", "Ann")
            .sole(db)
            .await
            .unwrap();
        assert_eq!(ann.id, 1);
        let none = User::query().where_("name", "=", "Zed").sole(db).await;
        assert!(matches!(none, Err(Error::NotFound(_))));
        let many = User::query().where_("parent_id", "=", 1).sole(db).await;
        assert!(matches!(many, Err(Error::MultipleRecords(_))));

        let name: String = User::query()
            .where_("id", "=", 2)
            .value(db, "name")
            .await
            .unwrap();
        assert_eq!(name, "Bo");
        let missing: Option<String> = User::query()
            .where_("id", "=", 9)
            .value(db, "name")
            .await
            .unwrap();
        assert_eq!(missing, None);

        let found = User::find_many(db, [3, 1, 9]).await.unwrap();
        let mut found = names(&found);
        found.sort_unstable();
        assert_eq!(found, ["Ann", "Cy"]);

        let page = User::query()
            .order_by("id")
            .simple_paginate(db, 1, 2)
            .await
            .unwrap();
        assert_eq!(names(&page.items), ["Ann", "Bo"]);
        assert!(page.has_more_pages());
        let last = User::query()
            .order_by("id")
            .simple_paginate(db, 2, 2)
            .await
            .unwrap();
        assert_eq!(names(&last.items), ["Cy"]);
        assert!(!last.has_more);

        let mut chunks = User::query().with("posts").chunk(2);
        let mut seen = Vec::new();
        while let Some(users) = chunks.next(db).await.unwrap() {
            assert!(users.iter().all(|u| u.posts.is_loaded()));
            seen.push(names(&users).join(","));
        }
        assert_eq!(seen, ["Ann,Bo", "Cy"]);

        let mut rows = QueryBuilder::table("orm_e_users").chunk(2);
        let unordered = rows.next(db).await.unwrap_err().to_string();
        assert!(unordered.contains("order_by"), "{unordered}");
    }
}

async fn relation_aggregates_and_self_relations(db: &Db) {
    {
        let users = User::query()
            .with_sum("posts", "votes")
            .with_avg("posts", "votes")
            .with_exists("posts")
            .with_count("children")
            .order_by("id")
            .get(db)
            .await
            .unwrap();
        let sums: Vec<_> = users
            .iter()
            .map(|u| *u.posts_sum_votes.loaded().unwrap())
            .collect();
        assert_eq!(sums, [Some(8), Some(1), None]);
        assert_eq!(*users[0].posts_avg_votes.loaded().unwrap(), Some(4.0));
        let exists: Vec<bool> = users
            .iter()
            .map(|u| *u.posts_exists.loaded().unwrap())
            .collect();
        assert_eq!(exists, [true, true, false]);
        let children: Vec<u64> = users
            .iter()
            .map(|u| *u.children_count.loaded().unwrap())
            .collect();
        assert_eq!(children, [2, 0, 0]);

        let parents = User::query()
            .where_has("children", |q| q.where_("orm_self.name", "=", "Cy"))
            .get(db)
            .await
            .unwrap();
        assert_eq!(names(&parents), ["Ann"]);

        let mut ann = User::find_or_fail(db, 1).await.unwrap();
        ann.load(db, "first_comment").await.unwrap();
        assert_eq!(
            ann.first_comment.loaded().unwrap().as_ref().unwrap().body,
            "nice"
        );
    }
}

async fn polymorphic_many_to_many(db: &Db) {
    {
        let post = Post::find_or_fail(db, 1).await.unwrap();
        let video = Video::find_or_fail(db, 1).await.unwrap();
        // Same key 1 for the post and the video: only the type tells them apart.
        post.tags_pivot().unwrap().attach(db, [1, 2]).await.unwrap();
        video.tags_pivot().unwrap().attach(db, [2]).await.unwrap();

        let tag_names = |tags: Vec<Tag>| {
            let mut names: Vec<String> = tags.into_iter().map(|t| t.name).collect();
            names.sort_unstable();
            names
        };
        assert_eq!(
            tag_names(post.tags().get(db).await.unwrap().into_inner()),
            ["blue", "red"]
        );
        assert_eq!(
            tag_names(video.tags().get(db).await.unwrap().into_inner()),
            ["blue"]
        );

        let pivot = post.tags_pivot().unwrap();
        pivot.sync_without_detaching(db, [2, 3]).await.unwrap();
        assert_eq!(
            tag_names(post.tags().get(db).await.unwrap().into_inner()),
            ["blue", "green", "red"]
        );
        pivot.toggle(db, [1, 3]).await.unwrap();
        assert_eq!(
            tag_names(post.tags().get(db).await.unwrap().into_inner()),
            ["blue"]
        );
        pivot.toggle(db, [1]).await.unwrap();
        assert_eq!(
            tag_names(post.tags().get(db).await.unwrap().into_inner()),
            ["blue", "red"]
        );
        // The video's pivot row was left alone throughout.
        assert_eq!(
            tag_names(video.tags().get(db).await.unwrap().into_inner()),
            ["blue"]
        );

        let blue = Tag::query()
            .where_("name", "=", "blue")
            .with("posts")
            .with("videos")
            .first(db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(blue.posts.loaded().unwrap().len(), 1);
        assert_eq!(blue.videos.loaded().unwrap().len(), 1);
        let red_videos = Tag::query()
            .where_has("videos", |q| q)
            .pluck::<String>(db, "name")
            .await
            .unwrap();
        assert_eq!(red_videos, ["blue"]);
    }
}

/// Which pooled session a statement ran on.
async fn session(db: &impl Executor) -> String {
    let sql = match db.dialect() {
        Dialect::MySql => "SELECT CAST(CONNECTION_ID() AS CHAR) AS s",
        _ => "SELECT SYS_CONTEXT('USERENV', 'SID') AS s FROM dual",
    };
    db.select(sql, []).await.unwrap()[0].get("s").unwrap()
}

#[tokio::test]
async fn read_replicas() {
    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        // One connection per pool, so a pool always answers with the same session.
        let db = Db::builder(&url)
            .max_connections(1)
            .min_connections(1)
            .read_url(&url)
            .connect()
            .await
            .unwrap();
        let writer = session(&db.write()).await;
        let replica = session(&db).await;
        assert_ne!(writer, replica);
        assert_eq!(session(&db).await, replica);
        let locked = match db.dialect() {
            Dialect::MySql => "SELECT CAST(CONNECTION_ID() AS CHAR) AS s FOR UPDATE",
            _ => "SELECT SYS_CONTEXT('USERENV', 'SID') AS s FROM dual FOR UPDATE",
        };
        let locked: String = db.select(locked, []).await.unwrap()[0].get("s").unwrap();
        assert_eq!(locked, writer);
        let tx = db.begin().await.unwrap();
        assert_eq!(session(&tx).await, writer);
        tx.rollback().await.unwrap();
        db.disconnect().await.unwrap();
    }
}
