//! Active-record models: the [`Model`] trait that `#[rustique::model]` implements.
//!
//! - `mod.rs`: [`Model`] with `find` / `save` / `delete` / ..., and [`Meta`].
//! - `query.rs`: [`Query`], a [`QueryBuilder`] that returns models.
//! - `relation.rs`: [`Rel`] fields, relation queries, eager loading, [`Pivot`].
//! - `factory.rs`: [`HasFactory`] and [`Factory`], test and seed data.
//! - `prune.rs`: [`Prunable`], deleting obsolete models.

mod factory;
mod observer;
mod prune;
mod query;
mod relation;

pub use factory::{Factory, HasFactory};
pub use observer::{Event, Observer, observe};
pub use prune::Prunable;
pub(crate) use query::EagerLoad;
pub use query::{GlobalScope, ModelChunks, ModelCursor, Nested, Query, RelName, scope};
pub use relation::{
    BoxFuture, FromRelated, LazyLoading, MorphTarget, MorphTo, OfMany, Pivot, Rel, Relation,
    RelationKind, RelationScope, Through, load, load_morph, morph_models, morph_to,
    prevent_lazy_loading, query,
};

use std::fmt;
use std::future::Future;

use chrono::SubsecRound;

use crate::db::Executor;
use crate::grammar;
use crate::{Collection, Dialect, Error, QueryBuilder, Result, Row, Value};
use observer::fire;

/// Column set on insert when the model has `timestamps`.
pub const CREATED_AT: &str = "created_at";
/// Column set on insert and update when the model has `timestamps`.
pub const UPDATED_AT: &str = "updated_at";
/// Column set by `delete` when the model has `soft_deletes`.
pub const DELETED_AT: &str = "deleted_at";

/// Bookkeeping that `#[rustique::model]` injects into every model as `__meta`:
/// whether the row exists and the column values last read or written, for
/// dirty tracking.
///
/// It never affects equality: two models with the same fields are equal.
#[derive(Clone, Default)]
pub struct Meta {
    exists: bool,
    /// Values in [`Model::COLUMNS`] order; empty until loaded or saved.
    original: Vec<Value>,
    /// Pivot columns of a `belongs_to_many` relation the model was loaded through.
    pivot: Option<Row>,
    /// Loaded together with other models, so a lazy relation query on it
    /// is likely one of many (N+1); see [`prevent_lazy_loading`].
    siblings: bool,
}

impl fmt::Debug for Meta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Meta")
            .field("exists", &self.exists)
            .finish_non_exhaustive()
    }
}

