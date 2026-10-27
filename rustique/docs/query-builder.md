# Query builder

`QueryBuilder` builds SQL for any table and returns `Row`s. Models use the same methods through `Query<M>` (see [Models](models.md)), which returns models instead of rows.

```rust
use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {

let rows = QueryBuilder::table("users")
    .where_("active", "=", true)
    .order_by("name")
    .get(&db)
    .await?;                       // Collection<Row>

for row in &rows {
    let name: String = row.get("name")?;
}
# Ok(()) }
```

Builder methods take `self` and return `Self`; chain them. Terminal methods are `async` and take `&impl Executor` (`&Db` or `&Transaction`).

## Retrieving

| Method | Returns |
|---|---|
| `get(&db)` | `Collection<Row>` |
| `first(&db)` | `Option<Row>` |
| `value::<T>(&db, "col")` | `T`: the column of the first row; use `Option<T>`, `NULL` when no row matches |
| `exists(&db)` | `bool` |
| `pluck::<T>(&db, "col")` | `Vec<T>` |
| `pluck_keyed::<K, V>(&db, "value_col", "key_col")` | `Vec<(K, V)>` (note: value column first) |
| `cursor(&db)` | streaming `Cursor`, see [large results](large-results.md) |
| `paginate`, `simple_paginate`, `cursor_paginate`, `chunk`, `chunk_by_id` | see [large results](large-results.md) |

`Collection<T>` derefs to a `Vec<T>` and adds `pluck(|x| ...)`, `key_by(|x| ...)`, `group_by(|x| ...)` and `into_inner()`.

## Aggregates

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let count = QueryBuilder::table("orders").count(&db).await?;                    // u64
let max: i64 = QueryBuilder::table("orders").max(&db, "amount").await?;
let avg: Option<f64> = QueryBuilder::table("orders").avg(&db, "amount").await?; // NULL on no rows
# Ok(()) }
```

Also `min`, `sum`. The result type is any `FromValue`; use `Option<T>` when the table may be empty.

## Selects

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("users")
    .select(["id", "name"])
    .add_select(["email"])
    .select_raw("UPPER(name) AS upper_name", [])
    .select_sub(QueryBuilder::table("posts").select_raw("COUNT(*)", []).where_column("posts.user_id", "=", "users.id"), "post_count")
    .distinct();
# Ok(()) }
```

## Where clauses

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("users")
    .where_("votes", ">", 100)
    .or_where("name", "=", "John")
    .where_column("updated_at", ">", "created_at")
    .where_in("id", [1, 2, 3])
    .where_not_in("status", ["banned"])
    .where_null("deleted_at")
    .where_not_null("email")
    .where_between("age", 18, 65)
    .where_not_between("score", 0, 10)
    .where_raw("LOWER(email) = ?", ["a@x.io".into()]);
# Ok(()) }
```

Every `where_*` has an `or_*` twin (`or_where_in`, `or_where_null`, ...), except the between, date, exists and sub-query forms.

Operators: `=`, `<>`, `!=`, `<`, `<=`, `>`, `>=`, `like`, `not like`, `is`, `is not` (case-insensitive words). An unknown operator makes the query fail when compiled.

Values are anything that converts into `Value`: integers, floats, `bool`, `&str`, `String`, `chrono` types, `rust_decimal::Decimal`, `serde_json::Value`, `Option<T>`, and your [cast types](casts.md). They are always sent as bindings, never spliced into SQL, with one exception: an `IN` list of more than 1000 integers (MySQL and Oracle allow 65,535 bindings per statement) is written into the SQL on MySQL, like Laravel's `whereIntegerInRaw`, and bound as one JSON array on Oracle, so `where_in("id", ids)` works with any number of integer ids. Longer lists of other values still fail with `Error::InvalidQuery` past 65,535 bindings.

Long `IN` lists are slow even so: the database parses and checks every value, and on MySQL each list length (and each list over 1000 integers) is a statement of its own in the prepared statement cache. In debug builds a `where_in` / `where_not_in` list of more than 1000 values logs a warning (target `rustique::query`). Ids plucked from one query to filter another belong in a subquery, which the database runs as one statement:

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
// Not: let ids: Vec<i64> = projects.pluck(&db, "id").await?; ... .where_in("project_id", ids)
let projects = QueryBuilder::table("projects").select(["id"]).where_("archived", "=", false);
let tasks = QueryBuilder::table("tasks").where_in_sub("project_id", projects).get(&db).await?;
# Ok(()) }
```

