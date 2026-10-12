use chrono::NaiveDateTime;
use rustique::{Db, Dialect, Executor, Model, Rel, Transaction};

use crate::{create_table, databases, drop_table};

#[rustique::model(table = "orm_r_countries")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Country {
    id: i64,
    name: String,
    #[rustique(has_many(User))]
    users: Rel<Vec<User>>,
    #[rustique(has_many_through(Post, through = User))]
    posts: Rel<Vec<Post>>,
}

#[rustique::model(table = "orm_r_users")]
#[derive(Debug, Clone, Default, PartialEq)]
struct User {
    id: i64,
    country_id: i64,
    name: String,
    #[rustique(belongs_to(Country))]
    country: Rel<Option<Country>>,
    #[rustique(has_one(Profile))]
    profile: Rel<Option<Profile>>,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
    #[rustique(belongs_to_many(Role, table = "orm_r_role_user"))]
    roles: Rel<Vec<Role>>,
    #[rustique(count)]
    posts_count: Rel<u64>,
}

#[rustique::model(table = "orm_r_profiles")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Profile {
    id: i64,
    user_id: i64,
    bio: String,
}

#[rustique::model(table = "orm_r_posts", soft_deletes)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Post {
    id: i64,
    user_id: i64,
    title: String,
    deleted_at: Option<NaiveDateTime>,
    #[rustique(belongs_to(User))]
    user: Rel<Option<Box<User>>>,
    #[rustique(has_many(Comment))]
    comments: Rel<Vec<Comment>>,
}

#[rustique::model(table = "orm_r_comments")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Comment {
    id: i64,
    post_id: i64,
    body: String,
}

#[rustique::model(table = "orm_r_roles")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Role {
    id: i64,
    name: String,
}

const TEXT: (&str, &str) = ("VARCHAR(20)", "VARCHAR2(20 CHAR)");
const ID: (&str, &str) = ("BIGINT", "NUMBER(19)");

