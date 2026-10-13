//! Code that passes the compile-time schema check against
//! `rustique-schema.fixture.txt` (with `--features schema-check`); the failures
//! are `compile_fail` doctests in `src/lib.rs`.

use rustique::{Model, Query, Rel};

#[rustique::model(table = "users", schema = "fixture")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    email: String,
    nickname: Option<String>,
    #[rustique(has_many(Post, foreign_key = "author_id", local_key = "id"))]
    posts: Rel<Vec<Post>>,
    #[rustique(belongs_to_many(Role, foreign_pivot_key = "user_id", pivot = ["level"]))]
    roles: Rel<Vec<Role>>,
    // The guessed `articles` lacks `writer_id`; the foreign key leads to `blog_entries`.
    #[rustique(has_many(Article, foreign_key = "writer_id"))]
    articles: Rel<Vec<Article>>,
}

#[rustique::model(table = "blog_entries", schema = "fixture")]
#[derive(Debug, Clone, Default)]
struct Article {
    id: i64,
    writer_id: i64,
}

// The snapshot name comes from the `fixture.` prefix.
#[rustique::model(table = "fixture.posts")]
#[derive(Debug, Clone, Default)]
struct Post {
    id: i64,
    author_id: i64,
    title: String,
    #[rustique(belongs_to(User, foreign_key = "author_id", scope = |q| q.order_by("users.email")))]
    author: Rel<Option<User>>,
}

#[rustique::model(table = "roles", schema = "fixture")]
#[derive(Debug, Clone, Default)]
struct Role {
    id: i64,
}

#[rustique::scopes(schema = "fixture")]
impl User {
    fn scope_named(q: Query<User>) -> Query<User> {
        q.where_not_null("users.nickname")
            .select(["users.id", "users.email"])
            // Lone names and expressions are not checked here.
            .order_by("anything")
    }
}

#[test]
fn checked_strings_expand_to_themselves() {
    assert_eq!(
        rustique::col!(schema = "fixture", "users.email"),
        "users.email"
    );
    assert_eq!(rustique::col!(schema = "fixture", "title"), "title");
    assert_eq!(rustique::col!(schema = "fixture", "posts.*"), "posts.*");
    const SQL: &str = rustique::sql!(
        schema = "fixture",
        "SELECT users.email, p.title FROM users JOIN posts p ON p.author_id = users.id \
         WHERE EXTRACT(YEAR FROM p.created) = ?"
    );
    assert!(SQL.starts_with("SELECT users.email"));
    // Without a snapshot of that name nothing is checked.
    assert_eq!(rustique::col!(schema = "missing", "a.b"), "a.b");
    assert_eq!(
        rustique::sql!("SELECT 1 FROM nowhere"),
        "SELECT 1 FROM nowhere"
    );
    assert_eq!(User::TABLE, "users");
    assert_eq!(Post::TABLE, "fixture.posts");
    assert_eq!(Role::TABLE, "roles");
    assert_eq!(Article::TABLE, "blog_entries");
    let _ = User::named();
}