Raw SQL fragments use `?` placeholders on both databases; the Oracle grammar rewrites them to `:1, :2, ...`. A raw condition next to others is parenthesized, so an `OR` inside it (`where_raw("a = ? OR b = ?", ...)`) cannot escape the soft-delete filter, scopes or `chunk_by_id`.

### Dates and JSON

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let day = chrono::NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
let q = QueryBuilder::table("users")
    .where_date("created_at", "=", day)            // created_at >= day AND created_at < day + 1
    .where_json("settings->theme", "=", "dark")    // key `theme` of JSON column `settings`
    .or_where_json("settings->tags->0", "=", "vip");  // first array item
# Ok(()) }
```

`where_date` compares the date part of a date-time column as a range, so an index on the column still applies. Operators: `=`, `<>`, `<`, `<=`, `>`, `>=`.

`where_json` reads a value inside a JSON column: MySQL `JSON_UNQUOTE(JSON_EXTRACT(...))`, Oracle `JSON_VALUE(...)` (with `RETURNING NUMBER` for numeric values). Path segments may use letters, digits, `_` and `-`; a digits-only segment is an array index. Booleans compare as `"true"` / `"false"`; NULL is rejected.

### Conditional clauses

```rust
# use rustique::prelude::*;
# async fn run(db: Db, search: Option<String>) -> rustique::Result<()> {
let q = QueryBuilder::table("users")
    .when(search.is_some(), |q| q.where_("name", "like", format!("%{}%", search.clone().unwrap())));
# Ok(()) }
```

### Grouping

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
// WHERE active = ? AND (role = ? OR role = ?)
let q = QueryBuilder::table("users")
    .where_("active", "=", true)
    .where_group(|g| g.where_("role", "=", "admin").or_where("role", "=", "owner"));
# Ok(()) }
```

`or_where_group` joins the group with `OR`.

### Sub-queries and exists

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let banned = QueryBuilder::table("bans").select(["user_id"]);
let q = QueryBuilder::table("users")
    .where_not_in_sub("id", banned)
    .where_exists(QueryBuilder::table("orders").where_column("orders.user_id", "=", "users.id"));
# Ok(()) }
```

Also `where_in_sub`, `where_not_exists`. For composite keys: `where_rows_in(["a", "b"], rows)` builds `(a, b) IN ((?, ?), ...)`.

## Joins

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("users")
    .join("posts", "users.id", "=", "posts.user_id")
    .left_join("profiles", "users.id", "=", "profiles.user_id")
    .left_join_with("logins", |j| j.on("users.id", "=", "logins.user_id").where_("logins.ok", "=", true))
    .cross_join("colors");
# Ok(()) }
```

Also `right_join`, `join_with`, `right_join_with`. `JoinClause` has `on`, `or_on`, `where_`, `or_where`, `where_raw`, `or_where_raw`, and `where_group` / `or_where_group` for parenthesized conditions:

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
// ON (kod_id <> ? AND sk_id = kod_id) OR (id_tree <> ? AND (typ LIKE CONCAT(id_tree, '%')))
let q = QueryBuilder::table("price_items")
    .join_with("products", |j| {
        j.where_group(|g| g.where_("kod_id", "<>", 0).on("sk_id", "=", "kod_id"))
            .or_where_group(|g| g.where_("id_tree", "<>", "0").where_raw("typ LIKE CONCAT(id_tree, '%')", []))
    });