/// Countries SK and CZ; users Ann and Bo (SK), Cy (CZ); Ann has a profile,
/// posts p1, p2 and roles admin, editor; Bo has the deleted post p3 and role
/// editor; Cy has post p4. Comments: two on p1, one on p4.
async fn seed(db: &Db) -> Vec<User> {
    let datetime = if db.dialect() == Dialect::MySql {
        "DATETIME(6)"
    } else {
        "TIMESTAMP(6)"
    };
    create_table(db, "orm_r_countries", &[("name", TEXT.0, TEXT.1)]).await;
    create_table(
        db,
        "orm_r_users",
        &[("country_id", ID.0, ID.1), ("name", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(
        db,
        "orm_r_profiles",
        &[("user_id", ID.0, ID.1), ("bio", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(
        db,
        "orm_r_posts",
        &[
            ("user_id", ID.0, ID.1),
            ("title", TEXT.0, TEXT.1),
            ("deleted_at", datetime, datetime),
        ],
    )
    .await;
    create_table(
        db,
        "orm_r_comments",
        &[("post_id", ID.0, ID.1), ("body", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(db, "orm_r_roles", &[("name", TEXT.0, TEXT.1)]).await;
    drop_table(db, "orm_r_role_user").await;
    let id = if db.dialect() == Dialect::MySql {
        ID.0
    } else {
        ID.1
    };
    let pivot = format!(
        "CREATE TABLE orm_r_role_user (user_id {id}, role_id {id}, PRIMARY KEY (user_id, role_id))"
    );
    db.statement(&pivot, []).await.unwrap();

    let mut countries = Vec::new();
    for name in ["SK", "CZ"] {
        let country = Country {
            name: name.into(),
            ..Default::default()
        };
        countries.push(Country::create(db, country).await.unwrap());
    }
    let mut users = Vec::new();
    for (name, country) in [("Ann", 0), ("Bo", 0), ("Cy", 1)] {
        let user = User {
            country_id: countries[country].id,
            name: name.into(),
            ..Default::default()
        };
        users.push(User::create(db, user).await.unwrap());
    }
    let profile = Profile {
        user_id: users[0].id,
        bio: "hi".into(),
        ..Default::default()
    };
    Profile::create(db, profile).await.unwrap();

    let mut posts = Vec::new();
    for (title, user) in [("p1", 0), ("p2", 0), ("p3", 1), ("p4", 2)] {
        let post = Post {
            user_id: users[user].id,
            title: title.into(),
            ..Default::default()
        };
        posts.push(Post::create(db, post).await.unwrap());
    }
    posts[2].delete(db).await.unwrap();
    for (body, post) in [("c1", 0), ("c2", 0), ("c3", 3)] {
        let comment = Comment {
            post_id: posts[post].id,
            body: body.into(),
            ..Default::default()
        };
        Comment::create(db, comment).await.unwrap();
    }

    let mut roles = Vec::new();
    for name in ["admin", "editor"] {
        let role = Role {
            name: name.into(),
            ..Default::default()
        };
        roles.push(Role::create(db, role).await.unwrap());
    }
    let (admin, editor) = (roles[0].id, roles[1].id);
    users[0]
        .roles_pivot()
        .unwrap()
        .attach(db, [admin, editor])
        .await
        .unwrap();
    users[1]
        .roles_pivot()
        .unwrap()
        .attach(db, [editor])
        .await
        .unwrap();
    users
}

fn titles(posts: &[Post]) -> Vec<&str> {
    let mut titles: Vec<&str> = posts.iter().map(|p| p.title.as_str()).collect();
    titles.sort_unstable();
    titles
}

/// The scenarios share tables, so they run one after another.
#[tokio::test]
async fn relations() {
    for db in databases().await {
        let users = seed(&db).await;
        lazy_relations(&db, &users).await;
        eager_loading_runs_one_query_per_relation(&db).await;
        eager_loading_every_relation_kind(&db).await;
        where_has_and_with_count(&db).await;
        pivot_attach_detach_sync(&db, &users).await;
    }
}

async fn lazy_relations(db: &Db, users: &[User]) {
    {
        let db = db.clone();
        let ann = &users[0];

        assert_eq!(titles(&ann.posts().get(&db).await.unwrap()), ["p1", "p2"]);
        let roles = ann.roles().order_by("name").get(&db).await.unwrap();
        let names: Vec<&str> = roles.iter().map(|r| r.name.as_str()).collect();
        assert_eq!(names, ["admin", "editor"]);
        assert_eq!(ann.profile().first_or_fail(&db).await.unwrap().bio, "hi");
        let sk = ann.country().first_or_fail(&db).await.unwrap();
        assert_eq!(sk.name, "SK");
        // Through Ann and Bo; Bo's post is soft-deleted.
        assert_eq!(titles(&sk.posts().get(&db).await.unwrap()), ["p1", "p2"]);

        let p4 = Post::query()
            .where_("title", "=", "p4")
            .first_or_fail(&db)
            .await
            .unwrap();
        assert_eq!(p4.user().first_or_fail(&db).await.unwrap().name, "Cy");
        assert_eq!(p4.comments().count(&db).await.unwrap(), 1);
    }
}

/// Statements run by the session, read from MySQL's counters.
async fn statements(tx: &Transaction) -> u64 {
    let rows = tx
        .select("SHOW SESSION STATUS LIKE 'Com_stmt_execute'", [])
        .await
        .unwrap();
    rows[0].get::<String>("Value").unwrap().parse().unwrap()
}

async fn eager_loading_runs_one_query_per_relation(db: &Db) {
    {
        let tx = db.begin().await.unwrap();
        let mysql = db.dialect() == Dialect::MySql;
        let before = if mysql { statements(&tx).await } else { 0 };
        let overhead = if mysql {
            statements(&tx).await - before
        } else {
            0
        };

        let users = User::query()
            .with("posts.comments")
            .order_by("name")
            .get(&tx)
            .await
            .unwrap();
        if mysql {
            let after = statements(&tx).await;
            // users, posts and comments, plus the counter query itself.
            assert_eq!(after - before - 2 * overhead, 3);
        }
        tx.rollback().await.unwrap();

        let ann = &users[0];
        let posts = ann.posts.loaded().unwrap();
        assert_eq!(titles(posts), ["p1", "p2"]);
        let mut comments: Vec<usize> = posts
            .iter()
            .map(|p| p.comments.loaded().unwrap().len())
            .collect();
        comments.sort_unstable();
        assert_eq!(comments, [0, 2]);
        assert!(users[1].posts.loaded().unwrap().is_empty(), "p3 is deleted");
        assert_eq!(titles(users[2].posts.loaded().unwrap()), ["p4"]);
        assert!(ann.roles.loaded().is_err(), "not requested");
    }
}

async fn eager_loading_every_relation_kind(db: &Db) {
    {
        let db = db.clone();
        let users = User::query()
            .with("country")
            .with("profile")
            .with("roles")
            .order_by("name")
            .get(&db)
            .await
            .unwrap();
        let [ann, bo, cy] = &users[..] else {
            panic!("three users expected")
        };
        assert_eq!(ann.country.loaded().unwrap().as_ref().unwrap().name, "SK");
        assert_eq!(cy.country.loaded().unwrap().as_ref().unwrap().name, "CZ");
        assert_eq!(ann.profile.loaded().unwrap().as_ref().unwrap().bio, "hi");
        assert_eq!(bo.profile, Rel::Loaded(None));
        assert_eq!(ann.roles.loaded().unwrap().len(), 2);
        assert_eq!(bo.roles.loaded().unwrap()[0].name, "editor");
        assert!(cy.roles.loaded().unwrap().is_empty());

        let countries = Country::query()
            .with("users")
            .with("posts.user")
            .order_by("name")
            .get(&db)
            .await
            .unwrap();
        let (cz, sk) = (&countries[0], &countries[1]);
        assert_eq!(sk.users.loaded().unwrap().len(), 2);
        assert_eq!(titles(sk.posts.loaded().unwrap()), ["p1", "p2"]);
        assert_eq!(titles(cz.posts.loaded().unwrap()), ["p4"]);
        let author = cz.posts.loaded().unwrap()[0].user.loaded().unwrap();
        assert_eq!(author.as_ref().unwrap().name, "Cy");

        let mut bo = bo.clone();
        bo.load(&db, "posts").await.unwrap();
        assert!(bo.posts.loaded().unwrap().is_empty());
    }
}

async fn where_has_and_with_count(db: &Db) {
    {
        let db = db.clone();
        let names = |users: rustique::Collection<User>| -> Vec<String> {
            users.into_iter().map(|u| u.name).collect()
        };

        let commented = User::query()
            .where_has("posts", |q| {
                q.where_("title", "=", "p4").or_where("title", "=", "p3")
            })
            .get(&db)
            .await
            .unwrap();
        assert_eq!(names(commented), ["Cy"], "p3 is deleted");

        let postless = User::query()
            .where_doesnt_have("posts", |q| q)
            .get(&db)
            .await
            .unwrap();
        assert_eq!(names(postless), ["Bo"]);

        let editors = User::query()
            .where_has("roles", |q| q.where_("name", "=", "editor"))
            .order_by("name")
            .get(&db)
            .await
            .unwrap();
        assert_eq!(names(editors), ["Ann", "Bo"]);

        let counts: Vec<u64> = User::query()
            .with_count("posts")
            .order_by("name")
            .get(&db)
            .await
            .unwrap()
            .iter()
            .map(|u| *u.posts_count.loaded().unwrap())
            .collect();
        assert_eq!(counts, [2, 0, 1]);
    }
}

async fn pivot_attach_detach_sync(db: &Db, users: &[User]) {
    {
        let db = db.clone();
        let roles = Role::query().order_by("name").get(&db).await.unwrap();
        let (admin, editor) = (roles[0].id, roles[1].id);
        let cy = &users[2];
        let pivot = cy.roles_pivot().unwrap();
        let role_names = || async {
            let roles = cy.roles().order_by("name").get(&db).await.unwrap();
            roles.into_iter().map(|r| r.name).collect::<Vec<_>>()
        };

        pivot.sync(&db, [editor, admin]).await.unwrap();
        assert_eq!(role_names().await, ["admin", "editor"]);
        pivot.sync(&db, [editor]).await.unwrap();
        assert_eq!(role_names().await, ["editor"]);
        assert_eq!(pivot.detach(&db, [editor]).await.unwrap(), 1);
        assert!(role_names().await.is_empty());
        pivot.attach(&db, [admin]).await.unwrap();
        assert_eq!(pivot.detach_all(&db).await.unwrap(), 1);
    }
}

#[rustique::model(table = "orm_r_lines", incrementing = false)]
#[derive(Debug, Clone, Default, PartialEq)]
struct Line {
    #[rustique(primary_key)]
    order_no: i64,
    #[rustique(primary_key)]
    line_no: i64,
    product: String,
    #[rustique(has_many(LineNote, foreign_key = ["order_no", "line_no"], local_key = ["order_no", "line_no"]))]
    notes: Rel<Vec<LineNote>>,
}

#[rustique::model(table = "orm_r_line_notes")]
#[derive(Debug, Clone, Default, PartialEq)]
struct LineNote {
    id: i64,
    order_no: i64,
    line_no: i64,
    note: String,
    #[rustique(belongs_to(Line, foreign_key = ["order_no", "line_no"]))]
    line: Rel<Option<Line>>,
}

#[tokio::test]
async fn composite_key_relations() {
    for db in databases().await {
        drop_table(&db, "orm_r_lines").await;
        let (id, text) = if db.dialect() == Dialect::MySql {
            (ID.0, TEXT.0)
        } else {
            (ID.1, TEXT.1)
        };
        let lines = format!(
            "CREATE TABLE orm_r_lines (order_no {id}, line_no {id}, product {text}, \
             PRIMARY KEY (order_no, line_no))"
        );
        db.statement(&lines, []).await.unwrap();
        create_table(
            &db,
            "orm_r_line_notes",
            &[
                ("order_no", ID.0, ID.1),
                ("line_no", ID.0, ID.1),
                ("note", TEXT.0, TEXT.1),
            ],
        )
        .await;

        for (order_no, line_no, product) in [(1, 1, "nut"), (1, 2, "bolt"), (2, 1, "nut")] {
            let line = Line {
                order_no,
                line_no,
                product: product.into(),
                ..Default::default()
            };
            Line::create(&db, line).await.unwrap();
        }
        for (order_no, line_no, note) in [(1, 2, "a"), (1, 2, "b"), (2, 1, "c")] {
            let note = LineNote {
                order_no,
                line_no,
                note: note.into(),
                ..Default::default()
            };
            LineNote::create(&db, note).await.unwrap();
        }

        let lines = Line::query()
            .with("notes")
            .order_by("order_no")
            .order_by("line_no")
            .get(&db)
            .await
            .unwrap();
        let counts: Vec<usize> = lines
            .iter()
            .map(|l| l.notes.loaded().unwrap().len())
            .collect();
        assert_eq!(counts, [0, 2, 1]);

        let notes = LineNote::query()
            .with("line")
            .order_by("note")
            .get(&db)
            .await
            .unwrap();
        let products: Vec<&str> = notes
            .iter()
            .map(|n| n.line.loaded().unwrap().as_ref().unwrap().product.as_str())
            .collect();
        assert_eq!(products, ["bolt", "bolt", "nut"]);
        assert_eq!(lines[1].notes().count(&db).await.unwrap(), 2);
    }
}
