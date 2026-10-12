# Rustique — Plan

An Eloquent-style ORM for Rust targeting **MySQL 8.4+** and **Oracle 19c+**.

Status: phases 1–12 done (query builder, SQL for both dialects, drivers, transactions, models, relations, schema builder, migrations, scopes, hooks, pagination, chunking, upsert; polymorphic relations, cursors, observers, CLI commands, query logging; two Eloquent parity passes, a bug hunt against real queries and the compile-time schema check). Section 9 lists the confirmed decisions, section 11 what is still open.

---

## 1. Goals and non-goals

**Goals (Eloquent parity where it makes sense in Rust)**

- Active-record models: `User::find`, `user.save()`, `user.delete()`, `User::create`.
- Fluent query builder: `where_`, `or_where`, `where_in`, `where_null`, `join`, `order_by`, `group_by`, `having`, `limit`/`offset`, `count`/`sum`/`max`, `exists`, `pluck`, `first`, `first_or_fail`, `paginate`, `chunk_by_id`.
- Relationships: `has_one`, `has_many`, `belongs_to`, `belongs_to_many` (pivot), `has_many_through`. Lazy loading through methods, eager loading through `with("posts.comments")`.
- Timestamps (`created_at`, `updated_at`), soft deletes (`deleted_at`, `with_trashed`, `only_trashed`, `restore`, `force_delete`).
- Local scopes, a soft-delete global scope, model lifecycle hooks (`creating`, `created`, `updating`, `saved`, `deleting`, ...).
- Dirty tracking: `save()` updates only changed columns; `is_dirty`, `get_original`.
- Transactions, raw SQL escape hatch, upsert, bulk insert.
- Schema builder + migrations (`Schema::create("users", |t| { t.id(); t.string("email").unique(); t.timestamps(); })`).

**Non-goals for v1**

- `fillable` / `guarded` / `hidden`: Rust types and serde already cover these.
- Accessors / mutators as magic: plain Rust methods and the `FromValue`/`ToValue` traits replace them.
- Polymorphic many-to-many (`morph_to_many` / `morphed_by_many`).
- Databases other than MySQL and Oracle. Compile-time checked SQL (sqlx-style); model structs are checked against a schema snapshot instead (phase 12).

---

## 2. Target API

```rust
use rustique::prelude::*;

#[rustique::model(table = "users", timestamps, soft_deletes)]
#[derive(Debug, Clone, Default)]
pub struct User {
    #[rustique(primary_key)]
    pub id: i64,
    pub name: String,
    pub email: String,
    pub settings: Settings, // #[derive(rustique::Cast)] #[cast(json)] struct Settings
    pub created_at: Option<NaiveDateTime>,
    pub updated_at: Option<NaiveDateTime>,
    pub deleted_at: Option<NaiveDateTime>,

    #[rustique(has_many(Post, foreign_key = "user_id"))]
    pub posts: Rel<Vec<Post>>,
}

#[rustique::scopes]
impl User {
    fn scope_active(q: Query<User>) -> Query<User> {
        q.where_(User::EMAIL_VERIFIED_AT, "is not", Null)
    }
}

let db = Db::connect("oracle://app:pw@host:1521/ORCLPDB1").await?;

// Query builder, typed column constants or plain strings
let users = User::query()
    .active()
    .where_(User::EMAIL, "like", "%@example.com")
    .where_in("id", [1, 2, 3])
    .with("posts.comments")
    .order_by_desc(User::CREATED_AT)
    .limit(10)
    .get(&db).await?;                       // Vec<User>

// Active record
let mut user = User { name: "Ann".into(), email: "a@x.io".into(), ..Default::default() };
user.save(&db).await?;                      // INSERT, sets id, created_at, updated_at
user.name = "Anna".into();
user.save(&db).await?;                      // UPDATE users SET name = ?, updated_at = ? WHERE id = ?

// Lazy relation = a pre-filtered query
let recent = user.posts().where_("published", "=", true).get(&db).await?;

// Eager-loaded relation lives in the field
for post in user.posts.loaded()? { /* ... */ }

// Transactions
let tx = db.begin().await?;
user.delete(&tx).await?;                    // soft delete
tx.commit().await?;
```

Notes on the shape:

- `#[rustique::model]` is an **attribute macro**, not a derive, because it must inject a hidden `__meta: rustique::Meta` field (holds `exists: bool` and the original row for dirty tracking). Structs are then built with `..Default::default()`.
- `Rel<T>` is `NotLoaded | Loaded(T)`. Lazy loading through field access is impossible in Rust (it needs `async` + a connection), so lazy loading is always explicit: `user.posts().get(&db)`.
- Every terminal method takes `&impl Executor`, implemented by `Db` and `Transaction`.
- Column constants (`User::EMAIL`) give typo checks; strings stay allowed for Eloquent feel and joins.

---

## 3. Architecture

Two crates in one workspace. Add more only when compile times or users demand it.