# Ok(()) }
```

### Subqueries as tables

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let latest = QueryBuilder::table("logins")
    .select(["user_id"])
    .select_raw("MAX(created_at) AS last_login", [])
    .group_by(["user_id"]);
// LEFT JOIN (SELECT ...) latest ON latest.user_id = users.id
let users = QueryBuilder::table("users")
    .left_join_sub(latest, "latest", "latest.user_id", "=", "users.id");

let per_branch = QueryBuilder::table("orders")
    .select_raw("branch, COUNT(*) AS orders_count", [])
    .group_by(["branch"]);
// SELECT * FROM (SELECT ...) per_branch WHERE orders_count > ?
let busy = QueryBuilder::from_sub(per_branch, "per_branch")
    .where_("orders_count", ">", 10)
    .get(&db)
    .await?;
# Ok(()) }
```

`join_sub` is the inner-join twin of `left_join_sub`. A `from_sub` query can be counted, paginated and used as a subquery itself, but not updated or deleted (`Error::InvalidQuery`). MySQL rejects `LIMIT` in an `IN (...)` subquery; put the limited query one `from_sub` deeper.

## Ordering, grouping, limits

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("users")
    .order_by("name")
    .order_by_desc("created_at")
    .latest("updated_at")   // order_by_desc; `oldest` is order_by
    .order_by_raw("FIELD(status, ?, ?)", ["new".into(), "old".into()])
    .group_by(["country"])
    .group_by_raw("YEAR(created_at)", [])
    .having("total", ">", 100)
    .having_raw("COUNT(*) > ?", [5.into()])
    .limit(10)
    .offset(20)
    .for_page(3, 15);   // page 3, 15 per page
