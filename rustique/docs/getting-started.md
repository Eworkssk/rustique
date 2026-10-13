# Getting started

## Install

```toml
[dependencies]
rustique-orm = "0.1"   # imported as `rustique`
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
chrono = "0.4"                        # for date/time fields
```

Cargo features (the first three are on by default):

| Feature | Effect |
|---|---|
| `mysql` | MySQL driver (`mysql_async`, pure Rust) |
| `mysql-tls` | TLS for MySQL through rustls (`?require_ssl=true`, `rustique::Tls`) with the `ring` crypto provider |
| `oracle` | Oracle driver (`oracle` crate). Needs Oracle Instant Client at **runtime** only |
| `mysql-tls-aws-lc-rs` | Off by default. The same with the `aws-lc-rs` provider, instead of `mysql-tls` |
| `mysql-rustls` | Off by default. The same without a provider: the application compiles in or installs one |
| `schema-check` | Off by default. Checks model structs against a schema snapshot at compile time, see [Schema and migrations](migrations.md#compile-time-schema-check) |

rustls needs exactly one crypto provider. When another crate of the application enables a different one (reqwest 0.13 brings `aws-lc-rs`), the first TLS connection panics with "no process-level CryptoProvider available". Pick the provider the rest of the application uses:

```toml
rustique-orm = { version = "0.1", default-features = false, features = ["mysql", "mysql-tls-aws-lc-rs", "oracle"] }
```

or install one at startup with `rustls::crypto::CryptoProvider::install_default`.

MySQL-only apps can turn Oracle off and never install Instant Client:

```toml
rustique-orm = { version = "0.1", default-features = false, features = ["mysql", "mysql-tls"] }
```

## Connecting

```rust,no_run
use rustique::prelude::*;

#[tokio::main]
async fn main() -> rustique::Result<()> {
    let db = Db::connect("mysql://user:password@localhost:3306/app").await?;
    // or: Db::connect("oracle://user:password@host:1521/service_name").await?
    Ok(())
}
```

`Db` is a connection pool, cheap to clone. Pass `&db` to every query.

URL notes:

- **MySQL**: any option `mysql_async` accepts in the query string (`pool_max`, ...). `?require_ssl=true` for TLS with the system's root certificates. Pooled connections are reset only after transactions and raw `Db::select` / `Db::statement` calls (see [session state](database.md#session-state)); `?reset_connection=true` resets them after every query, which halved throughput in a local benchmark since every query then prepares its statement again.
- **Oracle**: `oracle://user:password@host:1521/service_name`, or an EZConnect Plus address such as `oracle://user:password@tcps://host:2484/service?wallet_location=/path` (TLS). Pool of 10 connections.
- Queries wait up to 30 s for a free pooled connection, then fail with `Error::Database`.
- Percent-encode special characters in user and password.

### Pool settings and TLS

`Db::builder` sets the pool in code; its settings win over the same URL option:

```rust,no_run
# use std::time::Duration;
# use rustique::prelude::*;
# async fn run(url: String) -> rustique::Result<()> {
let db = Db::builder(&url)
    .max_connections(20)                       // MySQL default 100, Oracle 10
    .min_connections(2)                        // idle connections kept open
    .acquire_timeout(Duration::from_secs(5))   // wait for a free connection, default 30 s
    .idle_timeout(Duration::from_secs(300))    // close idle ones beyond the minimum
    .max_lifetime(Duration::from_secs(3600))   // recycle old connections
    .session_setup(["SET SESSION max_execution_time = 25000"]) // on every new connection
    .tls(rustique::Tls::new()                       // MySQL only
        .root_cert("/etc/ssl/mysql/ca.pem")    // private or cloud CA, also trusted
        .client_identity("/etc/ssl/mysql/client-cert.pem", "/etc/ssl/mysql/client-key.pem"))
    .connect()
    .await?;
# Ok(()) }
```

- `Tls::new()` alone verifies the server against the system's root certificates. `Tls::danger_accept_invalid_certs()` skips verification, for a local server with a self-signed certificate only.
- Oracle TLS stays in the URL (`tcps://` plus a wallet); `.tls(...)` on an Oracle URL is an `Error::Config`.
- `.session_setup([...])` runs its SQL on every new connection before the first query: `SET SESSION ...` on MySQL, `ALTER SESSION SET ...` (NLS formats, time zone) on Oracle. MySQL runs it again after each connection reset, and Oracle on the new session that replaces a dropped one, so the settings hold for every query; see [session state](database.md#session-state). A failing statement fails the query that opened the connection.
- `.keep_sessions_after_raw_sql()` returns connections that ran raw SQL to the pool without a reset (MySQL) or a new session (Oracle, about 25 ms per PL/SQL call). Only for apps whose raw SQL leaves no session state; see [session state](database.md#session-state).

### Lazy connection

`connect()` opens the pool and checks that the database answers. `connect_lazy()` only checks the URL and the pool sizes and returns at once; the first query opens the first connection, and fails with `Error::Database` while the database is down. Use it when the application must start, and answer its health checks, without its database. Oracle loads Instant Client only at that first query too.

```rust,no_run
# use rustique::prelude::*;
# fn run(url: String) -> rustique::Result<()> {
let db = Db::builder(&url).connect_lazy()?;   // no I/O, not async
# Ok(()) }
```

### Timeouts and cancellation

Wrap any query in `tokio::time::timeout` to bound it. What dropping the query future does:

- **Oracle**: the call is interrupted on the server (OCIBreak, the statement ends with ORA-01013). A transaction stays usable; a pooled connection is replaced. PL/SQL `DBMS_SESSION.SLEEP` ignores the interrupt.
- **MySQL**: a statement on a pooled connection is stopped with `KILL QUERY`, sent from a short-lived connection of its own; its connection goes back to the pool once the kill has landed. A statement in a transaction is not killed: the server finishes it, and its connection is busy until then (a dropped cursor stops as described in [large results](large-results.md)).

The Oracle driver is blocking and runs on tokio's blocking threads. Queries wait for a free connection without holding a thread, so one pool uses at most `max_connections` of them.

The ORM needs a tokio runtime (`mysql_async` does too).

### The global `Db`

Like Laravel's `DB` facade, you can register one process-wide pool:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64 }
# async fn run(url: String) -> rustique::Result<()> {
Db::connect(&url).await?.set_global()?;

let users = User::all(Db::global()).await?;
# Ok(()) }
```

`Db::global()` panics if `set_global` was never called. `set_global` errors if called twice.

### Closing

`db.disconnect().await?` drains the pool. Optional; dropping works too.

## A first model

```rust
use rustique::prelude::*;

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub name: String,
}
# async fn run(db: Db) -> rustique::Result<()> {

let ann = User::create(&db, User { name: "Ann".into(), ..Default::default() }).await?;
println!("{}", ann.id);            // filled from the database

let everyone = User::all(&db).await?;
# Ok(()) }
```

Next: [Models](models.md) or [Query builder](query-builder.md).

## Running the crate's own tests

SQL tests need no database. Integration tests run only when the URLs are set:

```bash
docker compose up -d
```

```bash
RUSTIQUE_TEST_MYSQL_URL=mysql://orm:secret@127.0.0.1:13306/orm_test RUSTIQUE_TEST_ORACLE_URL=oracle://orm:secret@127.0.0.1:11521/XEPDB1 cargo test
```
