# Transactions and raw SQL

## Executors

Every terminal method takes `&impl Executor`. Two types implement it:

- `Db`: the pool. Each statement borrows a connection.
- `Transaction`: one dedicated connection inside `BEGIN ... COMMIT`.

`AnyExecutor` wraps either one (`AnyExecutor::from(&db)`) where a stored closure needs a single type, such as factory callbacks (see [Factories](factories.md)).

So the same code runs inside or outside a transaction:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
async fn close_account(db: &impl Executor, user: &mut User) -> rustique::Result<()> {
    user.delete(db).await?;
    Post::query().where_("user_id", "=", user.id).delete(db).await?;
    Ok(())
}
```

## Read replicas

Like the `read` / `write` hosts of a Laravel connection, add replicas with `read_url`:

```rust
# use rustique::prelude::*;
# async fn run() -> rustique::Result<()> {
let db = Db::builder("mysql://app:pw@primary/app")
    .read_url("mysql://app:pw@replica-1/app")
    .read_url("mysql://app:pw@replica-2/app")      // used in turn
    .connect()
    .await?;

let users = QueryBuilder::table("users").get(&db).await?;          // a replica
let fresh = QueryBuilder::table("users").get(&db.write()).await?;  // the primary
# Ok(()) }
```

- Plain `SELECT`s go to a replica. Writes, `FOR UPDATE` / `FOR SHARE` / `LOCK IN SHARE MODE` reads (line breaks included), other raw statements (`WITH ...`, `CALL ...`), transactions, schema checks, migrations and `create_or_first`'s lookup after a unique violation use the primary.
- A replica may lag behind: read through `db.write()` (Laravel's `useWritePdo`) or inside a transaction when you must see a write made just before.
- Each replica gets its own pool with the builder's settings, and must use the same driver.

### Sticky reads

Laravel's `sticky` option: `.sticky(true)` (off by default) sends a request's reads to the primary once that request wrote. A `Db` is shared across requests, so the request is a scope you mark with `Db::sticky_scope`, for example around each handler in a middleware:

```rust
# use rustique::prelude::*;
# async fn run() -> rustique::Result<()> {
let db = Db::builder("mysql://app:pw@primary/app")
    .read_url("mysql://app:pw@replica-1/app")
    .sticky(true)
    .connect()
    .await?;

Db::sticky_scope(async {
    QueryBuilder::table("users").get(&db).await?;   // a replica
    db.statement("UPDATE users SET active = 1 WHERE id = ?", [1.into()]).await?;
    QueryBuilder::table("users").get(&db).await?;   // the primary, until the scope ends
    Ok::<_, rustique::Error>(())
})
.await?;
# Ok(()) }
```

- A write is any statement or transaction on the sticky `Db` (or its clones) inside the scope; a nested scope shares its outer one.
- Outside a scope, `sticky` does nothing.
- The scope lives in the current tokio task: work moved to `tokio::spawn` inside it does not share it (wrap that work in its own `Db::sticky_scope`). One write pins the reads of every sticky `Db` in the scope.

## Transactions

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
let tx = db.begin().await?;
user.save(&tx).await?;
order.save(&tx).await?;
tx.commit().await?;          // or tx.rollback().await?
# Ok(()) }
```

Rust has no async drop: a `Transaction` dropped without `commit` (an early `?` return, a panic) is rolled back before its connection goes back to the pool.

### Closure style

`db.transaction` commits when the closure returns `Ok` and rolls back on `Err` or a panic, like Laravel's `DB::transaction`:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
let order_id = db
    .transaction(async |tx| {
        user.save(tx).await?;
        order.save(tx).await?;
        Ok::<_, rustique::Error>(order.id)
    })
    .await?;
