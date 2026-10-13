# Rustique documentation

An Eloquent-style ORM for Rust, targeting **MySQL 8.4+** and **Oracle 19c+**. One API, two SQL dialects. Async, on tokio.

The pages follow the layout of the Laravel Eloquent docs, so if you know Laravel you will know where to look.

| Page | What it covers |
|---|---|
| [Getting started](getting-started.md) | Install, features, connecting (pool, TLS, session setup, lazy connection), the global `Db`, a first model |
| [Query builder](query-builder.md) | `QueryBuilder`: selects, wheres (dates, JSON), `when`, joins, unions, ordering, aggregates, `value`, insert / update / delete / upsert / increment |
| [Models](models.md) | `#[rustique::model]`, renamed columns, UUID / ULID keys, retrieving (`find_many`, `sole`), saving, `first_or_create` / `update_or_create`, `increment`, `refresh`, deleting, timestamps, soft deletes, dirty tracking, mass assignment (`Fill`), bulk insert / upsert, pruning |
| [Relationships](relationships.md) | `has_one`, `has_many`, `belongs_to`, `belongs_to_many`, `has_many_through`, `has_one_through`, polymorphic (`morph_to_many` too), eager loading and its constraints, N+1 detection, `with_count` / `with_sum` / `with_exists`, `where_has` / `where_has_morph`, pivots (`sync_without_detaching`, `toggle`) and pivot columns |
| [Casts](casts.md) | `#[derive(rustique::Cast)]`, `FromValue`, `Value`, custom types |
| [Scopes, hooks and observers](events.md) | `#[rustique::scopes]`, global scopes, `Hooks`, `Observer` |
| [Pagination, chunking and cursors](large-results.md) | `paginate`, `simple_paginate`, `cursor_paginate`, `chunk`, `chunk_by_id`, `cursor` |
| [Transactions and raw SQL](database.md) | read replicas, `begin`, `transaction(\|tx\| ...)`, deadlock retries, nested transactions, `rollback_after` for tests, savepoints, `select` / `statement`, errors, query logging |
| [Factories](factories.md) | `HasFactory`, `factory()`, `count`, `state`, `sequence`, `make` / `create`, `has` / `has_attached`, `for_` / `for_factory`, `recycle`, `after_making` / `after_creating` |
| [Schema and migrations](migrations.md) | `Schema`, `Blueprint`, `change()`, `use_current`, comments, `Migrator` (`fresh`, `reset`, `--step`), seeders, `migrations!`, the CLI, migrations from a schema diff (`make:migration --diff`), the compile-time schema check |
| [Database differences](dialects.md) | What changes between MySQL and Oracle |

## Cheat sheet

For people in a hurry and for coding agents. Every terminal method is `async` and takes `&impl Executor` (a `&Db` or a `&Transaction`) as its first argument.

```rust
use rustique::prelude::*;
use chrono::NaiveDateTime;

#[rustique::model(table = "users", timestamps, soft_deletes)]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,                       // `id` is the primary key unless a field has #[rustique(primary_key)]
    pub name: String,
    pub email: String,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
    pub deleted_at: Option<NaiveDateTime>,
    #[rustique(has_many(Post))]
    pub posts: Rel<Vec<Post>>,
}
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64, #[rustique(has_many(Comment))] pub comments: Rel<Vec<Comment>> }
# #[rustique::model(table = "comments")]
# #[derive(Debug, Clone, Default)]
# pub struct Comment { pub id: i64, pub post_id: i64 }
# async fn run() -> rustique::Result<()> {

let db = Db::connect("mysql://user:pass@localhost/app").await?;

let user = User::find(&db, 1).await?;                 // Option<User>
let user = User::find_or_fail(&db, 1).await?;         // Err(Error::NotFound) when missing
let users = User::query()
    .where_(User::EMAIL, "like", "%@example.com")
    .order_by_desc(User::ID)
    .with("posts.comments")                            // eager load, one query per level
    .get(&db).await?;                                  // Collection<User>

let mut user = User::create(&db, User { name: "Ann".into(), email: "a@x.io".into(), ..Default::default() }).await?;
user.name = "Anne".into();
user.save(&db).await?;                                 // UPDATE of dirty columns only
user.delete(&db).await?;                               // soft delete (sets deleted_at)

let posts = user.posts().where_("published", "=", true).get(&db).await?; // lazy relation query
let users = User::query().with_where("posts", |q| q.where_("published", "=", true)).get(&db).await?;
let users = User::query().with("posts:id,title").get(&db).await?;   // only those columns of posts
let page = User::query().cursor_paginate(&db, 50, None).await?;  // page.next_cursor for the next one
let ann = User::first_or_create(&db, vec![(User::EMAIL, "a@x.io".into())], vec![]).await?;
for post in user.posts.loaded()? { /* after with("posts") or user.load(&db, "posts") */ }

let tx = db.begin().await?;
user.force_delete(&tx).await?;
tx.commit().await?;                                    // dropped without commit = rollback
# Ok(()) }
```

Rules worth knowing before you write code:

1. Build models with `..Default::default()`. The macro adds a hidden `__meta` field.
2. Relations never load on field access. Use `with(...)`, `model.load(&db, "rel")`, or the lazy `model.rel()` query.
3. Column constants exist for every column: `User::EMAIL == "email"`. Plain strings work too.
4. Hooks and observers run on model methods (`save`, `delete`, ...), never on bulk query methods (`Query::update`, `Query::delete`, `Model::upsert`).
5. Oracle identifiers are uppercased and quoted. Write lowercase names in Rust.
6. Turn on `rustique::prevent_lazy_loading(rustique::LazyLoading::Deny)` in development to catch N+1 loops.
7. Timestamps are UTC unless `rustique::set_timezone(...)` sets the application time zone (Laravel's `app.timezone`) at startup.
8. Schema changes: edit `rustique-schema.txt` (written by `schema:snapshot`), then `make:migration <name> --diff` writes the migration from the database to it ([details](migrations.md#migrations-from-a-schema-diff)).
