# ORM benchmark: rustique vs diesel vs Eloquent

```sh
./bench/run.sh                     # MySQL; writes bench/RESULTS-mysql.md
DB=oracle ROUNDS=3 ./bench/run.sh  # Oracle; writes bench/RESULTS-oracle.md
```

Needs Docker (MySQL and Oracle from `../docker-compose.yml`), the `mysql` client, cargo, PHP 8.2+ with `pdo_mysql` (and `oci8` for Oracle), Oracle Instant Client for Oracle, composer and python3. `ROUNDS` defaults to 5.

On Oracle, diesel runs through the third-party [`diesel-oci`](https://crates.io/crates/diesel-oci) backend and Eloquent through [`yajra/laravel-oci8`](https://github.com/yajra/laravel-oci8). `schema-oracle.sql` loads the same rows with the column types both rustique's and Laravel's migrations create there: `NUMBER(19)` identity keys, `TIMESTAMP(6)`, and `CLOB` for `body` and for the JSON `payload` (with an `IS JSON` check). It ends with `DBMS_STATS.GATHER_SCHEMA_STATS`.

`schema.sql` loads 100,000 users with 10 posts each (1M posts) and 100,000 events whose `payload` is a JSON object with nested objects and arrays. Before every runner, `run.sh` deletes the rows earlier write scenarios added and waits until InnoDB has flushed its dirty pages. Each runner uses one connection and runs the scenarios in sequence. Each scenario does a short untimed warm-up, then records one sample per operation. Rounds rotate the runner order. Timings of all rounds are pooled, and memory figures are the median of the rounds.

| Scenario | Operation |
|---|---|
| `find_by_pk` | load one user by primary key |
| `where_order_limit_20` | `WHERE age = ? ORDER BY id LIMIT 20` |
| `fetch_{1000,10000,100000}_rows` | load that many users as models |
| `eager_50_users_500_posts` | 50 users with their posts (2 queries) |
| `eager_1000_users_10000_posts` | 1,000 users with their posts (2 queries) |
| `json_fetch_{1000,10000}` | load events, decoding `payload` into a typed value |
| `json_where_limit_20` | `WHERE payload->type = ? ORDER BY id LIMIT 20` |
| `update_single` / `json_update_single` | change a user's name / two nested payload fields, and save |
| `insert_single` / `json_insert_single` | create one user / one event |
| `bulk_insert_{1000,10000}` | insert that many users in one call |

JSON casts: rustique uses `#[derive(rustique::Cast)] #[cast(json)]` on a serde struct, diesel uses the same struct with `FromSql` / `ToSql` for `Json`, and Eloquent uses the `array` cast.

Memory, per scenario:

- **Heap peak**: the most memory in use above where the scenario started. In Rust this is a counting global allocator, and in PHP `memory_get_peak_usage()`. Diesel's figure misses the buffers libmysqlclient takes with C `malloc`.
- **Peak RSS**: the whole process (`VmHWM`, reset before each scenario). It includes the runtime itself and memory the allocator kept from earlier scenarios.

Differences in what each ORM does:

- `update_single`, `json_update_single`: rustique and Eloquent save an already loaded model (dirty tracking, `updated_at`). Diesel runs `UPDATE ... WHERE id = ?` with the changed columns.
- `insert_single`, `json_insert_single`: rustique and Eloquent read the generated key back into the model. Diesel on MySQL does not.
- `bulk_insert_*`: rustique and Eloquent use the query builder (`QueryBuilder::insert`, `Model::insert`), diesel inserts a `Vec` of `Insertable` structs. None of them return models.
- `json_where_limit_20`: diesel has no JSON path DSL, so it uses a raw SQL fragment with a bound value (`JSON_EXTRACT` on MySQL, `JSON_VALUE` on Oracle).
- Oracle JSON: diesel-oci has no JSON type, so diesel reads the `CLOB` as text and decodes it with serde, and writes a serialized string.
- Oracle Eloquent: yajra needs `server_version` set (it assumes 11g and refuses JSON paths otherwise).
- PHP runs without Xdebug, which would otherwise slow it down and turn the JIT off. `eloquent-opcache-jit` is the same script with opcache and the tracing JIT on.