# Ok(()) }
```

- Pass an async closure: `async |tx| { ... }`. The call is `Send` whenever the closure's body is, so it works in `tokio::spawn` and web handlers.
- The error type is yours: any `E: From<rustique::Error>`. Return `Err(AppError::OutOfStock)` to roll back on a business rule. Name the type once (`Ok::<_, rustique::Error>(..)` or a typed `let`) since `?` alone cannot infer it.
- The closure runs once. For retries, see below.

### Deadlock retries

`transaction_with_retries` is Laravel's `DB::transaction($fn, $attempts)`: when the closure (or the commit) fails with a deadlock, lock wait timeout or serialization failure, the transaction is rolled back and the closure runs again, up to `attempts` times in all. Other errors return at once.

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
db.transaction_with_retries(3, async |tx| {
    tx.statement("UPDATE stock SET qty = qty - 1 WHERE sku = ?", ["A1".into()]).await?;
    Ok::<_, rustique::Error>(())
})
.await?;
# Ok(()) }
```

The closure is `AsyncFnMut`, so it can run again: it may borrow or mutate what it captures, but not move it out. Inside `tokio::spawn` or a web handler, write `async move |tx| { ... }` so the closure owns its captures; a borrowing retry closure is not `Send` (a current rustc limit). `rustique::Error::is_concurrency_error()` names the errors that are retried (MySQL 1213 / 1205, ORA-00060 / ORA-08177). With your own error type, implement `rustique::TransactionError` for it.

### Nested transactions

`tx.transaction(...)` runs a closure behind a savepoint, like a nested `DB::transaction`: an `Err` undoes only the closure's work, and the outer transaction carries on.

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
db.transaction(async |tx| {
    tx.statement("INSERT INTO orders (total) VALUES (?)", [10.into()]).await?;
    let coupon = tx
        .transaction(async |tx| {
            tx.statement("INSERT INTO coupons (code) VALUES (?)", ["X".into()]).await
        })
        .await;
    if coupon.is_err() { /* the order stays */ }
    Ok::<_, rustique::Error>(())
})
.await?;
# Ok(()) }
```

### Rolling back after tests

`db.rollback_after(async |tx| ...)` runs the closure in a transaction that is always rolled back, and returns the closure's result. Run a test's queries on `tx` and the database is left as it was:

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let count = db
    .rollback_after(async |tx| {
        tx.statement("DELETE FROM users", []).await?;
        QueryBuilder::table("users").count(tx).await
    })
    .await?;
assert_eq!(count, 0);   // and the users are still there
# Ok(()) }
```

DDL commits implicitly, so schema changes inside the closure stay.

### Savepoints

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn risky(tx: &rustique::Transaction) -> rustique::Result<()> { Ok(()) }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
let tx = db.begin().await?;
let sp = tx.savepoint().await?;
if risky(&tx).await.is_err() {
    tx.rollback_to(&sp).await?;
} else {
    tx.release(sp).await?;
}
tx.commit().await?;
# Ok(()) }
```

DDL (`Schema::*`) commits implicitly on both MySQL and Oracle. Do not put it in a transaction and expect a rollback.

## Raw SQL

Like Laravel's `DB::select` / `DB::statement`. `?` marks bindings on both databases.

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
let rows = db.select("SELECT id, name FROM users WHERE id > ?", [10.into()]).await?;   // Vec<Row>
let id: i64 = rows[0].get("id")?;          // column lookup is case-insensitive

let done = db.statement("UPDATE users SET active = ? WHERE id = ?", [false.into(), 3.into()]).await?;
done.rows_affected;
# Ok(()) }
```

Never `format!` request data into raw SQL; pass it as a `?` binding.