impl PartialEq for Meta {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for Meta {}

/// A database table mapped to a struct, the equivalent of an Eloquent model.
///
/// Implement it with `#[rustique::model]` rather than by hand:
///
/// ```
/// use rustique::prelude::*;
///
/// #[rustique::model(table = "users", timestamps)]
/// #[derive(Debug, Clone, Default)]
/// pub struct User {
///     pub id: i64,
///     pub email: String,
///     pub created_at: Option<chrono::NaiveDateTime>,
///     pub updated_at: Option<chrono::NaiveDateTime>,
/// }
///
/// assert_eq!(User::EMAIL, "email");
/// let sql = User::query().where_(User::EMAIL, "=", "a@x.io").to_sql(Dialect::MySql)?.sql;
/// assert_eq!(sql, "SELECT * FROM `users` WHERE `email` = ?");
/// # Ok::<(), rustique::Error>(())
/// ```
///
/// Put `#[rustique::model]` above `#[derive(...)]`: derives listed before it run
/// before the hidden `__meta` field exists and fail with "missing field `__meta`".
/// With serde's `Serialize` / `Deserialize` derived, `__meta` is skipped.
///
/// Macro options: `table = "..."` (required), `timestamps` (fills
/// `created_at` / `updated_at`), `soft_deletes` (`delete` sets `deleted_at`),
/// `incrementing = false` (the application sets the key, not the database),
/// `uuid` / `ulid` (a `String` key filled with an ordered UUID or a ULID on
/// insert when empty).
///
/// `morph_alias = "..."`: see [`MORPH_ALIAS`](Self::MORPH_ALIAS).
///
/// `hooks`: the model implements [`Hooks`] itself. `observers(A, B)`: the
/// [`Observer`]s `A` and `B` run after the hooks. `global_scopes(S)`: the
/// [`GlobalScope`] `S` filters every query.
///
/// `created_at = "..."`, `updated_at = "..."`, `deleted_at = "..."`: column
/// names for `timestamps` and `soft_deletes` (see [`CREATED_AT`](Self::CREATED_AT)).
///
/// Field options: `#[rustique(primary_key)]` (default: the `id` field; several
/// fields make a composite key), `#[rustique(column = "...")]` when the column
/// name differs from the field name (the column constant keeps the field's
/// name), and `#[rustique(sequence = "USERS_SEQ")]` on the key, so Oracle fills
/// it from a sequence.
///
/// # Mass assignment
///
/// Columns are guarded unless marked `#[rustique(fillable)]`. The macro generates
/// `UserFill`: one `Option` field per fillable column, and
/// `user.fill(UserFill { .. })`, which sets the fields that are `Some`. When
/// the model derives `Deserialize`, so does `UserFill`, with
/// `deny_unknown_fields`: request bodies naming a guarded field are rejected,
/// a missing key leaves the field unchanged and `null` sets a nullable one to
/// `None`. `User::FILLABLE` lists the same columns, for column names that
/// arrive at runtime, such as before [`Query::update`].
///
/// The primary key (also a `uuid` / `ulid` or hand-set one) and the
/// `timestamps` / `soft_deletes` columns cannot be fillable; set those
/// fields directly:
///
/// ```compile_fail
/// #[rustique::model(table = "users")]
/// #[derive(Debug, Clone, Default)]
/// struct User {
///     #[rustique(fillable)] // error: the primary key cannot be `fillable`
///     id: i64,
/// }
/// ```
///
/// `#[rustique(guarded)]` no longer exists, since guarded is the default:
///
/// ```compile_fail
/// #[rustique::model(table = "users")]
/// #[derive(Debug, Clone, Default)]
/// struct User {
///     id: i64,
///     #[rustique(guarded)] // error: columns are guarded unless marked `fillable`
///     is_admin: bool,
/// }
/// ```
///
/// Deserializing `User` itself reads every field, `is_admin` included, for
/// trusted data such as a cache; parse untrusted input into `UserFill`.
///
/// A `#[rustique::scopes]` scope named like a [`Query`] method would be hidden by
/// it in `User::query().latest()`, so the macro refuses it:
///
/// ```compile_fail
/// # #[rustique::model(table = "users")]
/// # #[derive(Debug, Clone, Default)]
/// # struct User { id: i64 }
/// #[rustique::scopes]
/// impl User {
///     fn scope_latest(q: rustique::Query<User>) -> rustique::Query<User> { q } // error: hidden by `Query::latest`
/// }
/// ```
///
/// # Relations
///
/// A [`Rel`] field with a relation option gets a method of the same name
/// returning a [`Query`] of the related models, and can be filled by
/// [`Query::with`]. Keys take one column or a list (`["a", "b"]`); defaults
/// follow Eloquent, using snake-cased struct names (`BlogPost` → `blog_post_id`):
///
/// | Option | Keys (default) |
/// |---|---|
/// | `has_one(R)`, `has_many(R)` | `foreign_key` on R (`parent_id`), `local_key` (parent key) |
/// | `belongs_to(R)` | `foreign_key` on the parent (`field_id`), `owner_key` (R's key) |
/// | `belongs_to_many(R)` | `table` (`r_parent`, alphabetical), `foreign_pivot_key` (`parent_id`), `related_pivot_key` (`r_id`), `parent_key`, `related_key`; `pivot = ["col"]` and `pivot_timestamps` (or `pivot_timestamps = ["created", "updated"]`) for [`pivot`](Model::pivot) |
/// | `has_many_through(R, through = T)`, `has_one_through(R, through = T)` | `first_key` on T (`parent_id`), `second_key` on R (`t_id`), `local_key`, `second_local_key` (T's key) |
/// | `morph_one(R, name = "n")`, `morph_many(R, name = "n")` | `n_type` and `n_id` on R, `local_key` (parent key); `n_type` holds the parent's [`MORPH_ALIAS`](Model::MORPH_ALIAS) |
/// | `morph_to_many(R, name = "n")` | as `belongs_to_many`, with `table` (`ns`), `foreign_pivot_key` (`n_id`), `related_pivot_key` (`r_id`); pivot rows carry `n_type` = the parent's [`MORPH_ALIAS`](Model::MORPH_ALIAS) |
/// | `morphed_by_many(R, name = "n")` | the inverse: `foreign_pivot_key` (`parent_id`), `related_pivot_key` (`n_id`), `n_type` = R's alias |
/// | `morph_to`, `morph_to(name = "n")` | `n_type` / `n_id` on the parent (`n`: the field name); the field is `Rel<Option<T>>` with `T` a [`MorphTo`] enum |
///
/// Related models must implement `Clone`: one related row can belong to
/// several parents. Two models that hold each other through `has_one` /
/// `belongs_to` need a `Box` on one side: `Rel<Option<Box<User>>>`.
///
/// A `morph_to` field's method is `async` and takes the executor:
/// `comment.commentable(&db).await?`, since the target type varies per row.
///
/// `belongs_to_many` (and `morph_to_many` / `morphed_by_many`) also adds
/// `field_pivot()`, a [`Pivot`] with `attach`, `detach`, `sync`, `toggle`,
/// ... A `#[rustique(count)] posts_count: Rel<u64>` field receives
/// [`Query::with_count("posts")`](Query::with_count); `#[rustique(aggregate)]`
/// fields receive [`with_sum`](Query::with_sum), [`with_exists`](Query::with_exists), ...
///
/// ```
/// use rustique::prelude::*;
///
/// #[rustique::model(table = "users")]
/// #[derive(Debug, Clone, Default)]
/// pub struct User {
///     pub id: i64,
///     #[rustique(has_many(Post))]
///     pub posts: Rel<Vec<Post>>,
/// }
///
/// #[rustique::model(table = "posts")]
/// #[derive(Debug, Clone, Default)]
/// pub struct Post {
///     pub id: i64,
///     pub user_id: i64,
/// }
///
/// let user = User { id: 7, ..Default::default() };
/// let sql = user.posts().to_sql(Dialect::MySql)?.sql;
/// assert_eq!(sql, "SELECT * FROM `posts` WHERE `posts`.`user_id` = ?");
/// # Ok::<(), rustique::Error>(())
/// ```
pub trait Model: Hooks + Sized + Send + Sync + 'static {
    /// Primary key: the key field's type, or a tuple for a composite key.
    type Key: Send;