```
rustique/
├─ rustique/src/             # runtime library
│  ├─ error.rs          # Error, Result
│  ├─ value/            # Value enum; encode.rs (From<T>), decode.rs (FromValue)
│  ├─ row.rs            # Row (case-insensitive column lookup, Oracle '' = NULL)
│  ├─ query/            # QueryBuilder: mod.rs, wheres.rs, join.rs, ast.rs, operator.rs
│  ├─ grammar/          # writer.rs, select.rs, conditions.rs, dml.rs + mysql.rs, oracle.rs
│  ├─ db/               # Db, Executor, transaction.rs, mysql.rs / oracle.rs drivers (cargo features)
│  ├─ model.rs          # Model trait, Meta, save/delete/find, hooks
│  ├─ relation/         # HasOne, HasMany, BelongsTo, BelongsToMany, HasManyThrough, eager loader
│  ├─ schema/           # Blueprint, schema grammar per dialect
│  └─ migrate.rs        # Migrator + migrations table
└─ rustique-macros/          # proc-macros: #[model], #[scopes], migration registration
```

**Backend dispatch:** `Db` is an `enum { MySql(..), Oracle(..) }` behind cargo features, not a `dyn Trait`. Two backends do not justify trait objects, and enum dispatch avoids boxed async futures.

**Pipeline:** builder → AST → `Grammar` (per dialect) → `(sql: String, binds: Vec<Value>)` → driver → `Vec<Row>` → `Model::from_row`.

The AST/grammar split is the core of dual-DB support: one builder, two compilers.

---

## 4. Drivers

| | MySQL 8.4+ | Oracle 19c+ |
|---|---|---|
| Crate | `mysql_async` (pure Rust, async, built-in pool) | `oracle` (rust-oracle, ODPI-C, sync, built-in session pool) |
| Async | native | `tokio::task::spawn_blocking` around each call |
| Native dep | none | Oracle Instant Client at **runtime** only (`libclntsh`) |

- Runtime: **tokio**.
- Alternatives considered: `sqlx` for MySQL (heavier, its macros go unused since we build SQL ourselves); `sibyl` for async Oracle (smaller community, less maintained than `oracle`).
- Feature flags `mysql` / `oracle` mean MySQL-only users never need Instant Client.

---

## 5. Dialect differences the grammar must handle

| Concern | MySQL 8.4 | Oracle 19c |
|---|---|---|
| Placeholders | `?` | `:1, :2, ...` |
| Identifier quoting | `` `users`.`email` `` | `"USERS"."EMAIL"` (uppercased, see below) |
| Limit / offset | `LIMIT n OFFSET m` | `OFFSET m ROWS FETCH NEXT n ROWS ONLY` |
| Insert id | driver `last_insert_id()` | `INSERT ... RETURNING "ID" INTO :out` |
| Auto-increment DDL | `BIGINT UNSIGNED AUTO_INCREMENT` | `NUMBER(19) GENERATED BY DEFAULT AS IDENTITY` |
| Boolean | `TINYINT(1)` | `NUMBER(1)` + `CHECK (x IN (0,1))` (no SQL `BOOLEAN` before 23ai) |
| Upsert | `INSERT ... AS new ON DUPLICATE KEY UPDATE c = new.c` (`VALUES()` is deprecated) | `MERGE INTO ... USING (SELECT ... FROM dual)` |
| Bulk insert | multi-row `VALUES (..),(..)` | array binding (`Batch` API); `INSERT ALL` repeats identity values |
| `IN (...)` list | no practical limit | **max 1000 items**: eager loader must chunk keys |
| `SELECT` without table | allowed | needs `FROM DUAL` |
| Empty string | `''` | `''` **is NULL** |
| Strings | `VARCHAR(n)`, `TEXT` | `VARCHAR2(n CHAR)` (4000 bytes unless `MAX_STRING_SIZE=EXTENDED`), `CLOB` |
| Date/time | `DATETIME(6)`, `TIMESTAMP` | `TIMESTAMP(6)` (avoid `DATE`, it drops fractions) |
| JSON | `JSON` | `CLOB CHECK (col IS JSON)` (native `JSON` type is 21c+) |
| `DROP TABLE IF EXISTS` | yes | no (23ai only): PL/SQL block catching ORA-00942 |
| DDL in transactions | implicit commit | implicit commit |
| Identifier max length | 64 | 128 |
| Locks | `FOR UPDATE [SKIP LOCKED]` | same |

**Oracle identifier case:** unquoted Oracle identifiers fold to uppercase. The Oracle grammar uppercases and quotes every identifier (`email` becomes `"EMAIL"`). This matches tables created without quotes, stays safe for reserved words like `"LEVEL"` or `"SIZE"`, and `Row` lookup is case-insensitive, so models keep lowercase field names.

**Oracle empty string:** decoding NULL into a non-`Option` `String` fails by default. See decision 3.

---

## 6. Key internals

