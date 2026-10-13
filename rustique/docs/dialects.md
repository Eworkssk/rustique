# Database differences

You write one query; the grammar compiles it per database. Things you can see from the outside:

| Concern | MySQL 8.4 | Oracle 19c |
|---|---|---|
| Placeholders in raw SQL | `?` | write `?`; sent as `:1, :2, ...` |
| Identifiers | `` `users`.`email` `` | `"USERS"."EMAIL"`: uppercased and quoted |
| Limit / offset | `LIMIT n OFFSET m` | `OFFSET m ROWS FETCH NEXT n ROWS ONLY` |
| Generated key | `last_insert_id()` | `RETURNING ... INTO` |
| Upsert | `INSERT ... AS new ON DUPLICATE KEY UPDATE` | `MERGE` |
| Booleans | `TINYINT(1)` | `NUMBER(1)` |
| Empty string | `''` | `''` **is `NULL`** (decodes to `""` in a `String` field) |
| `IN (...)` lists | 65,535 bindings per statement; over 1000 integers are written as literals | 1000 items per list (longer lists become `OR`ed lists), 65,535 bindings; over 1000 integers are bound as one JSON array read by `JSON_TABLE` |
| `shared_lock()` | `FOR SHARE` | `Error::Unsupported` |
| `lock_for_update()` with `limit` / `first` | `LIMIT 1 FOR UPDATE` | `WHERE ROWID IN (SELECT ROWID ... FETCH NEXT 1 ROWS ONLY) FOR UPDATE` (ORA-02014 otherwise); `Error::Unsupported` with joins, grouping or `skip_locked` |
| Ordered union | `(...) UNION (...) ORDER BY x` | `SELECT * FROM ((...) UNION (...)) ORDER BY x` (ORA-00904 otherwise) |
| `TIME` column | `TIME` (outside a day, such as `25:00:00`, read as text) | none: `time()` creates `VARCHAR2(18 CHAR)`; times are stored as text and read back as `NaiveTime` |
| `use_current_on_update()` | `ON UPDATE CURRENT_TIMESTAMP(6)` | ignored (no `ON UPDATE`; the model sets the column) |
| Zero dates `0000-00-00` | read as `NULL` | n/a |
| JSON column | `JSON` | `CLOB` with `IS JSON` check |
| DDL in a transaction | auto-commits | auto-commits |

## Oracle notes

- Unquoted Oracle names fold to uppercase, so the grammar uppercases and quotes every identifier. Keep lowercase names in Rust; `Row` lookup is case-insensitive. Tables created with quoted lowercase names will not match.
- Needs Oracle Instant Client (`libclntsh`) at runtime. Calls run in `spawn_blocking`.
- Legacy sequence keys: `#[rustique(primary_key, sequence = "SEQ")]`, see [Models](models.md#primary-keys).
- Text and binary values over 32,767 bytes are bound as `CLOB` / `BLOB`, so they also work in `upsert` (`MERGE`), not only in `INSERT`.
- `UNION` cannot compare `CLOB` columns (ORA-00932); use `union_all` for tables with `CLOB` / JSON columns.
- Test against real 19c. Oracle 21c XE and 23ai accept SQL that 19c rejects.

## MySQL notes

- Statements MySQL cannot prepare (`CREATE PROCEDURE`, `CREATE TRIGGER`, `LOCK TABLES`, ...) run through the text protocol when they have no bindings.
- A `JSON` column reads into a `String` field as its text, and a `BIT(1)` column into a `bool`.

## Branching on the database

```rust
# use rustique::prelude::*;
# async fn run(db: Db, query: QueryBuilder) -> rustique::Result<()> {
match db.dialect() {
    Dialect::MySql => { /* ... */ }
    Dialect::Oracle => { /* ... */ }
    _ => { /* Dialect is #[non_exhaustive] */ }
}

let compiled = query.to_sql(Dialect::Oracle)?;   // inspect SQL without a database
# Ok(()) }
```
