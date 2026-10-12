<div align="center">

<img src="https://raw.githubusercontent.com/Eworkssk/rustique/main/assets/banner.svg" alt="Rustique: Eloquent-style ORM for Rust" width="640">

<br>

[![Rust](https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust&style=flat-square)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/edition-2024-C2410C?style=flat-square)](https://doc.rust-lang.org/edition-guide/)
[![MySQL](https://img.shields.io/badge/MySQL-8.4%2B-4479A1?logo=mysql&logoColor=white&style=flat-square)](https://dev.mysql.com)
[![Oracle](https://img.shields.io/badge/Oracle-19c%2B-F80000?logo=oracle&logoColor=white&style=flat-square)](https://www.oracle.com/database/)
[![Async](https://img.shields.io/badge/async-tokio-8A2BE2?style=flat-square)](https://tokio.rs)
[![unsafe](https://img.shields.io/badge/unsafe-forbidden-success?style=flat-square)](https://doc.rust-lang.org/nomicon/meet-safe-and-unsafe.html)

**Active-record models, a fluent query builder, relations, migrations and factories.**
**One API, two SQL dialects.**

[Documentation](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/README.md) ·
[Getting started](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/getting-started.md) ·
[Cheat sheet](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/README.md#cheat-sheet) ·
[Design](https://github.com/Eworkssk/rustique/blob/main/PLAN.md)

</div>

---

If you know Laravel Eloquent, you already know this library. **Rustique** brings the same
model-centric workflow to Rust, with plain structs, compile-time column constants and no
runtime magic.

```rust
use rustique::prelude::*;

#[rustique::model(table = "users", timestamps, soft_deletes)]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub name: String,
    pub email: String,
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
    pub deleted_at: Option<NaiveDateTime>,
    #[rustique(has_many(Post))]
    pub posts: Rel<Vec<Post>>,
}

let db = Db::connect("mysql://user:pass@localhost/app").await?;

// Query, filter, eager load
let users = User::query()
    .where_(User::EMAIL, "like", "%@example.com")
    .order_by_desc(User::ID)
    .with("posts.comments")          // one query per level, no N+1
    .get(&db).await?;

// Create, update (dirty columns only), soft delete
let mut user = User::create(&db, User { name: "Ann".into(), ..Default::default() }).await?;
user.name = "Anne".into();
user.save(&db).await?;
user.delete(&db).await?;

// Transactions roll back on error or drop
db.transaction(|tx| Box::pin(async move {
    user.save(tx).await?;
    order.save(tx).await?;
    Ok::<_, rustique::Error>(order.id)
})).await?;
```

## Highlights

|  |  |
|---|---|
| **Active-record models** | `#[rustique::model]` derives the whole thing. Dirty tracking, timestamps, soft deletes, UUID / ULID keys, mass assignment. |
| **Fluent query builder** | Wheres (dates, JSON, subqueries), joins, unions, aggregates, upsert, bulk insert. Every column gets a constant, so typos fail to compile. |
| **Relationships** | `has_one`, `has_many`, `belongs_to`, `belongs_to_many`, through, polymorphic and `morph_to_many`. Eager loading, `with_count`, `where_has`, pivots. |
| **Two databases, one API** | MySQL 8.4+ (pure Rust driver, rustls) and Oracle 19c+. The grammar handles quoting, limits, identity columns and JSON for you. |
| **Migrations and schema builder** | `Schema::create`, `Blueprint`, a `Migrator` with `fresh` / `reset` / `--step`, seeders and a ready-made CLI. |
| **Scopes, hooks, observers** | Local and global scopes, lifecycle hooks (`creating`, `saved`, `deleting`, ...) and observers. |
| **Large results** | `paginate`, `simple_paginate`, `cursor_paginate`, `chunk`, `chunk_by_id` and streaming cursors. |
| **Factories** | `User::factory().count(10).create(&db)` for tests and seeders. |
| **Casts** | `#[derive(rustique::Cast)]` maps JSON and custom types to columns. |
| **Safe by default** | `unsafe` is forbidden. N+1 detection (`prevent_lazy_loading`), deadlock retries, savepoints, query logging through `tracing`. |

## Install

Published on crates.io as `rustique-orm`; the library is imported as `rustique`:

```toml
[dependencies]
rustique-orm = "0.1"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
chrono = "0.4"
```

MySQL only? Skip the Oracle client entirely:

```toml
rustique-orm = { version = "0.1", default-features = false, features = ["mysql", "mysql-tls"] }
```

| Feature | Default | What it does |
|---|:---:|---|
| `mysql` | yes | MySQL driver (`mysql_async`, pure Rust) |
| `mysql-tls` | yes | TLS through rustls (`ring` provider; `mysql-tls-aws-lc-rs` for `aws-lc-rs`) |
| `oracle` | yes | Oracle driver. Needs Oracle Instant Client at **runtime** only |
| `schema-check` | no | Checks model structs against a schema snapshot at compile time |

Requires Rust 1.88 or newer.

## More taste

**Relations and constrained eager loading**

```rust
let users = User::query()
    .with_where("posts", |q| q.where_("published", "=", true))
    .with("posts:id,title")
    .get(&db).await?;

let drafts = user.posts().where_("published", "=", false).get(&db).await?;
```

**Migrations**

```rust
Schema::create(&db, "users", |t| {
    t.id();
    t.string("email", 255).unique();
    t.boolean("active").default(true);
    t.foreign_id("country_id").constrained("countries").cascade_on_delete();
    t.json("settings").nullable();
    t.timestamps();
    t.soft_deletes();
}).await?;
```

**Pagination without the offset cost**

```rust
let page = User::query().cursor_paginate(&db, 50, None).await?;
// page.next_cursor for the next one
```

**Factories**

```rust
impl HasFactory for User {
    fn definition(n: u64) -> Self {
        User { name: format!("User {n}"), email: format!("user{n}@example.com"), ..Default::default() }
    }
}
```

## Documentation

The pages follow the Laravel Eloquent docs layout. Every code block compiles as a doctest.

| | |
|---|---|
| [Getting started](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/getting-started.md) | Install, features, connecting, pool and TLS |
| [Query builder](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/query-builder.md) | Selects, wheres, joins, aggregates, writes |
| [Models](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/models.md) | Retrieving, saving, timestamps, soft deletes, dirty tracking |
| [Relationships](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/relationships.md) | All relation kinds, eager loading, pivots |
| [Casts](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/casts.md) | `Cast`, `FromValue`, custom types |
| [Scopes, hooks and observers](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/events.md) | Reusable filters and lifecycle events |
| [Pagination, chunking and cursors](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/large-results.md) | Working with big tables |
| [Transactions and raw SQL](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/database.md) | Replicas, nested transactions, retries, logging |
| [Factories](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/factories.md) | Test data and seeding |
| [Schema and migrations](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/migrations.md) | Schema builder, `Migrator`, CLI |
| [Database differences](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/dialects.md) | What changes between MySQL and Oracle |
| [Eloquent compatibility](https://github.com/Eworkssk/rustique/blob/main/rustique/docs/eloquent-compatibility.md) | What maps to Laravel, what does not |

## Development

Run the database-free tests (they check generated SQL for both dialects):

```bash
cargo test --workspace --all-features
```

Integration tests need real databases. Start them and point the tests at them:

```bash
docker compose up -d
export RUSTIQUE_TEST_MYSQL_URL=mysql://orm:secret@127.0.0.1:13306/orm_test
export RUSTIQUE_TEST_ORACLE_URL=oracle://orm:secret@127.0.0.1:11521/XEPDB1
cargo test --workspace --all-features
```

Before sending a change:

```bash
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
```

Layout: [`rustique/`](https://github.com/Eworkssk/rustique/blob/main/rustique) is the library, [`rustique-macros/`](https://github.com/Eworkssk/rustique/blob/main/rustique-macros) the proc macros,
[`rustique/docs/`](https://github.com/Eworkssk/rustique/blob/main/rustique/docs) the user guide, and [`PLAN.md`](https://github.com/Eworkssk/rustique/blob/main/PLAN.md) the design and the decisions behind it.

## Status

Pre-release. The API covers the full model, relation, migration and query surface described above and is
tested against MySQL 8.4 and Oracle XE 21c. See [`PLAN.md`](https://github.com/Eworkssk/rustique/blob/main/PLAN.md#11-open-items) for what is still open.

## License

Licensed under either of [Apache License, Version 2.0](https://github.com/Eworkssk/rustique/blob/main/LICENSE-APACHE) or [MIT license](https://github.com/Eworkssk/rustique/blob/main/LICENSE-MIT) at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in this crate
by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
