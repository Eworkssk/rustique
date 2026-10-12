use rustique::{Error, Model, Rel, Value};

use crate::{create_table, databases};

#[rustique::model(table = "orm_morph_posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    title: String,
    #[rustique(morph_many(Comment, name = "commentable"))]
    comments: Rel<Vec<Comment>>,
    #[rustique(count)]
    comments_count: Rel<u64>,
}

#[rustique::model(table = "orm_morph_videos", morph_alias = "App\\Models\\Video")]
#[derive(Debug, Clone, Default)]
struct Video {
    id: i64,
    title: String,
    #[rustique(morph_one(Comment, name = "commentable"))]
    comment: Rel<Option<Box<Comment>>>, // boxed: a comment holds its video
}

#[derive(Debug, Clone, rustique::MorphTo)]
enum Commentable {
    Post(Post),
    Video(Video),
}

#[rustique::model(table = "orm_morph_comments")]
#[derive(Debug, Clone, Default)]
struct Comment {
    id: i64,
    body: String,
    commentable_type: Option<String>,
    commentable_id: Option<i64>,
    #[rustique(morph_to)]
    commentable: Rel<Option<Commentable>>,
}

fn title(target: &Commentable) -> &str {
    match target {
        Commentable::Post(post) => &post.title,
        Commentable::Video(video) => &video.title,
    }
}

#[tokio::test]
async fn polymorphic_relations() {
    for db in databases().await {
        let text = ("title", "VARCHAR(50)", "VARCHAR2(50 CHAR)");
        create_table(&db, Post::TABLE, &[text]).await;
        create_table(&db, Video::TABLE, &[text]).await;
        create_table(
            &db,
            Comment::TABLE,
            &[
                ("body", "VARCHAR(50)", "VARCHAR2(50 CHAR)"),
                ("commentable_type", "VARCHAR(50)", "VARCHAR2(50 CHAR)"),
                ("commentable_id", "BIGINT", "NUMBER(19)"),
            ],
        )
        .await;
        for (table, name) in [
            (Post::TABLE, "p1"),
            (Post::TABLE, "p2"),
            (Video::TABLE, "v1"),
        ] {
            rustique::QueryBuilder::table(table)
                .insert(&db, &["title"], vec![vec![name.into()]])
                .await
                .unwrap();
        }
        // Post 1 and video 1 share id 1: only the type tells them apart.
        let comments = [
            ("on p1", Some("orm_morph_posts"), Some(1)),
            ("also p1", Some("orm_morph_posts"), Some(1)),
            ("on v1", Some("App\\Models\\Video"), Some(1)),
            ("orphan", None, None),
        ];
        for (body, kind, id) in comments {
            let comment = Comment {
                body: body.into(),
                commentable_type: kind.map(Into::into),
                commentable_id: id,
                ..Default::default()
            };
            Comment::create(&db, comment).await.unwrap();
        }

        // morph_many / morph_one, lazy and eager.
        let p1 = Post::find_or_fail(&db, 1).await.unwrap();
        let bodies: Vec<String> = p1
            .comments()
            .order_by("id")
            .pluck(&db, "body")
            .await
            .unwrap();
        assert_eq!(bodies, ["on p1", "also p1"]);
        let posts = Post::query()
            .with("comments")
            .with_count("comments")
            .order_by("id")
            .get(&db)
            .await
            .unwrap();
        let counts: Vec<_> = posts
            .iter()
            .map(|p| {
                (
                    p.comments.loaded().unwrap().len(),
                    *p.comments_count.loaded().unwrap(),
                )
            })
            .collect();
        assert_eq!(counts, [(2, 2), (0, 0)]);
        let with_comments = Post::query()
            .where_has("comments", |q| q)
            .count(&db)
            .await
            .unwrap();
        assert_eq!(with_comments, 1);
        let video = Video::query()
            .with("comment")
            .first_or_fail(&db)
            .await
            .unwrap();
        assert_eq!(
            video.comment.loaded().unwrap().as_ref().unwrap().body,
            "on v1"
        );

        // morph_to, eager across types (with a nested path) and lazy.
        let comments = Comment::query()
            .with("commentable")
            .order_by("id")
            .get(&db)
            .await
            .unwrap();
        let targets: Vec<Option<&str>> = comments
            .iter()
            .map(|c| c.commentable.loaded().unwrap().as_ref().map(title))
            .collect();
        assert_eq!(targets, [Some("p1"), Some("p1"), Some("v1"), None]);
        let on_video = comments[2].commentable(&db).await.unwrap().unwrap();
        assert!(matches!(on_video, Commentable::Video(ref v) if v.title == "v1"));
        assert!(comments[3].commentable(&db).await.unwrap().is_none());

        let nested = Comment::query()
            .where_("id", "=", 1)
            .with("commentable.comments")
            .first_or_fail(&db)
            .await
            .unwrap();
        let Some(Commentable::Post(post)) = nested.commentable.loaded().unwrap() else {
            panic!("expected a post");
        };
        assert_eq!(post.comments.loaded().unwrap().len(), 2);

        // Eloquent's morphWithCount: counts on the loaded posts only.
        let counted = Comment::query()
            .with_morph_count("commentable", &[(Post::MORPH_ALIAS, "comments")])
            .where_("id", "<=", 3)
            .order_by("id")
            .get(&db)
            .await
            .unwrap();
        let Some(Commentable::Post(post)) = counted[0].commentable.loaded().unwrap() else {
            panic!("expected a post");
        };
        assert_eq!(*post.comments_count.loaded().unwrap(), 2);
        assert!(matches!(
            counted[2].commentable.loaded().unwrap(),
            Some(Commentable::Video(_))
        ));
        let not_morph = Post::query()
            .with_morph_count("comments", &[("x", "y")])
            .get(&db)
            .await
            .unwrap_err();
        assert!(
            not_morph.to_string().contains("not a morph_to"),
            "{not_morph}"
        );

        Comment::query()
            .where_("id", "=", 4)
            .update(
                &db,
                vec![
                    ("commentable_type", Value::from("unknown")),
                    ("commentable_id", Value::from(1)),
                ],
            )
            .await
            .unwrap();
        let error = Comment::query()
            .with("commentable")
            .get(&db)
            .await
            .unwrap_err();
        assert!(matches!(error, Error::UnknownMorphType(ref t) if t == "unknown"));
    }
}