# Ok(()) }
```

`limit_per(["user_id"])` applies `limit` / `offset` to each group of rows sharing those columns, like Laravel's `groupLimit`: `SELECT * FROM (SELECT ..., ROW_NUMBER() OVER (PARTITION BY user_id ORDER BY <the query's order>) AS orm_row FROM ...) orm_limited WHERE orm_row <= ? ORDER BY orm_row`. The query needs an order. Eager loads use it for a `with_where` limit, and `Model::query()` has it too, for "the first N of each group" in one query: `Task::query().order_by("position").limit(50).limit_per(["task_list_id"])` loads at most 50 tasks of each list. The rows come back ordered by their number within the group (the first task of every list, then the second, ...), so group them in Rust. `paginate`, `simple_paginate`, `cursor_paginate`, `chunk` and `chunk_by_id` replace the limit, so they fail with `Error::InvalidQuery` on such a query.

`order_by_raw` sorts by an expression, which an ordinary column index cannot provide: `order_by_raw("due_on IS NULL", [])` makes the database sort every matching row before the `limit`. Sort by plain indexed columns on large tables, or keep the expression to a small, already filtered result.

## Unions

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let admins = QueryBuilder::table("admins").select(["id", "name"]);
let rows = QueryBuilder::table("users")
    .select(["id", "name"])
    .union(admins)            // union_all keeps duplicates
    .order_by("name")         // orders and limits the combined rows
    .limit(10)
    .get(&db)
    .await?;
# Ok(()) }
```

Each part is parenthesized, so the main query's ordering, limit and offset apply to all rows. Order by output column names (`name`, not `users.name`); Oracle reads an ordered union as an inline view (`SELECT * FROM (...) ORDER BY ...`). `count`, `exists` and the other aggregates work on unions. Unions cannot lock rows. Oracle cannot `union` tables with `CLOB` columns; use `union_all`.

## Locking

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("jobs").where_("done", "=", false);
q.clone().lock_for_update();                 // FOR UPDATE
q.clone().lock_for_update().skip_locked();   // FOR UPDATE SKIP LOCKED
q.clone().lock_for_update_no_wait();         // FOR UPDATE NOWAIT
q.clone().lock_for_update_wait(std::time::Duration::from_secs(5)); // Oracle FOR UPDATE WAIT 5
q.shared_lock();                             // MySQL FOR SHARE; Error::Unsupported on Oracle
# Ok(()) }
```

Locks only make sense inside a [transaction](database.md). `lock_for_update()` with `first`, `find` or `limit` works on both databases: Oracle, which rejects `FOR UPDATE` after a row limit, locks the rows the limited query picks (`WHERE ROWID IN (...)`). On Oracle that is not possible with joins, grouping or `skip_locked`, which then return `Error::Unsupported`.

`lock_for_update_no_wait()` fails at once, and `lock_for_update_wait(timeout)` after `timeout` (rounded up to whole seconds), when another transaction holds one of the rows. Tell that error apart with `Error::is_lock_timeout()` (MySQL 3572 / 1205, ORA-00054 / ORA-30006):

```rust
# use rustique::prelude::*;
# async fn run(tx: &rustique::Transaction) -> rustique::Result<()> {
match QueryBuilder::table("jobs").where_("id", "=", 1).lock_for_update_no_wait().first(tx).await {
    Err(e) if e.is_lock_timeout() => { /* someone else is working on it */ }
    other => { other?; }
}
# Ok(()) }
```

MySQL has no per-statement wait (its `innodb_lock_wait_timeout` is a session setting, see [`session_setup`](getting-started.md#pool-settings-and-tls)), so `lock_for_update_wait` returns `Error::Unsupported` there. Neither database combines `skip_locked` with a wait; that is an `Error::InvalidQuery`.

## Insert, update, delete

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let executed = QueryBuilder::table("users")
    .insert(&db, &["name", "email"], vec![
        vec!["Ann".into(), "a@x.io".into()],
        vec!["Bo".into(), "b@x.io".into()],
    ])
    .await?;
executed.rows_affected;   // u64
executed.last_insert_id;  // Option<u64>, of the first statement

let changed: u64 = QueryBuilder::table("users")
    .where_("id", "=", 1)
    .update(&db, vec![("name", "Ann".into())])
    .await?;

let deleted: u64 = QueryBuilder::table("users").where_("active", "=", false).delete(&db).await?;

// SET votes = votes + ?, and any other columns
QueryBuilder::table("users").where_("id", "=", 1).increment(&db, "votes", 1, vec![]).await?;
QueryBuilder::table("users").where_("id", "=", 1).decrement(&db, "credits", 5, vec![("updated_at", Value::Null)]).await?;
# Ok(()) }
```

Big inserts are split into several statements automatically.

### Upsert

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
QueryBuilder::table("prices").upsert(
    &db,
    &["sku", "price"],
    vec![vec!["A1".into(), 10.into()]],
    &["sku"],          // unique_by: the conflict columns
    &["price"],        // columns to update on conflict
).await?;
# Ok(()) }
```

MySQL uses `INSERT ... AS new ON DUPLICATE KEY UPDATE`, Oracle uses `MERGE`. MySQL counts an updated row twice in the returned number.

### Insert from a select

`insert_using` is Laravel's `insertUsing`: `INSERT INTO table (columns) SELECT ...` in one statement, so the rows never travel through the application. It returns how many rows were inserted.

```rust
# use rustique::prelude::*;
# async fn run(db: Db, ids: Vec<i64>) -> rustique::Result<()> {
let copied: u64 = QueryBuilder::table("orders_archive")
    .insert_using(
        &db,
        &["id", "total"],
        QueryBuilder::table("orders").select(["id", "total"]).where_in("id", ids),
    )
    .await?;
# Ok(()) }
```

The source query is any `QueryBuilder`, with one selected column per entry of `columns`; with no columns it fills every column of the table in order. `IN` lists over 1,000 integers work as in a plain select (literals on MySQL, one JSON array bind through `JSON_TABLE` on Oracle).

## Seeing the SQL

```rust
# use rustique::prelude::*;
# async fn run(db: Db) -> rustique::Result<()> {
let q = QueryBuilder::table("users").where_("id", "=", 1);
let compiled = q.to_sql(Dialect::Oracle)?;
compiled.sql;    // String
compiled.binds;  // Vec<Value>
# Ok(()) }
```

Also `to_insert_sql`, `to_insert_using_sql`, `to_upsert_sql`, `to_update_sql`, `to_delete_sql`. No database needed, good for tests.