- **`Value`**: `Null, Bool, I64, U64, F64, Decimal(rust_decimal), String, Bytes, Date, DateTime, Time, Json`. Dates use `chrono` and are stored as UTC, or in the zone set with `rustique::set_timezone` (phase 11).
- **`FromValue` / `From<T> for Value`**: implemented for primitives, `Option<T>`, `chrono`, `rust_decimal` and `serde_json::Value`. `#[derive(rustique::Cast)]` replaces Eloquent `$casts`: it implements both for unit enums (stored as snake_case text, `#[cast(rename = "...")]` per variant, or `#[cast(int)]` for the discriminant), one-field newtypes, and any serde type with `#[cast(json)]`. Other casts are hand-written impls of the two traits.
- **`Model` trait** (generated): `TABLE`, `PRIMARY_KEY`, `COLUMNS`, `from_row`, `to_values`, `primary_key_value`, `set_primary_key`, `meta()`, relation loader `load_relation(name, &mut [Self], db)`.
- **Eager loading**: for `with("posts.comments")`, run the parent query, collect keys, run one `WHERE fk IN (...)` query per relation (1000 keys per query on both databases), group by key, assign `Rel::Loaded`, then recurse for the nested path. Same N+1-avoidance strategy as Eloquent.
- **Soft deletes**: built-in global scope adds `deleted_at IS NULL` unless `with_trashed()` is used. `delete()` becomes an UPDATE; `force_delete()` is a real DELETE.
- **Hooks**: trait `ModelHooks` with async default no-op methods. A hook returning `Err` aborts the operation, like returning `false` in Eloquent.
- **Transactions**: `db.begin()` returns a `Transaction`. Rust has no async drop, so a dropped uncommitted transaction marks its connection for rollback before it goes back to the pool. Nested `begin()` on a `Transaction` uses `SAVEPOINT`.
- **Errors**: one `rustique::Error` (`thiserror`) with `NotFound`, `Database { code, message }`, `Decode { column }`, `RelationNotLoaded`, and others. Native error codes are kept so callers can match ORA-00001 / MySQL 1062 as unique-violation.

---

## 7. Phases

Estimates assume one developer who knows async Rust. Proc-macro and Oracle setup are the biggest unknowns.

1. **Core + grammar, no DB** — **done 2026-10-10**
   `Value`, `Row`, query AST, builder, MySQL and Oracle grammars. Tests in `rustique/tests/sql/` assert exact SQL and binds per dialect (plain `assert_eq!`, no `insta` needed yet).
   Deferred to the phase that needs them: aggregates (phase 2), `INSERT ... RETURNING` (phase 3), upsert (phase 6), row-value `IN` for composite keys (phase 4).

2. **Drivers + Db + transactions** — **done 2026-10-10**
   `Db::connect`, pooling, `Executor` (`select`/`statement` raw SQL with `?`), query terminal methods (`get`, `first`, `exists`, `count`/`max`/`min`/`sum`/`avg`, `insert`, `update`, `delete`), `begin`/`commit`/`rollback`, savepoints. Tests in `rustique/tests/db/` run on MySQL 8.4 and Oracle XE 21c.
   Later (2026-10-11): `Db::builder(url)` sets pool size, acquire / idle timeouts and max lifetime for both drivers (URL options still work; builder wins); MySQL acquire waits through `tokio::time::timeout` since `mysql_async` has none; default 30 s on both. `rustique::Tls` (feature `mysql-tls`) adds CA files, a client certificate and `danger_accept_invalid_certs` on top of `?require_ssl=true`; Oracle TLS stays in the `tcps://` URL. `db.transaction(|tx| Box::pin(async move { ... }))` commits on `Ok`, rolls back on `Err`; boxed because async closures can't promise `Send` futures on stable Rust; error type is any `E: From<Error>`.
   Async audit (2026-10-11): Oracle calls wait for a connection on a tokio semaphore (one permit per connection) instead of parking blocking threads in ODPI; a dropped Oracle query future sends OCIBreak (a cancelled pooled session is closed, not reused; `DBMS_SESSION.SLEEP` ignores breaks); a transaction with an open cursor fails other statements with `InvalidQuery` instead of deadlocking once the 100-row read-ahead fills. Rejected: awaiting the drop rollback (ODPI-C rolls back on release anyway, and the connection is not reused before), runtime-agnostic async (`mysql_async` needs tokio), non-`Send` hooks (models must work in `Send` handlers).
   Not done: MySQL `KILL QUERY` on a dropped future. (Nested transactions and deadlock retries came in phase 8.)

3. **Models** — **done 2026-10-10**
   `#[rustique::model]` macro (crate `rustique-macros`), `find`, `find_or_fail`, `all`, `save` (insert/update with dirty tracking), `delete`, `force_delete`, `restore`, `create`, timestamps, soft deletes, column constants, typed `Query<M>` (`with_trashed`, `only_trashed`, `where_key`). Sequence-based ids, trigger-filled ids (`RETURNING ... INTO`) and composite primary keys. Tests in `rustique/tests/db/models.rs` and `rustique/tests/sql/models.rs`.
   Casts: `#[derive(rustique::Cast)]` (enums, newtypes, JSON), tests in `rustique/tests/sql/casts.rs` and `rustique/tests/db/casts.rs`.
   Renamed columns and custom timestamp names came in phase 8; hooks in phase 6.