With the `schema-check` feature, wrap a literal in `rustique::sql!("...")` to check its tables and `table.column` references against the schema snapshot at compile time: see the [compile-time schema check](migrations.md#compile-time-schema-check).

### Session state

`SET`, `LOCK TABLES`, temporary tables and `GET_LOCK` change the connection they run on, not the `Db`. Two calls on `Db` can borrow different pooled connections, so run them inside a [transaction](#transactions), which keeps one connection until it ends:

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let tx = db.begin().await?;
tx.statement("SET time_zone = '+02:00'", []).await?;
let rows = tx.select("SELECT NOW() AS now", []).await?; // same connection, sees +02:00
tx.commit().await?;
# Ok(()) }
```

Settings every connection needs belong in [`session_setup`](getting-started.md#pool-settings-and-tls) instead: it runs on each new connection and again after each MySQL reset, so the resets below never undo it.

### Session time zone

`.timezone(zone)` is a shortcut for the session time zone of every connection: `SET time_zone = '...'` on MySQL, `ALTER SESSION SET TIME_ZONE = '...'` on Oracle. It runs with `session_setup` (before its statements, so a `session_setup` statement setting the zone wins) on each new connection and after each reset or replaced session.

```rust
# use rustique::prelude::*;
# async fn run() -> rustique::Result<()> {
rustique::set_timezone(chrono::FixedOffset::east_opt(2 * 3600).unwrap()); // what the ORM writes
let db = Db::builder("mysql://app:pw@localhost/app")
    .timezone("+02:00")                                      // what the database's NOW() uses
    .session_setup(["SET SESSION max_execution_time = 25000"]) // runs too
    .connect()
    .await?;
# rustique::set_timezone(chrono::Utc);
# Ok(()) }
```

- The session zone drives `NOW()` / `CURRENT_TIMESTAMP` (Oracle's, not `SYSTIMESTAMP`) and MySQL `TIMESTAMP` column conversion. The date-times the ORM itself writes and reads follow `rustique::set_timezone`; set both to the same zone so the two agree.
- Named zones (`"Europe/Bratislava"`) follow daylight saving time. MySQL needs its time zone tables loaded for them (`mysql_tzinfo_to_sql`); fixed offsets (`"+02:00"`) always work.
- A zone with a quote or backslash is an `Error::Config`; an unknown zone fails the first query.

Session state never reaches later queries that borrow the same pooled connection:

- **MySQL**: a connection that ran raw SQL through `select` / `statement`, or a transaction, is reset when it returns to the pool. The reset also drops the connection's cached prepared statements, so its next queries prepare again. Query builder and model queries outside a transaction skip the reset and keep their prepared statements (about half the round trips per query).
- **Oracle** has no reset short of a new session (about 25 ms, against well under 1 ms for a pooled one). Raw SQL through `select` / `statement`, in a transaction or not, that does not start with `SELECT`, `WITH`, `INSERT`, `UPDATE`, `DELETE` or `MERGE` (`ALTER SESSION`, `SET ROLE`, PL/SQL blocks, `CALL`, ...) drops its session from the pool when the statement or transaction ends. Raw queries and DML keep their session, so a PL/SQL function called from them must not change package state or contexts, and rows in a global temporary table `ON COMMIT PRESERVE ROWS` outlive the request; delete them before it ends.

Raw fragments inside the builder (`select_raw("@x := 1")`) are not checked; keep session changes out of them.

When the application's raw SQL leaves no session state behind, for example PL/SQL procedures that only read and write tables, `Db::builder(url).keep_sessions_after_raw_sql()` skips the reset and the new Oracle session for that `Db`. MySQL transactions are still reset. Everything listed above as a leak is then on you again.

Raw fragments inside the builder: `select_raw`, `where_raw`, `having_raw`, `order_by_raw` (see [query builder](query-builder.md)). Use the [dialect](dialects.md) to branch when SQL differs:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
if db.dialect() == Dialect::Oracle { /* ... */ }
# Ok(()) }
```

Also on `Executor`: `fetch_all(Compiled)`, `execute(Compiled)`, `execute_all(Vec<Compiled>)` (all in one transaction or savepoint; on failure it returns the failing statement's error, also when a MySQL deadlock already rolled back the whole transaction), `cursor(Compiled)`.

### Oracle sequences

`next_sequence_values(sequence, count)` reserves `count` values of an Oracle sequence in one round trip (`SELECT "SEQ".NEXTVAL FROM DUAL CONNECT BY LEVEL <= :1`), for keys the application assigns before inserting, such as parent and child rows written together:

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let ids: Vec<i64> = db.next_sequence_values("orders_seq", 500).await?;
# Ok(()) }
```

- The values are unique, but not necessarily consecutive or ascending (sequence cache, RAC).
- It always runs on the main database, never a read replica. A schema-qualified name (`"app.orders_seq"`) works; `count` 0 returns no values without a query.
- MySQL has no sequences: `Error::Unsupported`.

## Errors

Everything returns `rustique::Result<T>` = `Result<T, rustique::Error>`:

| Variant | When |
|---|---|
| `NotFound(table)` | `find_or_fail`, `first_or_fail`, `sole` |
| `MultipleRecords(table)` | `sole` found more than one row |
| `Database { code, message }` | the server refused. `code` is native: MySQL `1062`, Oracle `1` (ORA-00001) for unique violations. `is_concurrency_error()` spots deadlocks, `is_lock_timeout()` a `NOWAIT` / `WAIT n` lock that gave up |
| `Decode { model, column, .. }` | a column does not fit the field type; `model` is the table when building a model (the message reads `` `users.age` ``) |
| `RelationNotLoaded` | `rel.loaded()` before loading |
| `UnknownRelation { .. }` | a `with` / `where_has` / `with_count` name that does not exist |
| `UnknownMorphType(String)` | `*_type` value matching no `MorphTo` variant |
| `InvalidQuery(String)` | bad operator, binding count mismatch, ... |
| `Unsupported { .. }` | feature missing on this database (`shared_lock` on Oracle) |
| `Config(String)` | bad URL, driver not compiled in, global `Db` set twice |
| `MigrationNotFound`, `ColumnNotFound` | as named |

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users", soft_deletes)]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub deleted_at: Option<chrono::NaiveDateTime> }
# #[rustique::model(table = "posts")]
# #[derive(Debug, Clone, Default)]
# pub struct Post { pub id: i64, pub user_id: i64 }
# async fn run(db: Db, mut user: User, mut order: Post) -> rustique::Result<()> {
match user.save(&db).await {
    Err(e) if e.is_unique_violation() => { /* duplicate: MySQL 1062, ORA-00001 */ }
    other => other?,
}
# Ok(()) }
```

## Query logging

Every statement goes through `tracing`, target `rustique::query`:

- `DEBUG`: SQL, duration, row count.
- `TRACE`: bindings.
- An `INFO` span `rustique.query` per statement with OpenTelemetry attributes, ready for `tracing-opentelemetry`:
  - `db.system`, `db.statement`, `otel.name` (the first SQL word), `otel.kind = "client"`.
  - `db.namespace` (MySQL database, Oracle service name), `server.address` and `server.port` of the pool the statement ran on (a replica's for reads sent there).
  - `db.operation.batch.size`: rows of an Oracle array-bound insert or upsert (only when 2 or more).
  - `db.response.returned_rows` for queries, `rustique.rows_affected` for statements (OpenTelemetry has no attribute for affected rows), once the statement succeeds. A cursor's span gets `db.response.returned_rows` when its rows run out, or, if it is dropped or fails early, the rows read from the database until then (read-ahead included).

```bash
RUST_LOG=rustique::query=debug cargo run
```

### Errors in OpenTelemetry

- A failing statement (including cursors, `BEGIN`, `COMMIT`, `ROLLBACK` and savepoints) sets `otel.status_code = "error"` on its `rustique.query` span. The status message holds only the database's error code, since database messages can quote row data.
- A value that does not fit its Rust type (`Error::Decode`), or a model field whose column the result lacks, emits an `ERROR` event on target `rustique::decode`. Models are built after the statement's span has ended, so the event lands in the caller's current span. Its fields:
  - `error`: the error's message, with column and type names (`` cannot decode column `users.age`: expected i64, found string ``), never the value.
  - `exception.type`: `rustique::Error::Decode` or `rustique::Error::ColumnNotFound`, which APMs group errors by.
  - `model`: the model's table, when one was being built.
  - `exception.stacktrace`: a backtrace cleaned up to your code and the ORM call it made. Runtime, standard library, `tracing` and driver frames are dropped, and each run of ORM frames keeps only its outermost one. It is captured even without `RUST_BACKTRACE`. File and line numbers need debug info (`debug = "line-tables-only"` in your release profile is enough); without it only function names remain.

  The event has no message and an `error` field, so `tracing-opentelemetry` exports it as an OpenTelemetry `exception` event (with `exception.message`), and any `ERROR` event marks the span failed.

The error is still returned as well. To silence the events, and skip the backtrace, use `RUST_LOG=rustique::decode=off`.
