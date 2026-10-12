//! Query counts (N+1) and large data sets.
//!
//! Statements are counted from the `rustique::query` log: a global subscriber keeps
//! every logged statement, and a test counts those naming its own tables, so
//! tests running in parallel do not disturb each other.

use std::sync::{Mutex, Once};

use rustique::{Db, Dialect, Executor, Model, QueryBuilder, Rel, Value};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::{Context, Layer, SubscriberExt};

use crate::{create_table, databases};

static STATEMENTS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Keeps the SQL of every `DEBUG` event of `rustique::query` (one per statement).
struct Counter;

impl<S: Subscriber> Layer<S> for Counter {
    fn on_event(&self, event: &Event<'_>, _: Context<'_, S>) {
        let meta = event.metadata();
        if meta.target() == "rustique::query" && *meta.level() == Level::DEBUG {
            let mut sql = Sql(None);
            event.record(&mut sql);
            STATEMENTS.lock().unwrap().extend(sql.0);
        }
    }
}

struct Sql(Option<String>);

impl Visit for Sql {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "sql" {
            self.0 = Some(value.to_owned());
        }
    }

    // Ignoring the rest also skips formatting the bindings.
    fn record_debug(&mut self, _: &Field, _: &dyn std::fmt::Debug) {}
}

/// Runs `f` and returns its output plus the number of statements naming `table`.
async fn queries_on<T>(table: &str, f: impl Future<Output = T>) -> (T, usize) {
    static INIT: Once = Once::new();
    INIT.call_once(|| {
        let subscriber = tracing_subscriber::registry().with(Counter);
        tracing::subscriber::set_global_default(subscriber).unwrap();
    });
    let count = || {
        let statements = STATEMENTS.lock().unwrap();
        statements
            .iter()
            .filter(|sql| sql.to_lowercase().contains(table))
            .count()
    };
    let before = count();
    let output = f.await;
    (output, count() - before)
}

#[rustique::model(table = "orm_perf_users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    name: String,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
}

#[rustique::model(table = "orm_perf_posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    user_id: i64,
    title: String,
    #[rustique(belongs_to(User))]
    user: Rel<Option<User>>,
    #[rustique(has_many(Comment))]
    comments: Rel<Vec<Comment>>,
}

#[rustique::model(table = "orm_perf_comments")]
#[derive(Debug, Clone, Default)]
struct Comment {
    id: i64,
    post_id: i64,
    body: String,
}

const TEXT: (&str, &str) = ("VARCHAR(20)", "VARCHAR2(20 CHAR)");
const ID: (&str, &str) = ("BIGINT", "NUMBER(19)");

/// `users` users with ids 1..=users, one post per user and one comment per
/// post, inserted in SQL so large sets stay fast.
async fn seed(db: &Db, users: u32) {
    create_table(db, "orm_perf_users", &[("name", TEXT.0, TEXT.1)]).await;
    create_table(
        db,
        "orm_perf_posts",
        &[("user_id", ID.0, ID.1), ("title", TEXT.0, TEXT.1)],
    )
    .await;
    create_table(
        db,
        "orm_perf_comments",
        &[("post_id", ID.0, ID.1), ("body", TEXT.0, TEXT.1)],
    )
    .await;

    // 1..=100_000 from five cross-joined digit tables.
    let digit = |n: u32| {
        if db.dialect() == Dialect::MySql {
            format!("SELECT {n} AS n")
        } else {
            format!("SELECT {n} AS n FROM dual")
        }
    };
    let digits = (0..10).map(digit).collect::<Vec<_>>().join(" UNION ALL ");
    let numbers = format!(
        "SELECT a.n + 10 * b.n + 100 * c.n + 1000 * d.n + 10000 * e.n + 1 AS n \
         FROM ({digits}) a CROSS JOIN ({digits}) b CROSS JOIN ({digits}) c \
         CROSS JOIN ({digits}) d CROSS JOIN ({digits}) e"
    );
    let statements = [
        format!(
            "INSERT INTO orm_perf_users (id, name) \
             SELECT n, 'user' FROM ({numbers}) t WHERE n <= {users}"
        ),
        "INSERT INTO orm_perf_posts (id, user_id, title) SELECT id, id, 'post' FROM orm_perf_users"
            .to_owned(),
        "INSERT INTO orm_perf_comments (post_id, body) SELECT id, 'comment' FROM orm_perf_posts"
            .to_owned(),
    ];
    for sql in statements {
        db.statement(&sql, []).await.unwrap();
    }
}

/// One query per relation, also for more parents than MySQL's 65,535
/// placeholders per statement. One test: both parts share the tables.
#[tokio::test]
async fn eager_loading() {
    for db in databases().await {
        seed(&db, 50).await;

        // Same count whatever the number of parents.
        for limit in [1, 50] {
            let query = User::query().with("posts.comments").limit(limit);
            let (users, queries) = queries_on("orm_perf_", query.get(&db)).await;
            let users = users.unwrap();
            assert_eq!(users.len() as u64, limit);
            assert!(users.iter().all(|u| u.posts.loaded().unwrap().len() == 1));
            assert_eq!(queries, 3, "{:?}: users, posts, comments", db.dialect());
        }

        let (posts, queries) = queries_on("orm_perf_", Post::query().with("user").get(&db)).await;
        assert!(
            posts
                .unwrap()
                .iter()
                .all(|p| p.user.loaded().unwrap().is_some())
        );
        assert_eq!(queries, 2, "{:?}: posts, users", db.dialect());

        // Loading into models fetched earlier: one query per relation too.
        let mut users = User::all(&db).await.unwrap();
        let (loaded, queries) = queries_on("orm_perf_", users.load(&db, "posts.comments")).await;
        loaded.unwrap();
        assert!(users.iter().all(|u| u.posts.loaded().unwrap().len() == 1));
        assert_eq!(queries, 2, "{:?}: posts, comments", db.dialect());

        const USERS: u32 = 70_000;
        seed(&db, USERS).await;
        let users = User::query()
            .with("posts")
            .get(&db)
            .await
            .unwrap_or_else(|e| panic!("{:?}: {e}", db.dialect()));
        assert_eq!(users.len(), USERS as usize);
        assert!(users.iter().all(|u| u.posts.loaded().unwrap().len() == 1));
    }
}

/// Inserts and upserts with more bindings than one statement may have.
#[tokio::test]
async fn large_inserts() {
    const ROWS: i64 = 70_000;
    for db in databases().await {
        create_table(&db, "orm_large_items", &[("name", TEXT.0, TEXT.1)]).await;
        let items = QueryBuilder::table("orm_large_items");
        let rows = |name: &str| {
            (1..=ROWS)
                .map(|id| vec![Value::from(id), Value::from(name)])
                .collect::<Vec<_>>()
        };

        let inserted = items.insert(&db, &["id", "name"], rows("a")).await;
        let inserted = inserted.unwrap_or_else(|e| panic!("{:?}: {e}", db.dialect()));
        assert_eq!(inserted.rows_affected, ROWS as u64);

        let upsert = items.upsert(&db, &["id", "name"], rows("b"), &["id"], &["name"]);
        upsert
            .await
            .unwrap_or_else(|e| panic!("{:?}: {e}", db.dialect()));
        let renamed = items
            .clone()
            .where_("name", "=", "b")
            .count(&db)
            .await
            .unwrap();
        assert_eq!(renamed, ROWS as u64, "{:?}", db.dialect());
    }
}