4. **Relationships + eager loading** — **done 2026-10-10**
   All five relation types as `Rel<T>` fields (`#[rustique(has_many(Post))]`, ...), lazy methods returning `Query<R>` (`user.posts()`), `with()` with nested paths, `Model::load`, `with_count` into `#[rustique(count)]` fields, `where_has` / `where_doesnt_have`, pivot `attach` / `detach` / `detach_all` / `sync` through the generated `user.roles_pivot()`. Composite keys use row-value `IN` (`where_rows_in`), split at 1000 rows on Oracle. Serde skips unloaded relations. Tests in `rustique/tests/db/relations.rs` (MySQL counts the statements: `with("posts.comments")` runs exactly 3) and `rustique/tests/sql/relations.rs`.
   Related models must be `Clone`. (Eager-load constraints and pivot columns came in phase 8, polymorphic relations in phase 7, self-relations in `where_has` / `with_count` in phase 9.)

5. **Schema builder + migrations** — **done 2026-10-10**
   `Schema::create`/`table`/`drop`/`drop_if_exists`/`rename`/`has_table`/`has_column` with a `Blueprint` (`rustique/src/schema.rs`, DDL in `grammar/schema.rs`). Column types: `id`, `big_increments`, `big_integer`, `integer`, `small_integer`, `foreign_id`, `boolean`, `string`, `char`, `text`, `long_text`, `double`, `decimal`, `date`, `datetime`, `json`, `binary`, `uuid`, `timestamps`, `soft_deletes`; modifiers `nullable`, `default`, `unsigned`, `primary`, `unique`, `index`, `constrained` + `cascade_on_delete`/`null_on_delete`. Composite `primary`/`unique`/`index`/`foreign`, `drop_column`, `rename_column`, `drop_index`, `drop_foreign`. Index names follow Laravel (`{table}_{columns}_{unique|index|foreign}`). `Migrator` (`migrate`/`rollback`/`status`) over a Laravel-shaped `migrations` table; migrations are modules with `up`/`down` fns listed in `rustique::migrations![...]`, each recorded right after it succeeds. Tests in `rustique/tests/sql/schema.rs` and `rustique/tests/db/schema.rs` (one migration set runs on both DBs, including FK cascade and defaults).
   `migrate` / `rollback` hold a lock (MySQL `GET_LOCK`, Oracle a `migrations_lock` table) so two migrators run one after another. `Migrator::table` / `lock_name` / `lock_timeout` rename them and bound the wait.
   Not done: `time` columns (Oracle has no `TIME`), migrations generated from a schema diff. (`change()` came in phase 8; `use_current`, raw defaults, comments, `fresh` / `reset` / `--step` in phase 9.)

