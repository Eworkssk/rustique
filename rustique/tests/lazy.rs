//! `prevent_lazy_loading` changes process-wide state, so it gets its own
//! test binary. Runs on the databases of `RUSTIQUE_TEST_*_URL`, like `tests/db`.

use rustique::prelude::*;
use rustique::{LazyLoading, prevent_lazy_loading};

#[rustique::model(table = "orm_lazy_users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    #[rustique(has_many(Post))]
    posts: Rel<Vec<Post>>,
}

#[rustique::model(table = "orm_lazy_posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    user_id: i64,
}

#[tokio::test]
async fn lazy_loading_on_listed_models_is_flagged() {
    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::connect(&url).await.unwrap();
        for table in ["orm_lazy_posts", "orm_lazy_users"] {
            Schema::drop_if_exists(&db, table).await.unwrap();
        }
        Schema::create(&db, "orm_lazy_users", |t| {
            t.id();
        })
        .await
        .unwrap();
        Schema::create(&db, "orm_lazy_posts", |t| {
            t.id();
            t.big_integer("user_id");
        })
        .await
        .unwrap();
        let mut users = vec![User::default(), User::default()];
        User::insert_many(&db, &mut users).await.unwrap();

        prevent_lazy_loading(LazyLoading::Deny);
        let listed = User::query().get(&db).await.unwrap();
        let error = listed[0].posts().get(&db).await.unwrap_err();
        assert!(
            error.to_string().contains("lazy loading `posts`"),
            "{error}"
        );
        // Alone, eager-loaded or explicitly loaded: fine.
        let alone = User::find(&db, users[0].id).await.unwrap().unwrap();
        alone.posts().get(&db).await.unwrap();
        let mut eager = User::query().with("posts").get(&db).await.unwrap();
        assert!(eager[0].posts.is_loaded());
        eager[1].load(&db, "posts").await.unwrap();

        prevent_lazy_loading(LazyLoading::Warn);
        listed[0].posts().get(&db).await.unwrap();
        prevent_lazy_loading(LazyLoading::Allow);
        for table in ["orm_lazy_posts", "orm_lazy_users"] {
            Schema::drop(&db, table).await.unwrap();
        }
    }
}
