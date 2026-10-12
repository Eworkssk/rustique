# Eloquent compatibility

Status key: **Have** = present. **Partial** = present with gaps. **Missing** = not implemented. **n/a** = the problem does not arise in this ORM, or Rust covers it another way.

Checked against the code on 2026-10-11, after the parity pass that closed the gaps of the first version of this report.

## Relations

| Eloquent | Here | Status |
|---|---|---|
| hasOne, hasMany, belongsTo, belongsToMany, morphOne/Many/To | `#[rustique(...)]` attributes | Have |
| hasManyThrough, hasOneThrough | `has_many_through`, `has_one_through` | Have |
| morphToMany / morphedByMany | `morph_to_many`, `morphed_by_many`, with the full pivot API | Have |
| withCount, withSum, withAvg, withMin, withMax, withExists | `with_count(_where)`, `with_sum`, `with_avg`, `with_min`, `with_max`, `with_exists(_where)` into `#[rustique(count)]` / `#[rustique(aggregate)]` fields | Have; each with a `_where` closure variant and `"relation as alias"` naming |
| eager-load constraints, `with('posts' => fn)` | `with_where` | Have; no per-parent limits |
| `with('posts:id,title')` | `with("posts:id,title")` | Have |
| `where` on a relation definition | `scope = fn` in the relation attribute | Have |
| latestOfMany, oldestOfMany, ofMany | `latest_of_many [= "column"]`, `oldest_of_many` on `has_one` / `morph_one` | Have; one column plus the key, single-column keys |
| pivot extra columns, `withPivot`, pivot timestamps | `pivot = [...]`, `pivot_timestamps`, `attach_with`, `update_existing` | Have |
| sync, syncWithoutDetaching, toggle | `sync`, `sync_without_detaching`, `toggle`, `attach`, `detach` | Have |
| whereHas / withCount on self-relations | inner table aliased `orm_self` | Have |
| whereHasMorph / whereDoesntHaveMorph | `where_has_morph`, `where_doesnt_have_morph` | Have |
| morphWithCount | `with_morph_count` | Have |
| withCount on morphTo itself | rejected, as in Eloquent | n/a |

## Models

| Eloquent | Here | Status |
|---|---|---|
| fillable / guarded | `{Model}Fill`, `#[rustique(fillable)]`; guarded by default | Have |
| accessors / mutators | plain methods, cast newtypes | n/a |
| hidden / visible / appends | `#[serde(skip_serializing)]`, custom `Serialize` | n/a; no runtime `makeHidden` |
| toArray / toJson | serde `Serialize` | Have |
| casts | `#[derive(rustique::Cast)]` | Have |
| custom timestamp columns | `created_at = "..."` etc. | Have |
| `app.timezone` | `rustique::set_timezone`, `rustique::now` | Have; process-wide, no per-connection session time zone |
| partial selects (`select('id', 'name')`) | missing columns take `Default` | Have |
| touch, replicate, refresh, fresh | same names | Have |
| UUID / ULID keys | `#[rustique::model(uuid)]` / `ulid` (UUID v7, lowercase ULID) | Have |
| global scopes | `GlobalScope`, `without_global_scope(s)` | Have |
| model pruning | `Prunable` (`prune`, `mass_prune`, `pruning` hook) | Have; no `model:prune` command |
| preventLazyLoading | `prevent_lazy_loading` | Have |

## Query builder

| Eloquent | Here | Status |
|---|---|---|
| where family, joins, unions, subqueries, when, fromSub, joinSub / leftJoinSub, groupByRaw | all present (`from_sub`, `join_sub`, `left_join_sub`, `group_by_raw`) | Have |
| aggregates, pluck, value | `count`, `sum`, `avg`, `min`, `max`, `pluck`, `pluck_keyed`, `value` | Have |
| whereIntegerInRaw | automatic for `IN` lists over 1000 integers | Have |
| increment, decrement | builder, `Query` and model | Have |
| firstOrNew, firstOrCreate, updateOrCreate, createOrFirst | same names | Have |
| chunk, chunkById, lazy, lazyById, cursor | `chunk`, `chunk_by_id`, `cursor` | Have |
| paginate, simplePaginate, cursorPaginate | same names | Have |
| sole, find, findMany, whereKey | `sole`, `find`, `find_many`, `where_key`, `where_keys` | Have; `sole` on model queries only |
| locks | `lock_for_update`, `shared_lock`, `skip_locked` | Have |

## Transactions and database

| Eloquent | Here | Status |
|---|---|---|
| nested transactions via savepoints | `tx.transaction(...)` | Have |
| deadlock retry | `transaction_with_retries` | Have |
| read / write connection split | `DbBuilder::read_url`, `Db::write` | Have; no `sticky` |
| query logging | `tracing`, target `rustique::query` | Have |

## Schema and migrations

| Eloquent | Here | Status |
|---|---|---|
| Schema builder, rename, drop, dropAllTables | present | Have |
| change column | `change()` | Have |
| migrate, rollback, rollback --step, reset, fresh, status | `Migrator` and the CLI | Have; no `squash` |
| migration lock against concurrent runs | `GET_LOCK` / lock table | Have |
| useCurrent, raw defaults, column and table comments | `use_current`, `default_raw`, `comment` | Have; no `useCurrentOnUpdate` |
| time columns | none (Oracle has no `TIME`) | Missing |

## Other

| Eloquent | Here | Status |
|---|---|---|
| events and observers | `Hooks` and `Observer` | Have |
| hooks on bulk queries | not fired, as in Eloquent | Have |
| factories | `HasFactory`, `Factory` | Have; no Faker |
| seeders | `Migrator::seeder`, `db:seed`, `--seed` | Have |
| lazy collections | `cursor` streams models | Have |