6. **Extras** — **done 2026-10-10**
   `#[rustique::scopes]` (`scope_active(q)` becomes `User::active()` and `Query::active()` through a generated `UserScopes` trait; keyword names become `r#type`; names of `Model` functions such as `find` are refused), `Hooks` trait (`saving`/`saved`, `creating`/`created`, `updating`/`updated`, `deleting`/`deleted`, `force_deleting`/`force_deleted`, `restoring`/`restored`; an `Err` aborts) enabled with `#[rustique::model(hooks)]`, `pluck`, `pluck_keyed`, `paginate` into `Page<T>`, `chunk_by_id` (cursor-style `while let Some(chunk) = chunks.next(&db).await?`, ORs grouped, ordering replaced; composite keys compare as `a > ? OR (a = ? AND b > ?)` since Oracle has no row-value `>`), `shared_lock` (MySQL `FOR SHARE`; `Unsupported` on Oracle, which has no row-level shared lock), `upsert` (MySQL `INSERT ... AS new ON DUPLICATE KEY UPDATE`, Oracle `MERGE` with array binding), `Model::insert_many(&mut [M])` (all or nothing; fills generated keys, timestamps and `exists`; one multi-row `INSERT` on MySQL when `innodb_autoinc_lock_mode` is 0/1, else one `INSERT ... RETURNING` per row through `Executor::execute_all`, which wraps statements in a transaction or savepoint; the `oracle` crate's batch API cannot return values), `Model::upsert` (timestamps filled, no hooks, keys not read back). Aggregates, builder bulk insert and `lock_for_update` came with phases 1–2. Tests in `rustique/tests/sql/extras.rs` and `rustique/tests/db/extras.rs`.
   Not done: hooks on bulk queries (as in Eloquent). `insert_many` on MySQL's default lock mode 2 costs one round trip per row (Oracle batches since phase 8).

7. **Later / optional** — **done 2026-10-11**
   Polymorphic relations: `#[rustique(morph_one(R, name = "n"))]` / `morph_many` (filter `n_type` = the parent's `Model::MORPH_ALIAS`, default the table name, `#[rustique::model(morph_alias = "App\\Models\\Post")]` for Laravel data; `with`, `with_count`, `where_has` work) and `#[rustique(morph_to)]` on a `Rel<Option<T>>` with `#[derive(rustique::MorphTo)] enum T { Post(Post), ... }` (eager loading runs one query per type present, nested paths apply to every type; lazy `comment.commentable(&db).await`; an unknown type is `Error::UnknownMorphType`). Streaming cursors: `QueryBuilder::cursor` / `Query::cursor` return `Cursor` / `ModelCursor` (`while let Some(row) = cursor.next().await?`); a background task reads up to 100 rows ahead on its own connection (or the transaction's, which the cursor borrows), dropping the cursor stops it; model cursors refuse `with` (use `chunk_by_id`). Observers: `trait Observer<M>` with the `Hooks` methods as associated functions, registered with `#[rustique::model(observers(Audit, ...))]`, run after the model's hooks; one generic observer can serve every model. CLI: `rustique::cli::run(&migrator, args)` for the application's own binary (migrations are compiled in, so no standalone binary): `migrate`, `migrate:rollback`, `migrate:status` (URL from `$DATABASE_URL`, or the variable named by `--env`; never an argument, which `ps` would show), `make:migration <name> [--path dir]`. Query logging: every statement through `tracing`, target `rustique::query`, SQL with duration and row count at `DEBUG`, bindings at `TRACE`; each statement also runs in an `INFO` span `rustique.query` with OpenTelemetry database attributes (`otel.name`, `otel.kind = "client"`, `db.system`, `db.statement`, `otel.status_code` on failure; a cursor's span lasts as long as its rows) for `tracing-opentelemetry` to export under the application's current span (cursors and transaction-control statements record failures too). Decode errors (`Error::decode`, which names the model: `Row` carries the table `hydrate` sets) and model columns missing from a row emit an `ERROR` event on target `rustique::decode` in the caller's span, since models are built after the statement span ends: `error` without a message (an OpenTelemetry exception), `model`, and `exception.stacktrace` from `backtrace.rs` (forced capture, cleaned of runtime, std and driver frames, ORM runs collapsed to their outermost frame; parses `Backtrace`'s `Display` until `frames()` is stable). Values never appear. Tests in `rustique/tests/db/{cursors,observers,morphs}.rs`, `rustique/tests/logging.rs` and `rustique/src/cli.rs`.
   Not done: `make:migration` registering the module itself. (`morph_to_many`, `where_has_morph` and `migrate:fresh` / `reset` came in phase 9.)

8. **Eloquent parity pass** — **done 2026-10-11**
   From a comparison with Eloquent, Django, SQLAlchemy, EF Core, Hibernate, Prisma, Drizzle and Diesel. Row locking, the Oracle statement cache (ODPI-C caches 20 statements per session by default) and the migration lock already existed.
   Query builder: `when`, `latest` / `oldest`, `where_date` (compiled as a range so indexes apply), `where_json` / `or_where_json` (`col->a->0`; MySQL `JSON_UNQUOTE(JSON_EXTRACT())`, Oracle `JSON_VALUE` with `RETURNING NUMBER` for numbers; path segments limited to `[A-Za-z0-9_-]` since they are SQL literals), `union` / `union_all` (parts parenthesized, the main query's order and limit apply to all; aggregates wrap), `increment` / `decrement`, `cursor_paginate` into `CursorPage` (keyset on the `order_by` columns, mixed directions, opaque hex-encoded JSON cursors with typed values, backwards paging by flipping the order).
   Models: `#[rustique(column = "...")]`, `created_at` / `updated_at` / `deleted_at = "..."` options (now `Model::CREATED_AT` etc.), `first_or_new` / `first_or_create` / `update_or_create` / `create_or_first` (a unique violation on insert falls back to selecting the winner, with `FOR SHARE` on MySQL so REPEATABLE READ sees it; no savepoint, since both databases undo only the failed statement), `increment` / `decrement` (SQL addition, local value updated, `updating` / `updated` hooks), `touch`, `fresh` / `refresh` (ignore soft deletes and global scopes), `replicate`. Global scopes: `trait GlobalScope<M>` registered with `global_scopes(...)`, applied in `Query::into_builder` (so relation queries, eager loads and bulk updates see them), `without_global_scope::<S>()` / `without_global_scopes()`.
   Relations: `with_where(path, |q| ...)` constrains the last relation of a path (closures are `Arc<dyn Fn(QueryBuilder) -> QueryBuilder>`, untyped since `load_relation` dispatches by name; grouped like scopes), `with_count_where`; `belongs_to_many(..., pivot = [...], pivot_timestamps)` reads pivot columns into `Model::pivot()` (a `Row` in `Meta`), `Pivot::attach_with` / `update_existing`, timestamps on attach. `prevent_lazy_loading(Allow | Warn | Deny)`: a lazy relation query on a model loaded with others (flag in `Meta`) warns or fails.
   Transactions: `tx.transaction(...)` behind a savepoint, `db.transaction_with_retries(n, ...)` on deadlock / lock wait / serialization failure (`Error::is_concurrency_error`, `TransactionError` for application error types), `db.rollback_after(...)` for tests.
   Oracle `insert_many`: one PL/SQL block per 100 rows (`INSERT ... RETURNING INTO` local variables, copied into out binds at the end so they follow every in bind); `Compiled::returning` is now a count, and `fetch_all` returns the out binds as rows. Schema: `Column::change()` (MySQL `MODIFY`; Oracle `MODIFY` of type and default plus a nullability switch only when it differs, in a PL/SQL block).
   Tests in `rustique/tests/sql/helpers.rs`, `rustique/tests/db/helpers.rs` and `rustique/tests/lazy.rs` (own binary: the lazy-loading switch is process-wide).
   Factories: `HasFactory::definition(n)` plus `Factory` (`count`, `state`, `sequence`, `make`, `create`, `has` / `has_attached`, `for_` / `for_factory`, `recycle`, `after_making` / `after_creating`). Children and parent factories are stored type-erased, so their callbacks get an `AnyExecutor` (`&Db` or `&Transaction`); `Relation` now carries its `RelationKind` and models expose `morph_columns` for this. No Faker equivalent; `definition` gets a process-wide counter `n`.
   Not done: schema-diff migrations, per-parent limits in eager constraints, typed eager-constraint closures.

9. **Second Eloquent parity pass** — **done 2026-10-11**
   From `docs/eloquent-compatibility.md`, whose first version was partly stale (many items had come in phase 8).
   Query builder: `value`, `simple_paginate` into `SimplePage` (one row more than the page, no count), offset `chunk` (needs `order_by` on `QueryBuilder`; models order by key), `Query::sole` (`Error::MultipleRecords`), `where_keys`, `Model::find_many`.
   Relations: `with_sum` / `with_avg` / `with_min` / `with_max` (`{relation}_{fn}_{column}`, `Expr::Aggregate` got an optional alias) and `with_exists` (`CASE WHEN EXISTS ... THEN 1 ELSE 0 END`, as Oracle 19c has no booleans in the select list) into `#[rustique(aggregate)]` fields (a synonym of `count`). `has_one_through`. `morph_to_many` / `morphed_by_many`: `belongs_to_many` at runtime with `Through::morph` filtering and filling the pivot's type column, so pivots and factories work unchanged. Self-relations: `QueryBuilder::alias_table` renames the related (or pivot) table to `orm_self` in `correlated`, top-level conditions only (not raw SQL or nested subqueries). `where_has_morph` / `where_doesnt_have_morph` through `MorphTo::targets` (generated by the derive) and `Model::morph_targets`; `where_has` itself keeps its `FnOnce` closure. Pivot `sync_without_detaching`, `toggle`.
   Models: `uuid` / `ulid` options (`uuid` crate, already built by `mysql_async`, with `v7`; a ULID is a v7 UUID in Crockford base 32, so it sorts by time), filled before `creating` when the key is NULL or empty, also in `insert_many`. `Prunable` (`prune` loads chunks by key and force-deletes with hooks, `mass_prune` is one `DELETE`; soft-deleted rows included).
   Database: read replicas (`DbBuilder::read_url`, round-robin; plain `SELECT`s without locks or out binds only; `Db::write()`; schema checks, migrations, `create_or_first`'s lookup and the MySQL lock-mode probe use the primary through `AnyExecutor::on_writer`). No sticky reads: a `Db` has no request scope.
   Schema: `use_current` (UTC: MySQL `(UTC_TIMESTAMP(6))`, Oracle `SYS_EXTRACT_UTC(SYSTIMESTAMP)`), `default_raw`, column and table comments (Oracle `COMMENT ON`), `ulid` columns, `Schema::drop_all_tables` (MySQL one `DROP` with FK checks off on a pinned connection; Oracle `CASCADE CONSTRAINTS PURGE` per table, exact names). `Migrator::rollback_steps`, `reset`, `fresh` (keeps the Oracle lock table it holds), `seeder` / `seed`; CLI `migrate:reset`, `migrate:fresh`, `--step`, `--seed`, `db:seed`.
   Tests in `rustique/tests/sql/eloquent.rs` and `rustique/tests/db/eloquent.rs` (one test for the shared tables, plus replica routing checked by session id).
   Later the same day: `with_sum_where` / `with_avg_where` / `with_min_where` / `with_max_where`, and `with_morph_count` (Eloquent's `morphWithCount`): `EagerLoad::morph_counts` travels down the path into `Nested`, and `morph_models` adds `with_count` for the counts whose alias is the loaded model's; a non-`morph_to` target is `InvalidQuery`.
   Not done: sticky replica reads, `use_current_on_update`. `migrate:fresh` is tested by an ignored test (`--ignored fresh_drops_every_table`): it drops every table of the shared test schema.

10. **Bug hunt against real queries** — **done 2026-10-11**
   Edge cases in `rustique/tests/db/edge_cases.rs` and `rustique/tests/sql/edge_cases.rs`.
   Fixed: Oracle `lock_for_update()` with `first` / `find` / `limit` (ORA-02014) now locks through `WHERE ROWID IN (limited query)`, still `Unsupported` with joins, grouping or `skip_locked`; Oracle ordered unions read as an inline view (ORA-00904 on `SELECT *` parts); a raw condition next to others is parenthesized, so its `OR` cannot escape soft deletes, scopes or keyset chunking (a trailing `--` comment gets a line break); Oracle text / binary over 32,767 bytes binds as `CLOB` / `BLOB` (ORA-01461 in `MERGE` and `UPDATE`), batches type such a column from its longest value; MySQL statements that cannot be prepared (error 1295: `CREATE PROCEDURE`, `LOCK TABLES`) run through the text protocol when unbound; MySQL zero dates read as `NULL` and `TIME` outside a day as text instead of failing the whole row; `exists()` keeps the columns of unions and `HAVING` queries (and `limit(0)`); `simple_paginate` / `cursor_paginate` with `u64::MAX` and `decrement(i64::MIN)` no longer overflow; replica routing sees `FOR UPDATE` across line breaks and `LOCK IN SHARE MODE`; an empty `morph_to` type points nowhere, as in Eloquent; a MySQL `JSON` column reads into `String`, `BIT(1)` into `bool`, and Oracle text into `NaiveTime`; the Oracle raw-SQL lexer knows `q'[...]'` literals; `Error::Decode` shows its cause.
   Open then, closed in phase 11: time zone option, `where_in` beyond 65,535 values, `latestOfMany` and relation-level `where`.
   Query builder (2026-10-11): `QueryBuilder::from_sub(query, alias)` (Laravel `fromSub`), `join_sub` / `left_join_sub`, `right_join_with`, `group_by_raw`; `JoinClause::where_group` / `or_where_group` / `where_raw` / `or_where_raw` (nested and raw ON conditions); `with_count` / `with_sum` / ... / `with_exists` accept `"relation as alias"`. UPDATE / DELETE on a `from_sub` query is `InvalidQuery`; Oracle cannot lock a limited `from_sub` query (no ROWID window).

11. **Laravel gaps from the bug hunt** — **done 2026-10-11**
   Time zone: `rustique::set_timezone(tz)` takes any `chrono::TimeZone` (`FixedOffset`, `Local`, `chrono_tz::Tz` from the application's own dependency, so the ORM adds none), process-wide like Laravel's `app.timezone`. Timestamps, soft deletes, `touch` and pivot timestamps use `rustique::now()` in that zone; `DateTime<Utc>` values are written and read as wall-clock time in it (a wall time skipped by a DST jump uses the offset around it); Oracle `TIMESTAMP WITH TIME ZONE` reads into it. `use_current` defaults stay UTC; no per-connection session time zone. Tests in `rustique/tests/timezone.rs` (own binary: process-wide state).
   Long `IN` lists: more than 1000 integers skip per-item bindings (`grammar/conditions.rs`). MySQL writes them as literals (Laravel's `whereIntegerInRaw`); Oracle also caps a statement at 65,535 list items written as literals (ORA-00913), so it binds the list as one JSON array: `c IN (SELECT c0 FROM JSON_TABLE(:1, '$[*]' COLUMNS (c0 NUMBER PATH '$[0]')))`, which also replaces the 1000-item `OR` split. Composite `where_rows_in` does the same. Lists of other values still stop at 65,535 bindings.
   Relations: `scope = f` on any relation attribute (`fn(Query<R>) -> Query<R>`, a path or a non-capturing closure), applied grouped to lazy queries, eager loads and, through `Relation::related_query`, to `where_has` and counts; the generated code passes it to `__private::query` / `load` since `Relation` is not generic. `latest_of_many [= "col"]` / `oldest_of_many` on `has_one` / `morph_one`: `key IN (SELECT key FROM (SELECT key, ROW_NUMBER() OVER (PARTITION BY fk ORDER BY col, key) ...) WHERE rank = 1)` (new `Expr::RowNumber`, on top of `from_sub`), ranked inside the soft-delete filter, global scopes and `scope` (Eloquent's `ofMany` closure), NULL columns excluded like `MAX`; single-column related keys only. `with("posts:id,title")` selects columns of the path's last relation (qualified with the related table; pivot columns and the matched key still travel along), also for `morph_to`. Partial selects: `Query::hydrate` marks rows whose select list has no `*` / `table.*`, and the generated `from_row` reads fields through `Row::take_field`, which gives `Default` for missing columns only on such rows. Tests in `rustique/tests/sql/relation_constraints.rs`, `rustique/tests/db/relation_constraints.rs`, `rustique/tests/db/edge_cases.rs` and `rustique/tests/sql/wheres.rs`.
   Not done: `ofMany` with several columns or other aggregates, per-connection session time zones.

12. **Compile-time schema check** — **done 2026-10-12**
   Cargo feature `schema-check` (off by default; `rustique` forwards it to `rustique-macros`). The CLI's `schema:snapshot [--path file]` writes `rustique-schema.txt` from `information_schema.columns` (MySQL `column_type`) or `user_tab_columns` (Oracle `NUMBER(p,s)` / `VARCHAR2(n)` rebuilt from precision, scale and length). It has a `dialect` line, then one tab-separated `table, column, null / not null, type` line per column. `#[rustique::model]` reads the file next to `$CARGO_MANIFEST_DIR` and reports every mismatch at once: missing table (at `table = "..."`), missing column, nullable column without `Option` (Oracle `String` excepted, decision 3; `#[rustique(not_null)]` opts a field out, for legacy columns that are nullable but never `NULL`), and type mismatches that follow the `FromValue` impls (`rustique-macros/src/schema_check.rs`: `RustKind::reads`). Names match case-insensitively. It skips unknown field types (casts, `rustique::Value`), `schema.table` names, and runs without the file. The generated `const _ = include_bytes!(snapshot)` makes cargo rebuild models when the snapshot changes. There is no live database connection at compile time: `rustique-macros` would need both drivers and Oracle Instant Client, CI would need a database, and proc macros would not rerun when the schema changes. Tests: unit tests in `schema_check.rs`, `snapshot_lists_columns` in `rustique/tests/db/schema.rs`. Checked by hand against every test model on both databases, with no false type errors.
   Not done: checking relation keys, query-builder column strings and raw SQL; a snapshot per workspace rather than per crate.

**Total to useful v1 (phases 1–6): about 9–10 weeks.** Phases 7–8 are beyond v1.

---

## 8. Testing

- **Unit / snapshot:** SQL plus binds per dialect, no DB. Most bugs surface here, and it runs fast in CI.
- **Integration:** `docker-compose.yml` with `mysql:8.4` and `gvenzl/oracle-xe:21-slim-faststart` (closest free image to 19c). Tests run only when `RUSTIQUE_TEST_MYSQL_URL` / `RUSTIQUE_TEST_ORACLE_URL` are set.
- **Oracle image warning:** the convenient `gvenzl/oracle-free` image is **23ai**. It accepts SQL that 19c rejects (`BOOLEAN`, `IF EXISTS`, `SELECT 1` without `FROM DUAL`), so passing tests there do not prove 19c support. Use `container-registry.oracle.com/database/enterprise:19.3.0.0` (requires accepting Oracle's license) or an image built from `oracle/docker-images`, at least in CI.
- A shared integration test suite, generic over the backend, runs every scenario on both DBs.

---

## 9. Decisions (confirmed 2026-10-10)

1. **Async API on tokio.** Oracle calls run in `spawn_blocking`.
2. **Single model struct** with an injected hidden `__meta` field. Build new models with `..Default::default()`.
3. **Oracle `''` = NULL:** on Oracle only, NULL decodes into a non-`Option` `String` as `""`.
4. **Oracle identifiers** are uppercased and quoted (`email` becomes `"EMAIL"`).
5. **Schemas:** support new schemas (identity columns) **and** legacy ones:
   - **Sequence-based ids:** `#[rustique(sequence = "USERS_SEQ")]`. Insert uses `"USERS_SEQ".NEXTVAL` in the VALUES list. With a trigger-filled id, the key comes back through `RETURNING ... INTO`. MySQL ignores the attribute.
   - **Composite primary keys:** `#[rustique(primary_key)]` on several fields. `find` takes a tuple, `save`/`delete` filter on every key column, and relations accept multi-column keys (`foreign_key = ["a", "b"]`). Eager loading on composite keys uses `(a, b) IN ((?, ?), ...)`, which works on both DBs; the Oracle 1000-item chunking still applies.

---

## 10. Preparing for production

1. Test performance, N+1 queries, parent copies when using relationships, large datasets, and more. Done: `rustique/tests/db/perf.rs`.
2. Safely test against real queries from a real codebase. Done: phase 10.

Final review (2026-10-12): fixed `Model::increment` leaving unsigned and decimal fields dirty (a later `save` would have overwritten concurrent increments); `#[rustique::scopes]` now refuses names of `Query` methods, which hid the scope in a chain; stale docs fixed.

## 11. Open items

Everything left undone, gathered from the phases above:

- No CI. Integration tests run locally on Oracle XE 21c; nothing runs them on a real 19c yet (section 8).
- MySQL: a dropped query future does not `KILL QUERY`; the statement runs to the end.
- Oracle: a break sent just as a call ends can reach the transaction's next statement as ORA-01013 (`shortcut:` in `db/oracle.rs`).
- Schema: no `time` columns, no `use_current_on_update`, no migrations generated from a schema diff.
- `make:migration` does not register the module in `migrations![...]`.
- Relations: no per-parent limits in eager constraints, closures are untyped `QueryBuilder`s, `ofMany` takes one column.
- No sticky replica reads, no per-connection session time zone.
- Schema check: relation keys, query-builder column strings and raw SQL are unchecked; one snapshot per crate.
- `#[rustique::scopes]` keeps its own copy of the `Query` method names (`shortcut:` in `rustique-macros/src/lib.rs`).