    /// Table name.
    const TABLE: &'static str;
    /// Every column, in field order.
    const COLUMNS: &'static [&'static str];
    /// Primary key columns.
    const PRIMARY_KEY: &'static [&'static str];
    /// The database generates the (single) key: auto-increment, identity,
    /// trigger or [`SEQUENCE`](Self::SEQUENCE). Inserts then leave it out
    /// and read it back.
    const INCREMENTING: bool;
    /// Oracle sequence that fills the key on insert. MySQL ignores it.
    const SEQUENCE: Option<&'static str>;
    /// Fills `created_at` and `updated_at`.
    const TIMESTAMPS: bool;
    /// Value stored in the `*_type` column of polymorphic relations
    /// pointing at this model. Defaults to the table name; set it with
    /// `#[rustique::model(morph_alias = "...")]`, e.g. to the class name a
    /// Laravel application stored.
    const MORPH_ALIAS: &'static str = Self::TABLE;
    /// `delete` sets `deleted_at`, and queries skip deleted rows.
    const SOFT_DELETES: bool;
    /// Creation time column of `timestamps`: `created_at` unless set with
    /// `#[rustique::model(created_at = "...")]`.
    const CREATED_AT: &'static str = CREATED_AT;
    /// Update time column of `timestamps`: `updated_at` unless set with
    /// `#[rustique::model(updated_at = "...")]`.
    const UPDATED_AT: &'static str = UPDATED_AT;
    /// Deletion time column of `soft_deletes`: `deleted_at` unless set with
    /// `#[rustique::model(deleted_at = "...")]`.
    const DELETED_AT: &'static str = DELETED_AT;

    /// Builds the model from a row, moving the values out of it (see
    /// [`Row::take`]). The result is not marked as existing; queries do that.
    ///
    /// # Errors
    ///
    /// [`Error::ColumnNotFound`] or [`Error::Decode`] when a column is
    /// missing or does not fit its field.
    fn from_row(row: &mut Row) -> Result<Self>;

    /// Field values in [`COLUMNS`](Self::COLUMNS) order.
    fn to_values(&self) -> Vec<Value>;

    /// Sets the field of `column` from `value`.
    ///
    /// # Errors
    ///
    /// [`Error::ColumnNotFound`] for an unknown column, [`Error::Decode`]
    /// when the value does not fit the field.
    fn set_value(&mut self, column: &str, value: Value) -> Result<()>;

    /// Splits a key into one value per [`PRIMARY_KEY`](Self::PRIMARY_KEY) column.
    fn key_values(key: Self::Key) -> Vec<Value>;

    /// The injected [`Meta`].
    fn meta(&self) -> &Meta;

    /// The injected [`Meta`], mutably.
    fn meta_mut(&mut self) -> &mut Meta;

    /// The relation called `name`. Generated for models with relations.
    #[doc(hidden)]
    fn relation(name: &str) -> Option<Relation> {
        let _ = name;
        None
    }

    /// `[type, id]` columns of the `morph_to` relation `name`. Generated
    /// for models with `morph_to` fields.
    #[doc(hidden)]
    fn morph_columns(name: &str) -> Option<[&'static str; 2]> {
        let _ = name;
        None
    }

    /// The models the `morph_to` relation `name` can point at. Generated
    /// for models with `morph_to` fields.
    #[doc(hidden)]
    fn morph_targets(name: &str) -> Vec<MorphTarget> {
        let _ = name;
        Vec::new()
    }

    /// Eager-loads relation `name` into `models`, then `nested` relation
    /// paths on the related models. Generated for models with relations.
    #[doc(hidden)]
    fn load_relation<'a, E: Executor>(
        db: &'a E,
        models: &'a mut [Self],
        name: &'a str,
        nested: Nested,
    ) -> BoxFuture<'a, Result<()>> {
        let _ = (db, models, nested);
        Box::pin(async move {
            Err(Error::UnknownRelation {
                model: Self::TABLE,
                relation: name.to_owned(),
            })
        })
    }

    /// Runs `event` on the model's observers. Generated for models with `observers(...)`.
    #[doc(hidden)]
    fn fire_observers(
        &mut self,
        event: Event,
        db: &impl Executor,
    ) -> impl Future<Output = Result<()>> + Send {
        let _ = (event, db);
        async { Ok(()) }
    }

    /// A new key for `#[rustique::model(uuid)]` / `#[rustique::model(ulid)]` models,
    /// set on insert when the key is empty. Generated for those models.
    #[doc(hidden)]
    fn new_unique_id() -> Option<Value> {
        None
    }

    /// Applies the global scopes of `#[rustique::model(global_scopes(...))]`.
    #[doc(hidden)]
    fn apply_global_scopes(query: Query<Self>) -> Query<Self> {
        query
    }

    /// The pivot row of a model loaded through a `belongs_to_many`
    /// relation: the relation's `pivot` columns (and `created_at` /
    /// `updated_at` with `pivot_timestamps`), like Eloquent's `$role->pivot`.
    /// `None` for models loaded otherwise.
    fn pivot(&self) -> Option<&Row> {
        self.meta().pivot.as_ref()
    }

    /// Loads relation paths (`"posts.comments"`) into this model's [`Rel`] fields.
    /// For many models, use [`Collection::load`]: one query instead of one per model.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownRelation`], database and decode errors.
    fn load(
        &mut self,
        db: &impl Executor,
        relations: &str,
    ) -> impl Future<Output = Result<()>> + Send {
        let paths = vec![EagerLoad::path(relations)];
        async move { eager_load(db, std::slice::from_mut(self), &paths).await }
    }

    /// Starts a query on this model's table.
    fn query() -> Query<Self> {
        Query::new()
    }

    /// Every row of the table (without soft-deleted ones).
    ///
    /// # Errors
    ///
    /// Database and decode errors.
    fn all(db: &impl Executor) -> impl Future<Output = Result<Collection<Self>>> + Send {
        async move { Self::query().get(db).await }
    }

    /// The row with primary key `key`, if any.
    ///
    /// # Errors
    ///
    /// Database and decode errors.
    fn find(
        db: &impl Executor,
        key: Self::Key,
    ) -> impl Future<Output = Result<Option<Self>>> + Send {
        async move { Self::query().where_key(key).first(db).await }
    }

    /// The row with primary key `key`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when there is none, plus database and decode errors.
    fn find_or_fail(
        db: &impl Executor,
        key: Self::Key,
    ) -> impl Future<Output = Result<Self>> + Send {
        async move { Self::query().where_key(key).first_or_fail(db).await }
    }

    /// The rows with primary keys in `keys`, like Eloquent's `findMany`;
    /// missing keys are skipped. The order is the database's.
    ///
    /// # Errors
    ///
    /// Database and decode errors.
    fn find_many(
        db: &impl Executor,
        keys: impl IntoIterator<Item = Self::Key>,
    ) -> impl Future<Output = Result<Collection<Self>>> + Send {
        let query = Self::query().where_keys(keys);
        async move { query.get(db).await }
    }

    /// Inserts `model` and returns it with its generated key and timestamps.
    ///
    /// # Errors
    ///
    /// As [`save`](Self::save).
    fn create(db: &impl Executor, mut model: Self) -> impl Future<Output = Result<Self>> + Send {
        async move {
            model.save(db).await?;
            Ok(model)
        }
    }

    /// The first model whose columns equal `attributes`, or a new unsaved
    /// one with `attributes` and then `values` set, like Eloquent's `firstOrNew`.
    ///
    /// # Errors
    ///
    /// Database and decode errors, and [`Error::ColumnNotFound`] /
    /// [`Error::Decode`] when a column or value does not fit the model.
    fn first_or_new(
        db: &impl Executor,
        attributes: Vec<(&str, Value)>,
        values: Vec<(&str, Value)>,
    ) -> impl Future<Output = Result<Self>> + Send
    where
        Self: Default,
    {
        async move {
            let query = attributes.iter().fold(Self::query(), |q, (column, value)| {
                q.where_(*column, "=", value.clone())
            });
            if let Some(found) = query.first(db).await? {
                return Ok(found);
            }
            let mut model = Self::default();
            for (column, value) in attributes.into_iter().chain(values) {
                model.set_value(column, value)?;
            }
            Ok(model)
        }
    }

    /// As [`first_or_new`](Self::first_or_new), but saves the new model,
    /// like Eloquent's `firstOrCreate`. When a concurrent caller inserts the
    /// same row first, this returns that row instead, as
    /// [`create_or_first`](Self::create_or_first) does; that needs a unique
    /// index on the `attributes` columns.
    ///
    /// # Errors
    ///
    /// As [`first_or_new`](Self::first_or_new) and [`save`](Self::save).
    fn first_or_create(
        db: &impl Executor,
        attributes: Vec<(&str, Value)>,
        values: Vec<(&str, Value)>,
    ) -> impl Future<Output = Result<Self>> + Send
    where
        Self: Default,
    {
        async move {
            if let Some(found) = matching::<Self>(&attributes).first(db).await? {
                return Ok(found);
            }
            Ok(create_or_first(db, attributes, values).await?.0)
        }
    }

    /// Inserts a model with `attributes` and `values` set, or, when the
    /// insert hits a unique index (another caller inserted it first), returns
    /// the existing row matching `attributes`, like Eloquent's `createOrFirst`.
    /// Use it when the row usually does not exist yet.
    ///
    /// Without a unique index on the `attributes` columns, racing callers
    /// both insert. On a unique violation the `creating` hook has already
    /// run. No savepoint is needed: MySQL and Oracle undo only the failed
    /// statement, and the transaction carries on.
    ///
    /// # Errors
    ///
    /// As [`save`](Self::save); a unique violation only when no row matches
    /// `attributes` afterwards (the index covers other columns).
    fn create_or_first(
        db: &impl Executor,
        attributes: Vec<(&str, Value)>,
        values: Vec<(&str, Value)>,
    ) -> impl Future<Output = Result<Self>> + Send
    where
        Self: Default,
    {
        async move { Ok(create_or_first(db, attributes, values).await?.0) }
    }

    /// Finds the model whose columns equal `attributes` (or creates one with
    /// them), sets `values` and saves it, like Eloquent's `updateOrCreate`.
    /// A concurrent insert of the same row is updated instead, as in
    /// [`first_or_create`](Self::first_or_create).
    ///
    /// # Errors
    ///
    /// As [`first_or_create`](Self::first_or_create).
    fn update_or_create(
        db: &impl Executor,
        attributes: Vec<(&str, Value)>,
        values: Vec<(&str, Value)>,
    ) -> impl Future<Output = Result<Self>> + Send
    where
        Self: Default,
    {
        async move {
            let mut model = match matching::<Self>(&attributes).first(db).await? {
                Some(found) => found,
                None => {
                    let (model, created) = create_or_first(db, attributes, values.clone()).await?;
                    if created {
                        return Ok(model);
                    }
                    model
                }
            };
            for (column, value) in values {
                model.set_value(column, value)?;
            }
            model.save(db).await?;
            Ok(model)
        }
    }

    /// Adds `amount` to `column` in the database (`column = column + ?`,
    /// so concurrent increments add up) and in the model, plus `updated_at`
    /// with `timestamps`. Other changed fields are not saved. Runs the
    /// `updating` and `updated` [`Hooks`].
    ///
    /// # Errors
    ///
    /// Hook and database errors, and [`Error::InvalidQuery`] when the model
    /// was never saved or `column` or `amount` is not a number.
    fn increment(
        &mut self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value> + Send,
    ) -> impl Future<Output = Result<()>> + Send {
        let amount = amount.into();
        async move { increment(self, db, column, amount).await }
    }

    /// Subtracts `amount` from `column`; see [`increment`](Self::increment).
    ///
    /// # Errors
    ///
    /// As [`increment`](Self::increment).
    fn decrement(
        &mut self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value> + Send,
    ) -> impl Future<Output = Result<()>> + Send {
        let amount = crate::query::negate(amount.into());
        async move { increment(self, db, column, amount?).await }
    }

    /// Sets `updated_at` to now and saves the model (with any other
    /// changes), like Eloquent's `touch`. Does nothing without `timestamps`.
    ///
    /// # Errors
    ///
    /// As [`save`](Self::save).
    fn touch(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            if !Self::TIMESTAMPS {
                return Ok(());
            }
            self.set_value(Self::UPDATED_AT, now())?;
            self.save(db).await
        }
    }

    /// The model's row as it is now in the database, found by the key the
    /// model was loaded with; `None` when it is gone or was never saved.
    /// Soft-deleted rows and global scopes do not hide it. Relations are not loaded.
    ///
    /// # Errors
    ///
    /// Database and decode errors.
    fn fresh(&self, db: &impl Executor) -> impl Future<Output = Result<Option<Self>>> + Send {
        async move {
            if !self.exists() {
                return Ok(None);
            }
            let original = &self.meta().original;
            let mut query = Self::query().with_trashed().without_global_scopes();
            for key in Self::PRIMARY_KEY {
                let value = original[column_index::<Self>(key)].clone();
                query = query.where_(format!("{}.{key}", Self::TABLE), "=", value);
            }
            query.first(db).await
        }
    }

    /// Reloads the model's columns from the database, discarding unsaved
    /// changes. Relation fields keep what they hold.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when the row is gone or the model was never
    /// saved, plus database and decode errors.
    fn refresh(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            let fresh = self.fresh(db).await?.ok_or(Error::NotFound(Self::TABLE))?;
            for (column, value) in Self::COLUMNS.iter().zip(fresh.to_values()) {
                self.set_value(column, value)?;
            }
            *self.meta_mut() = fresh.meta().clone();
            Ok(())
        }
    }

    /// An unsaved copy of the model, like Eloquent's `replicate`: the key
    /// and timestamp columns are reset to their `Default` values, every
    /// other field (relations included) is cloned.
    #[must_use]
    fn replicate(&self) -> Self
    where
        Self: Clone + Default,
    {
        let mut copy = self.clone();
        let defaults = Self::default().to_values();
        let mut reset: Vec<&str> = Self::PRIMARY_KEY.to_vec();
        if Self::TIMESTAMPS {
            reset.extend([Self::CREATED_AT, Self::UPDATED_AT]);
        }
        for column in reset {
            let value = defaults[column_index::<Self>(column)].clone();
            copy.set_value(column, value)
                .expect("a field accepts its own default value");
        }
        *copy.meta_mut() = Meta::default();
        copy
    }

    /// Inserts the model if it does not exist yet, otherwise updates its
    /// changed columns. Does nothing when no column changed.
    ///
    /// Runs the [`Hooks`]: `saving`, then `creating` / `created` or (when a
    /// column changed) `updating` / `updated`, then `saved`.
    ///
    /// # Errors
    ///
    /// Hook and database errors, and [`Error::InvalidQuery`] when an
    /// incrementing model's insert generates no key.
    fn save(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            fire(self, Event::Saving, db).await?;
            if self.meta().exists {
                update(self, db).await?;
            } else {
                insert(self, db).await?;
            }
            fire(self, Event::Saved, db).await
        }
    }

    /// Deletes the row. With `soft_deletes`, sets `deleted_at` instead.
    /// Does nothing for a model that was never saved. Runs the `deleting`
    /// and `deleted` [`Hooks`].
    ///
    /// # Errors
    ///
    /// Hook and database errors.
    fn delete(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            if !self.meta().exists {
                return Ok(());
            }
            fire(self, Event::Deleting, db).await?;
            if Self::SOFT_DELETES {
                soft_delete(self, db).await?;
            } else {
                force_delete(self, db).await?;
            }
            fire(self, Event::Deleted, db).await
        }
    }

    /// Deletes the row for real, even with `soft_deletes`. Runs the
    /// `force_deleting`, `deleting`, `deleted` and `force_deleted` [`Hooks`].
    ///
    /// # Errors
    ///
    /// Hook and database errors.
    fn force_delete(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            if !self.meta().exists {
                return Ok(());
            }
            fire(self, Event::ForceDeleting, db).await?;
            fire(self, Event::Deleting, db).await?;
            force_delete(self, db).await?;
            fire(self, Event::Deleted, db).await?;
            fire(self, Event::ForceDeleted, db).await
        }
    }

    /// Inserts `models`, all or nothing, and updates them as [`save`](Self::save)
    /// would: generated keys, timestamps, [`exists`](Self::exists). No
    /// [`Hooks`] run.
    ///
    /// Without a generated key, or on MySQL with `innodb_autoinc_lock_mode`
    /// 0 or 1, this is one multi-row `INSERT`; MySQL then assigns consecutive
    /// ids (in steps of `auto_increment_increment`). On Oracle, every 100
    /// rows travel as one PL/SQL block of `INSERT ... RETURNING` statements,
    /// one round trip per block. On MySQL's default lock mode 2, where ids
    /// of one statement may interleave with concurrent inserts, each row is
    /// its own `INSERT` reading its key back, run through [`Executor::execute_all`].
    ///
    /// # Errors
    ///
    /// As [`save`](Self::save); on error no row is inserted.
    fn insert_many(
        db: &impl Executor,
        models: &mut [Self],
    ) -> impl Future<Output = Result<()>> + Send {
        async move {
            if models.is_empty() {
                return Ok(());
            }
            for model in models.iter_mut() {
                fill_unique_id(model)?;
            }
            if Self::TIMESTAMPS {
                let now = now();
                for model in models.iter_mut() {
                    model.set_value(Self::CREATED_AT, now.clone())?;
                    model.set_value(Self::UPDATED_AT, now.clone())?;
                }
            }
            let (columns, rows) = bulk_rows(models, &[], false);
            if Self::INCREMENTING {
                let key = Self::PRIMARY_KEY[0];
                let ids = insert_get_ids::<Self>(db, &columns, rows).await?;
                for (model, id) in models.iter_mut().zip(ids) {
                    model.set_value(key, Value::U64(id))?;
                }
            } else {
                let query = QueryBuilder::table(Self::TABLE);
                query.insert(db, &columns, rows).await?;
            }
            models.iter_mut().for_each(sync_original);
            Ok(())
        }
    }

    /// Inserts `models`, updating the `update` columns (plus `updated_at`
    /// with `timestamps`) of rows that clash on `unique_by`; see
    /// [`QueryBuilder::upsert`]. A generated key is only written when it is
    /// part of `unique_by`. Leaves `models` untouched and runs no [`Hooks`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::upsert`].
    fn upsert(
        db: &impl Executor,
        models: &[Self],
        unique_by: &[&str],
        update: &[&str],
    ) -> impl Future<Output = Result<u64>> + Send {
        async move {
            let (columns, rows) = bulk_rows(models, unique_by, true);
            let mut update = update.to_vec();
            if Self::TIMESTAMPS && !update.contains(&Self::UPDATED_AT) {
                update.push(Self::UPDATED_AT);
            }
            let query = QueryBuilder::table(Self::TABLE);
            query.upsert(db, &columns, rows, unique_by, &update).await
        }
    }

    /// Clears `deleted_at` and saves the model. Runs the `restoring` and
    /// `restored` [`Hooks`] around those of [`save`](Self::save).
    ///
    /// # Errors
    ///
    /// As [`save`](Self::save).
    fn restore(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async move {
            fire(self, Event::Restoring, db).await?;
            self.set_value(Self::DELETED_AT, Value::Null)?;
            self.save(db).await?;
            fire(self, Event::Restored, db).await
        }
    }

    /// Whether the model is soft-deleted.
    fn trashed(&self) -> bool {
        Self::SOFT_DELETES && !self.to_values()[column_index::<Self>(Self::DELETED_AT)].is_null()
    }

    /// Whether the row exists in the database: loaded, or saved by this model.
    fn exists(&self) -> bool {
        self.meta().exists
    }

    /// Columns changed since the model was loaded or saved, with their new
    /// values. A model that was never saved has every column dirty.
    fn dirty(&self) -> Vec<(&'static str, Value)> {
        let original = &self.meta().original;
        Self::COLUMNS
            .iter()
            .zip(self.to_values())
            .enumerate()
            .filter(|(i, (_, value))| !original.get(*i).is_some_and(|o| same(o, value)))
            .map(|(_, (column, value))| (*column, value))
            .collect()
    }

    /// Whether any column changed since the model was loaded or saved.
    fn is_dirty(&self) -> bool {
        !self.dirty().is_empty()
    }

    /// Value of `column` when the model was loaded or last saved.
    fn get_original(&self, column: &str) -> Option<&Value> {
        let index = Self::COLUMNS.iter().position(|c| *c == column)?;
        self.meta().original.get(index)
    }
}

