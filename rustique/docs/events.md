# Scopes, hooks and observers

## Local scopes

```rust
use rustique::Query;
# use rustique::prelude::*;
# #[rustique::model(table = "users")]
# #[derive(Debug, Clone, Default)]
# pub struct User { pub id: i64, pub active: bool, pub r#type: String, pub country_id: i64 }

#[rustique::scopes]
impl User {
    fn scope_active(q: Query<User>) -> Query<User> {
        q.where_("active", "=", true)
    }

    fn scope_of_type(q: Query<User>, ty: &str) -> Query<User> {
        q.where_("type", "=", ty)
    }
}

# async fn run(db: Db) -> rustique::Result<()> {
let users = User::active().of_type("admin").get(&db).await?;
let users = User::query().where_("country_id", "=", 1).active().get(&db).await?;
# Ok(()) }
```

- `scope_x` becomes `User::x()` and `Query<User>::x()`. Extra arguments carry over.
- The query methods live in a generated trait `UserScopes`. Import it (`use crate::models::UserScopes;`) where you call scopes outside the defining module.
- As in Eloquent, `OR`s inside a scope cannot escape: the conditions before the scope and those it adds are each wrapped in parentheses.
- A scope named like a keyword (`scope_type`) becomes `r#type`. A scope cannot share a name with a relation method, a `Model` function (`find`, `all`, ...) or a `Query` method (`latest`, `with`, `count`, ...), which would hide it in a chain; the macro refuses the last two.
- Other items in the `impl` block stay as they are.

## Global scopes

A global scope is a filter every query of a model applies, such as tenant scoping. Implement `rustique::GlobalScope` and list it in `global_scopes(...)`. One generic scope can serve every model:

```rust
use rustique::prelude::*;
use rustique::{GlobalScope, Query};

pub struct Tenant;

impl<M: Model> GlobalScope<M> for Tenant {
    fn apply(query: Query<M>) -> Query<M> {
        let tenant = 7; // e.g. read from a tokio task-local set per request
        query.where_(format!("{}.tenant_id", M::TABLE), "=", tenant)
    }
}

#[rustique::model(table = "posts", global_scopes(Tenant))]
#[derive(Debug, Clone, Default)]
pub struct Post { pub id: i64, pub tenant_id: i64 }

# async fn run(db: Db) -> rustique::Result<()> {
let mine = Post::query().get(&db).await?;                                  // ... WHERE posts.tenant_id = ?
let all = Post::query().without_global_scope::<Tenant>().get(&db).await?;
let raw = Post::query().without_global_scopes().get(&db).await?;          // soft deletes still apply
# Ok(()) }
```

- Scopes apply when the query runs, after its own conditions; `OR`s on either side stay grouped, as with local scopes.
- They apply to relation queries, eager loads, `with_count`, `where_has`, and bulk `update` / `delete` / `increment` through `Query`.
- They do not apply to `save`, `delete`, `force_delete` or `restore` on a model, which address one row by its key, nor to `fresh` / `refresh`.
- Soft deletes are a built-in global scope; `with_trashed` / `only_trashed` control it.

## Hooks (model events)

Add `hooks` to the model and implement `rustique::Hooks`. Every method defaults to a no-op; implement the ones you need.

```rust
use rustique::prelude::*;

#[rustique::model(table = "users", hooks)]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub email: String,
}

impl rustique::Hooks for User {
    async fn saving(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        self.email = self.email.trim().to_lowercase();
        Ok(())
    }

    async fn deleting(&mut self, _db: &impl Executor) -> rustique::Result<()> {
        Err(rustique::Error::InvalidQuery("users cannot be deleted".into()))   // Err aborts
    }
}
```

| Hook pair | Runs around |
|---|---|
| `saving` / `saved` | every `save` (insert or update) |
| `creating` / `created` | insert |
| `updating` / `updated` | update |
| `deleting` / `deleted` | `delete` (soft or hard) |
| `force_deleting` / `force_deleted` | `force_delete` |
| `restoring` / `restored` | `restore` |

Returning `Err` aborts the operation with that error, like returning `false` in Eloquent. The `db` argument is the same executor the operation runs on, so queries in a hook join the caller's transaction.

Hooks run on model methods only: `save`, `create`, `delete`, `force_delete`, `restore`, and `updating` / `updated` on `increment` / `decrement`. They do **not** run for `Query::update`, `Query::delete`, `insert_many`, `upsert` or the query builder.

## Observers

Observers hold hooks outside the model. One observer type can serve many models.

```rust
use rustique::prelude::*;

pub struct Audit;

impl<M: rustique::Model> rustique::Observer<M> for Audit {
    async fn deleted(model: &mut M, db: &impl Executor) -> rustique::Result<()> {
        tracing::info!(table = M::TABLE, "deleted");
        Ok(())
    }
}

#[rustique::model(table = "users", observers(Audit))]
#[derive(Debug, Clone, Default)]
pub struct User { pub id: i64 }
```

Observer methods are associated functions with the same names as the `Hooks` methods, taking `model: &mut M`. They run after the model's own hooks, in the listed order. An `Err` aborts, as with hooks.
