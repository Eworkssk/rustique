# Pagination, chunking and cursors

## Pagination

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool }
# async fn run(db: Db) -> rustique::Result<()> {
let page = User::query().order_by("id").paginate(&db, 2, 15).await?;   // page 2, 15 per page

&page.items;           // Collection<User>
&page.total;           // rows across all pages
&page.page;            // 2
&page.per_page;        // 15
page.last_page();      // 1 when there are no rows
page.has_more_pages();
# Ok(()) }
```

Runs a `COUNT(*)` and a page query. `QueryBuilder::paginate` works the same and returns `Page<Row>`. Order the query, or pages are not stable.

`simple_paginate` (Laravel's `simplePaginate`) skips the count: it fetches one row more than the page to tell whether another page follows.

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool }
# async fn run(db: Db) -> rustique::Result<()> {
let page = User::query().order_by("id").simple_paginate(&db, 2, 15).await?;   // SimplePage<User>
&page.items;
page.has_more_pages();   // or `page.has_more`; there is no total
# Ok(()) }
```

### Cursor pagination

Offset pages get slower the deeper they go. `cursor_paginate` (Laravel's `cursorPaginate`) filters on the ordering columns instead, so every page is one indexed range query. There is no total and no page number, only next and previous cursors:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool }
# async fn run(db: Db, cursor_from_request: Option<String>) -> rustique::Result<()> {
let page = User::query()
    .where_("active", "=", true)
    .order_by_desc("id")
    .cursor_paginate(&db, 15, cursor_from_request.as_deref())   // None for the first page
    .await?;

&page.items;          // Collection<User>
&page.next_cursor;    // Option<String>; None on the last page
&page.prev_cursor;    // Option<String>; None on the first page
page.has_more_pages();
# Ok(()) }
```

- Cursors are opaque strings, safe in a URL; their values are always sent as bindings.
- The ordering columns must be unique together and never NULL. Models append the primary key when it is missing; without ordering they page by key.
- `QueryBuilder::cursor_paginate` returns `CursorPage<Row>`; there the ordering is required and its columns must be selected.
- `order_by_raw` cannot be paged on.

## Chunking

Processes a big table in batches, keyed on the primary key (Laravel's `chunkById`):

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool }
# async fn run(db: Db) -> rustique::Result<()> {
let mut chunks = User::query().where_("active", "=", true).chunk_by_id(500);
while let Some(users) = chunks.next(&db).await? {
    for user in users.iter() { /* ... */ }
}
# Ok(()) }
```

- Each batch is a fresh query `WHERE id > last_id ORDER BY id LIMIT 500`, so it is safe to update or delete rows as you go.
- Your ordering is replaced by key order. `OR`s in your conditions are grouped.
- Composite keys work (`a > ? OR (a = ? AND b > ?)`).
- `with(...)` works: each chunk is eager-loaded.
- `QueryBuilder::chunk_by_id(500, "id")` takes the column explicitly.

`chunk(500)` (Laravel's `chunk`) pages with `LIMIT` / `OFFSET` in your own order instead. Rows that are inserted, deleted or changed while chunking shift the pages, so prefer `chunk_by_id` when you modify the rows you walk. Models without ordering are ordered by key; `QueryBuilder::chunk` needs an `order_by` and fails without one. Laravel's `lazy()` / `lazyById()` are `cursor` and `chunk_by_id` here.

## Cursors

Streams rows one at a time without loading them all:

```rust
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool }
# async fn run(db: Db) -> rustique::Result<()> {
let mut cursor = User::query().where_("active", "=", true).cursor(&db)?;
while let Some(user) = cursor.next().await? {
    /* ... */
}
# Ok(()) }
```

- A background task reads up to 100 rows ahead on its own connection (or on the transaction's, which the cursor then borrows). `cursor_buffered(&db, n)` sets another read-ahead: more for fewer round trips, fewer to hold less in memory. On Oracle, the rows per round trip come from [`DbBuilder::fetch_array_size`](getting-started.md#pool-settings-and-tls) (default 100).
- On a transaction, the cursor holds the connection: until it returns `None` or is dropped, any other statement on that transaction fails with `Error::InvalidQuery` (waiting would deadlock). Collect the rows first, or write through `db` / another transaction, if you need to write while reading.
- Dropping the cursor stops it.
- Model cursors refuse `with(...)`; use `chunk_by_id` when you need relations.
- `QueryBuilder::cursor(&db)?` streams `Row`s.