/// Lifecycle hooks of a model, the equivalent of Eloquent model events.
///
/// Every hook does nothing by default; one that returns `Err` aborts the
/// operation with that error. `#[rustique::model]` implements this trait with the
/// defaults unless the model has the `hooks` option, in which case implement
/// it yourself:
///
/// ```
/// use rustique::prelude::*;
///
/// #[rustique::model(table = "users", hooks)]
/// #[derive(Debug, Clone, Default)]
/// pub struct User {
///     pub id: i64,
///     pub email: String,
/// }
///
/// impl rustique::Hooks for User {
///     async fn saving(&mut self, _db: &impl Executor) -> rustique::Result<()> {
///         self.email = self.email.trim().to_lowercase();
///         Ok(())
///     }
/// }
/// ```
///
/// Hooks run around [`Model::save`], [`Model::delete`],
/// [`Model::force_delete`] and [`Model::restore`] (and so `create`), not
/// around queries that change many rows at once.
#[allow(unused_variables)]
pub trait Hooks: Send {
    /// Before an insert or update.
    fn saving(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an insert or update, and after a `save` with nothing to update.
    fn saved(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before an insert.
    fn creating(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an insert; the generated key is set.
    fn created(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before an update of changed columns.
    fn updating(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After an update.
    fn updated(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before a delete or soft delete.
    fn deleting(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After a delete or soft delete.
    fn deleted(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before [`Model::force_delete`], ahead of `deleting`.
    fn force_deleting(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After [`Model::force_delete`], following `deleted`.
    fn force_deleted(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// Before [`Model::restore`], ahead of `saving`.
    fn restoring(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }

    /// After [`Model::restore`], following `saved`.
    fn restored(&mut self, db: &impl Executor) -> impl Future<Output = Result<()>> + Send {
        async { Ok(()) }
    }
}

/// Loads relation `paths` into `models`, one query per relation. Paths that
/// share a first segment (`posts.comments`, `posts.tags`) load it once.
pub(crate) async fn eager_load<M: Model>(
    db: &impl Executor,
    models: &mut [M],
    paths: &[EagerLoad],
) -> Result<()> {
    if models.is_empty() {
        return Ok(());
    }
    let mut relations: Vec<(&str, Nested)> = Vec::new();
    for load in paths {
        let (name, rest) = match load.path.split_once('.') {
            Some((name, rest)) => (name, Some(rest)),
            None => (load.path.as_str(), None),
        };
        let index = match relations.iter().position(|(n, _)| *n == name) {
            Some(index) => index,
            None => {
                relations.push((name, Nested::default()));
                relations.len() - 1
            }
        };
        let nested = &mut relations[index].1;
        match rest {
            Some(rest) => nested.paths.push(EagerLoad {
                path: rest.to_owned(),
                ..load.clone()
            }),
            None => {
                if !load.morph_counts.is_empty() && M::morph_columns(name).is_none() {
                    return Err(Error::InvalidQuery(format!(
                        "`with_morph_count`: `{name}` of `{}` is not a morph_to relation",
                        M::TABLE
                    )));
                }
                nested.constraints.push(load.clone());
            }
        }
    }
    for (name, nested) in relations {
        M::load_relation(db, models, name, nested).await?;
    }
    Ok(())
}

/// Builds a model from a row and marks it as loaded.
pub(crate) fn hydrate<M: Model>(mut row: Row) -> Result<M> {
    row.set_model(M::TABLE);
    let mut model = M::from_row(&mut row)?;
    sync_original(&mut model);
    Ok(model)
}

/// Whether a column is unchanged. JSON compares as documents, so text that
/// differs only in key order (a rebuilt `HashMap`) is not a change.
fn same(original: &Value, current: &Value) -> bool {
    match (original, current) {
        (Value::Json(a), Value::Json(b)) if a != b => {
            let parse = |s: &str| serde_json::from_str::<serde_json::Value>(s).ok();
            matches!((parse(a), parse(b)), (Some(a), Some(b)) if a == b)
        }
        _ => original == current,
    }
}

fn sync_original<M: Model>(model: &mut M) {
    let original = model.to_values();
    let meta = model.meta_mut();
    meta.exists = true;
    meta.original = original;
}

/// Current time in the application time zone, truncated to the
/// microseconds both databases store.
pub(crate) fn now() -> Value {
    Value::DateTime(crate::now().trunc_subsecs(6))
}

/// `#[rustique::model]` checks that the timestamp, soft-delete and key columns exist.
fn column_index<M: Model>(column: &str) -> usize {
    M::COLUMNS
        .iter()
        .position(|c| *c == column)
        .unwrap_or_else(|| panic!("model `{}` has no `{column}` column", M::TABLE))
}

/// The model's row, found by its key as it was loaded, so a changed key still
/// updates the right row.
fn key_query<M: Model>(model: &M) -> QueryBuilder {
    let original = &model.meta().original;
    M::PRIMARY_KEY
        .iter()
        .fold(QueryBuilder::table(M::TABLE), |query, key| {
            query.where_(*key, "=", original[column_index::<M>(key)].clone())
        })
}

/// Inserts `rows` and returns the key generated for each; see [`Model::insert_many`].
async fn insert_get_ids<M: Model>(
    db: &impl Executor,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
) -> Result<Vec<u64>> {
    let no_key = || {
        Error::InvalidQuery(format!(
            "insert into `{}` generated no key; use `incrementing = false` \
             if the application sets it",
            M::TABLE
        ))
    };
    if let Some(step) = mysql_consecutive_ids(db, rows.len()).await? {
        // Ids are consecutive within each statement, not across them.
        let mut left = rows.len();
        let size = grammar::rows_per_statement(db.dialect(), columns.len());
        let query = QueryBuilder::table(M::TABLE);
        let mut ids = Vec::with_capacity(left);
        for executed in query.insert_batches(db, columns, rows).await? {
            let first = executed.last_insert_id.ok_or_else(no_key)?;
            let count = left.min(size);
            left -= count;
            ids.extend((0..count as u64).map(|i| first + i * step));
        }
        return Ok(ids);
    }
    let key = M::PRIMARY_KEY[0];
    if db.dialect() == Dialect::Oracle {
        // One PL/SQL block per batch of rows, each `INSERT` reporting its key.
        let size = grammar::ORACLE_BLOCK_ROWS.min(grammar::MAX_BINDS / (columns.len() + 1));
        let mut rows = rows.into_iter();
        let mut blocks = Vec::new();
        loop {
            let batch: Vec<_> = rows.by_ref().take(size.max(1)).collect();
            if batch.is_empty() {
                break;
            }
            let block =
                grammar::insert_get_ids(db.dialect(), M::TABLE, columns, batch, key, M::SEQUENCE)?;
            blocks.push(block);
        }
        let results = match blocks.len() {
            1 => vec![db.fetch_all(blocks.remove(0)).await?],
            _ => db.fetch_each(blocks).await?,
        };
        return results
            .into_iter()
            .flatten()
            .map(|row| row.get::<Option<u64>>("id")?.ok_or_else(no_key))
            .collect();
    }
    let queries = rows
        .into_iter()
        .map(|row| grammar::insert_get_id(db.dialect(), M::TABLE, columns, row, key, M::SEQUENCE))
        .collect::<Result<Vec<_>>>()?;
    db.execute_all(queries)
        .await?
        .into_iter()
        .map(|executed| executed.last_insert_id.ok_or_else(no_key))
        .collect()
}

/// `Some(auto_increment_increment)` when one multi-row MySQL `INSERT` of
/// `rows` rows gets consecutive ids: lock modes 0 and 1 guarantee it, the
/// default mode 2 only without concurrent bulk inserts.
async fn mysql_consecutive_ids(db: &impl Executor, rows: usize) -> Result<Option<u64>> {
    if db.dialect() != Dialect::MySql || rows < 2 {
        return Ok(None);
    }
    let sql = "SELECT @@innodb_autoinc_lock_mode AS lock_mode, \
               @@auto_increment_increment AS step";
    let settings = db.erase().on_writer().select(sql, []).await?;
    let Some(row) = settings.first() else {
        return Ok(None);
    };
    let lock_mode: u64 = row.get("lock_mode")?;
    (lock_mode < 2).then(|| row.get("step")).transpose()
}

/// Columns and rows for a bulk insert, without a generated key unless it is
/// in `keep`. `stamp` fills `created_at` and `updated_at` with the current time.
fn bulk_rows<M: Model>(
    models: &[M],
    keep: &[&str],
    stamp: bool,
) -> (Vec<&'static str>, Vec<Vec<Value>>) {
    let generated = M::INCREMENTING && !keep.contains(&M::PRIMARY_KEY[0]);
    let skip = generated.then(|| column_index::<M>(M::PRIMARY_KEY[0]));
    let stamps: Vec<usize> = if M::TIMESTAMPS && stamp {
        vec![
            column_index::<M>(M::CREATED_AT),
            column_index::<M>(M::UPDATED_AT),
        ]
    } else {
        Vec::new()
    };
    let now = now();
    let keep = |i: &usize| Some(*i) != skip;
    let columns = (0..M::COLUMNS.len())
        .filter(keep)
        .map(|i| M::COLUMNS[i])
        .collect();
    let rows = models
        .iter()
        .map(|model| {
            let mut values = model.to_values();
            for &i in &stamps {
                values[i] = now.clone();
            }
            values
                .into_iter()
                .enumerate()
                .filter(|(i, _)| keep(i))
                .map(|(_, value)| value)
                .collect()
        })
        .collect();
    (columns, rows)
}

async fn soft_delete<M: Model>(model: &mut M, db: &impl Executor) -> Result<()> {
    let mut columns = vec![M::DELETED_AT];
    if M::TIMESTAMPS {
        columns.push(M::UPDATED_AT);
    }
    let now = now();
    for column in &columns {
        model.set_value(column, now.clone())?;
    }
    let values = columns.iter().map(|c| (*c, now.clone())).collect();
    key_query(model).update(db, values).await?;
    // Other unsaved changes stay dirty, as in Eloquent.
    for column in columns {
        let index = column_index::<M>(column);
        model.meta_mut().original[index] = now.clone();
    }
    Ok(())
}

async fn force_delete<M: Model>(model: &mut M, db: &impl Executor) -> Result<()> {
    key_query(model).delete(db).await?;
    model.meta_mut().exists = false;
    Ok(())
}

async fn insert<M: Model>(model: &mut M, db: &impl Executor) -> Result<()> {
    // Before the hooks, which may want the key; as Eloquent's `HasUuids`.
    fill_unique_id(model)?;
    fire(model, Event::Creating, db).await?;
    if M::TIMESTAMPS {
        let now = now();
        model.set_value(M::CREATED_AT, now.clone())?;
        model.set_value(M::UPDATED_AT, now)?;
    }
    let generated = M::INCREMENTING.then(|| M::PRIMARY_KEY[0]);
    let (columns, row): (Vec<&str>, Vec<Value>) = M::COLUMNS
        .iter()
        .copied()
        .zip(model.to_values())
        .filter(|(column, _)| Some(*column) != generated)
        .unzip();

    if let Some(key) = generated {
        let compiled =
            grammar::insert_get_id(db.dialect(), M::TABLE, &columns, row, key, M::SEQUENCE)?;
        let id = db.execute(compiled).await?.last_insert_id.ok_or_else(|| {
            Error::InvalidQuery(format!(
                "insert into `{}` generated no key; use `incrementing = false` \
                 if the application sets it",
                M::TABLE
            ))
        })?;
        model.set_value(key, Value::U64(id))?;
    } else {
        QueryBuilder::table(M::TABLE)
            .insert(db, &columns, vec![row])
            .await?;
    }
    sync_original(model);
    fire(model, Event::Created, db).await
}

/// Sets a new UUID / ULID key when the model has one and its key is NULL or empty.
fn fill_unique_id<M: Model>(model: &mut M) -> Result<()> {
    let Some(id) = M::new_unique_id() else {
        return Ok(());
    };
    let key = M::PRIMARY_KEY[0];
    let empty = match &model.to_values()[column_index::<M>(key)] {
        Value::Null => true,
        Value::String(s) => s.is_empty(),
        _ => false,
    };
    if empty {
        model.set_value(key, id)?;
    }
    Ok(())
}

/// An ordered UUID (version 7, time-ordered), like Laravel's `HasUuids`.
#[doc(hidden)]
pub fn new_uuid() -> Value {
    uuid::Uuid::now_v7().to_string().into()
}

/// A ULID, lowercase like Laravel's `HasUlids`: the 128 bits of a version 7
/// UUID (48-bit millisecond time, then random bits) in Crockford base 32, so
/// it sorts by creation time. The UUID's 6 version and variant bits stay fixed.
#[doc(hidden)]
pub fn new_ulid() -> Value {
    ulid_string(uuid::Uuid::now_v7().as_u128()).into()
}

fn ulid_string(bits: u128) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
    // 26 characters of 5 bits: 130 bits, the first character holds the top 3.
    (0..26)
        .rev()
        .map(|i| ALPHABET[((bits >> (i * 5)) & 0x1f) as usize] as char)
        .collect()
}

/// Models whose columns equal `attributes`.
fn matching<M: Model>(attributes: &[(&str, Value)]) -> Query<M> {
    (attributes.iter()).fold(M::query(), |q, (column, value)| {
        q.where_(*column, "=", value.clone())
    })
}

/// [`Model::create_or_first`], also telling whether the model was inserted.
async fn create_or_first<M: Model + Default>(
    db: &impl Executor,
    attributes: Vec<(&str, Value)>,
    values: Vec<(&str, Value)>,
) -> Result<(M, bool)> {
    let mut model = M::default();
    for (column, value) in attributes.iter().cloned().chain(values) {
        model.set_value(column, value)?;
    }
    match model.save(db).await {
        Ok(()) => Ok((model, true)),
        Err(error) if error.is_unique_violation() => {
            let mut query = matching::<M>(&attributes);
            if db.dialect() == Dialect::MySql {
                // A locking read sees rows committed after the transaction's
                // snapshot (REPEATABLE READ); Oracle reads committed rows anyway.
                query = query.shared_lock();
            }
            // The main database: a replica may not have the winner's row yet.
            match query.first(&db.erase().on_writer()).await? {
                Some(found) => Ok((found, false)),
                None => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

async fn increment<M: Model>(
    model: &mut M,
    db: &impl Executor,
    column: &str,
    amount: Value,
) -> Result<()> {
    if !model.exists() {
        return Err(Error::InvalidQuery(format!(
            "save the `{}` model before incrementing it",
            M::TABLE
        )));
    }
    let index = M::COLUMNS
        .iter()
        .position(|c| *c == column)
        .ok_or_else(|| Error::ColumnNotFound(column.to_owned()))?;
    let current = model.to_values().swap_remove(index);
    let sum = add(&current, &amount).ok_or_else(|| {
        Error::InvalidQuery(format!("cannot add {amount:?} to `{column}` ({current:?})"))
    })?;
    fire(model, Event::Updating, db).await?;
    let mut extra = Vec::new();
    if M::TIMESTAMPS {
        let now = now();
        model.set_value(M::UPDATED_AT, now.clone())?;
        extra.push((M::UPDATED_AT, now));
    }
    key_query(model)
        .increment(db, column, amount, extra.clone())
        .await?;
    model.set_value(column, sum)?;
    // The field's own `Value` (`U64` for a `u32`, ...), so the model is not dirty.
    let stored = model.to_values().swap_remove(index);
    let original = &mut model.meta_mut().original;
    original[index] = stored;
    for (column, value) in extra {
        original[column_index::<M>(column)] = value;
    }
    fire(model, Event::Updated, db).await
}

/// `a + b` for numbers of any [`Value`] variant; `None` for other values.
fn add(a: &Value, b: &Value) -> Option<Value> {
    use crate::value::integer;
    use rust_decimal::Decimal;
    let decimal = |v: &Value| match v {
        Value::Decimal(d) => Some(*d),
        Value::F64(f) => Decimal::try_from(*f).ok(),
        other => integer(other).and_then(|n| Decimal::try_from_i128_with_scale(n, 0).ok()),
    };
    match (a, b) {
        (Value::Bool(_), _) | (_, Value::Bool(_)) => None,
        (Value::F64(x), y) | (y, Value::F64(x)) => {
            let y = match y {
                Value::F64(y) => *y,
                other => f64::try_from(decimal(other)?).ok()?,
            };
            Some(Value::F64(x + y))
        }
        _ => match (integer(a), integer(b)) {
            (Some(x), Some(y)) => {
                let sum = x.checked_add(y)?;
                Some(i64::try_from(sum).map_or_else(
                    |_| {
                        Value::Decimal(
                            Decimal::try_from_i128_with_scale(sum, 0).unwrap_or_default(),
                        )
                    },
                    Value::I64,
                ))
            }
            _ => Some(Value::Decimal(decimal(a)?.checked_add(decimal(b)?)?)),
        },
    }
}

async fn update<M: Model>(model: &mut M, db: &impl Executor) -> Result<()> {
    if !model.is_dirty() {
        return Ok(());
    }
    fire(model, Event::Updating, db).await?;
    if M::TIMESTAMPS {
        model.set_value(M::UPDATED_AT, now())?;
    }
    let dirty = model.dirty();
    key_query(model).update(db, dirty.clone()).await?;
    // The other columns of `original` are already current.
    for (column, value) in dirty {
        model.meta_mut().original[column_index::<M>(column)] = value;
    }
    fire(model, Event::Updated, db).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_key_order_is_not_a_change() {
        let json = |s: &str| Value::Json(s.to_owned());
        assert!(same(&json(r#"{"a":1,"b":2}"#), &json(r#"{"b":2,"a":1}"#)));
        assert!(!same(&json(r#"{"a":1,"b":2}"#), &json(r#"{"a":1,"b":3}"#)));
    }

    #[test]
    fn ulids_are_crockford_and_ordered() {
        assert_eq!(ulid_string(0), "0".repeat(26));
        assert_eq!(ulid_string(u128::MAX), format!("7{}", "z".repeat(25)));
        let (Value::String(a), Value::String(b)) = (new_ulid(), new_ulid()) else {
            panic!("ULIDs are strings");
        };
        assert_eq!(a.len(), 26);
        assert!(a <= b || a[..10] == b[..10], "{a} then {b}");
        let Value::String(uuid) = new_uuid() else {
            panic!("UUIDs are strings");
        };
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "7");
    }
}
