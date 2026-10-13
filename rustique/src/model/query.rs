//! [`Query`]: a [`QueryBuilder`] bound to a model.

use std::any::TypeId;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

use super::{Model, eager_load, hydrate, now};
use crate::db::Executor;
use crate::query::{Aggregate, Clause, Condition, Conjunction, Expr, Order};
use crate::{
    Chunks, Collection, Compiled, Cursor, CursorPage, Dialect, Error, FromValue, JoinClause, Page,
    QueryBuilder, Result, Row, SimplePage, Value,
};

/// A query on a model's table that returns models, from [`Model::query`].
///
/// Every [`QueryBuilder`] method is available. For models with
/// `soft_deletes`, deleted rows are left out unless
/// [`with_trashed`](Self::with_trashed) or [`only_trashed`](Self::only_trashed)
/// is used.
#[must_use]
pub struct Query<M> {
    pub(super) builder: QueryBuilder,
    trashed: Trashed,
    /// Relation paths to eager-load.
    pub(super) eager: Vec<EagerLoad>,
    /// Columns read into each model's [`pivot`](super::Model::pivot) row:
    /// `(selected alias, pivot column)`.
    pub(super) pivot: Vec<(String, &'static str)>,
    scopes: Scopes,
    model: PhantomData<fn() -> M>,
}

/// Which global scopes a query applies.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Scopes {
    All,
    Without(Vec<TypeId>),
    None,
}

/// A filter that every query of a model applies, like an Eloquent global
/// scope: register it with `#[rustique::model(global_scopes(Tenant, ...))]`.
/// Leave it out of one query with [`Query::without_global_scope`].
///
/// One scope can serve every model, for example tenant filtering:
///
/// ```
/// use rustique::prelude::*;
/// use rustique::{GlobalScope, Query};
///
/// pub struct Tenant;
///
/// impl<M: Model> GlobalScope<M> for Tenant {
///     fn apply(query: Query<M>) -> Query<M> {
///         let tenant = 7; // e.g. from a tokio task-local set per request
///         query.where_(format!("{}.tenant_id", M::TABLE), "=", tenant)
///     }
/// }
///
/// #[rustique::model(table = "posts", global_scopes(Tenant))]
/// #[derive(Debug, Clone, Default)]
/// pub struct Post {
///     pub id: i64,
///     pub tenant_id: i64,
/// }
///
/// let sql = Post::query().where_("id", ">", 1).to_sql(Dialect::MySql)?.sql;
/// assert_eq!(sql, "SELECT * FROM `posts` WHERE `id` > ? AND `posts`.`tenant_id` = ?");
/// let all = Post::query().without_global_scope::<Tenant>().to_sql(Dialect::MySql)?.sql;
/// assert_eq!(all, "SELECT * FROM `posts`");
/// # Ok::<(), rustique::Error>(())
/// ```
///
/// Scopes apply when the query runs (or compiles), after its own
/// conditions, which stay grouped as with local scopes. They apply to
/// relation queries, eager loads, `where_has` and bulk `update` / `delete`
/// through [`Query`], but not to [`Model::save`](super::Model::save) and
/// friends, which address one row by its key.
pub trait GlobalScope<M: Model>: 'static {
    /// Adds the scope's conditions.
    fn apply(query: Query<M>) -> Query<M>;
}

/// Changes the eager-load query of one relation; see [`Query::with_where`].
pub(crate) type Constraint = Arc<dyn Fn(QueryBuilder) -> QueryBuilder + Send + Sync>;

/// A relation path to eager-load, maybe with a constraint on its last relation.
#[derive(Clone)]
pub(crate) struct EagerLoad {
    pub path: String,
    pub constraint: Option<Constraint>,
    /// Columns of `with("posts:id,title")`, read from the last relation.
    pub columns: Option<Vec<String>>,
    /// `(morph alias, relation)`: counts on the models a `morph_to` relation
    /// at the end of `path` loads; see [`Query::with_morph_count`].
    pub morph_counts: Vec<(String, String)>,
}

impl EagerLoad {
    /// `path` without a constraint; `"posts.comments:id,body"` selects the
    /// comments' columns, like Eloquent's `with('posts.comments:id,body')`.
    pub(crate) fn path(path: impl Into<String>) -> Self {
        let path = path.into();
        let (path, columns) = match path.split_once(':') {
            Some((path, columns)) => (
                path.to_owned(),
                Some(columns.split(',').map(|c| c.trim().to_owned()).collect()),
            ),
            None => (path, None),
        };
        Self {
            path,
            constraint: None,
            columns,
            morph_counts: Vec::new(),
        }
    }
}

impl fmt::Debug for EagerLoad {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EagerLoad")
            .field("path", &self.path)
            .field("constrained", &self.constraint.is_some())
            .field("columns", &self.columns)
            .field("morph_counts", &self.morph_counts)
            .finish()
    }
}

/// What to load after one relation: constraints on its own query and the
/// relation paths below it. Passed through code that `#[rustique::model]` generates.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct Nested {
    pub(crate) constraints: Vec<EagerLoad>,
    pub(crate) paths: Vec<EagerLoad>,
}

impl Nested {
    /// Adds the `with_morph_count` counts meant for models of `R` to `query`.
    pub(crate) fn morph_counts<R: Model>(&self, query: Query<R>) -> Query<R> {
        (self.constraints.iter())
            .flat_map(|load| &load.morph_counts)
            .filter(|(alias, _)| alias == R::MORPH_ALIAS)
            .fold(query, |query, (_, relation)| query.with_count(relation))
    }
}

impl Nested {
    /// Columns chosen with `with("relation:a,b")`, if any.
    pub(crate) fn columns(&self) -> Option<&[String]> {
        self.constraints
            .iter()
            .find_map(|load| load.columns.as_deref())
    }

    /// Applies the constraints to the related query, each grouped like a scope.
    pub(crate) fn constrain(&self, builder: QueryBuilder) -> QueryBuilder {
        self.constraints
            .iter()
            .filter_map(|load| load.constraint.as_ref())
            .fold(builder, |builder, constraint| {
                grouped(builder, |b| constraint(b))
            })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trashed {
    Without,
    With,
    Only,
}

impl<M> Clone for Query<M> {
    fn clone(&self) -> Self {
        Self {
            builder: self.builder.clone(),
            trashed: self.trashed,
            eager: self.eager.clone(),
            pivot: self.pivot.clone(),
            scopes: self.scopes.clone(),
            model: PhantomData,
        }
    }
}

impl<M> fmt::Debug for Query<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Query")
            .field("builder", &self.builder)
            .field("trashed", &self.trashed)
            .field("eager", &self.eager)
            .field("pivot", &self.pivot)
            .field("scopes", &self.scopes)
            .finish()
    }
}

/// Forwards builder methods to the inner [`QueryBuilder`].
macro_rules! delegate {
    ($($name:ident($($arg:ident: $ty:ty),*);)*) => {$(
        #[doc = concat!("See [`QueryBuilder::", stringify!($name), "`].")]
        pub fn $name(mut self, $($arg: $ty),*) -> Self {
            self.builder = self.builder.$name($($arg),*);
            self
        }
    )*};
}

impl<M: Model> Query<M> {
    pub(super) fn new() -> Self {
        Self {
            builder: QueryBuilder::table(M::TABLE),
            trashed: Trashed::Without,
            eager: Vec::new(),
            pivot: Vec::new(),
            scopes: Scopes::All,
            model: PhantomData,
        }
    }

    /// Leaves out global scope `S` (see [`GlobalScope`]).
    pub fn without_global_scope<S: GlobalScope<M>>(mut self) -> Self {
        let id = TypeId::of::<S>();
        match &mut self.scopes {
            Scopes::All => self.scopes = Scopes::Without(vec![id]),
            Scopes::Without(ids) => ids.push(id),
            Scopes::None => {}
        }
        self
    }

    /// Leaves out every global scope (see [`GlobalScope`]). The soft-delete
    /// filter stays; use [`with_trashed`](Self::with_trashed) for that.
    pub fn without_global_scopes(mut self) -> Self {
        self.scopes = Scopes::None;
        self
    }

    /// Applies global scope `S` unless it was left out. Called by code that
    /// `#[rustique::model(global_scopes(...))]` generates.
    #[doc(hidden)]
    pub fn apply_global_scope<S: GlobalScope<M>>(self) -> Self {
        let skip = match &self.scopes {
            Scopes::All => false,
            Scopes::Without(ids) => ids.contains(&TypeId::of::<S>()),
            Scopes::None => true,
        };
        if skip { self } else { scope(self, S::apply) }
    }

    /// See [`QueryBuilder::when`].
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition { build(self) } else { self }
    }

    /// Newest first: descending by the model's `created_at` column.
    pub fn latest(self) -> Self {
        self.order_by_desc(format!("{}.{}", M::TABLE, M::CREATED_AT))
    }

    /// Oldest first: ascending by the model's `created_at` column.
    pub fn oldest(self) -> Self {
        self.order_by(format!("{}.{}", M::TABLE, M::CREATED_AT))
    }

    /// See [`QueryBuilder::union`]. `query`'s soft-delete filter and global
    /// scopes apply to its part.
    pub fn union(mut self, query: Self) -> Self {
        self.builder = self.builder.union(query.into_builder());
        self
    }

    /// See [`QueryBuilder::union_all`].
    pub fn union_all(mut self, query: Self) -> Self {
        self.builder = self.builder.union_all(query.into_builder());
        self
    }

    delegate! {
        select(columns: impl IntoIterator<Item: Into<String>>);
        add_select(columns: impl IntoIterator<Item: Into<String>>);
        select_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        select_sub(query: QueryBuilder, alias: impl Into<String>);
        distinct();
        where_(column: impl Into<String>, op: &str, value: impl Into<Value>);
        or_where(column: impl Into<String>, op: &str, value: impl Into<Value>);
        where_column(first: impl Into<String>, op: &str, second: impl Into<String>);
        or_where_column(first: impl Into<String>, op: &str, second: impl Into<String>);
        where_in(column: impl Into<String>, values: impl IntoIterator<Item: Into<Value>>);
        or_where_in(column: impl Into<String>, values: impl IntoIterator<Item: Into<Value>>);
        where_not_in(column: impl Into<String>, values: impl IntoIterator<Item: Into<Value>>);
        or_where_not_in(column: impl Into<String>, values: impl IntoIterator<Item: Into<Value>>);
        where_in_sub(column: impl Into<String>, query: QueryBuilder);
        where_not_in_sub(column: impl Into<String>, query: QueryBuilder);
        where_null(column: impl Into<String>);
        or_where_null(column: impl Into<String>);
        where_not_null(column: impl Into<String>);
        or_where_not_null(column: impl Into<String>);
        where_between(column: impl Into<String>, low: impl Into<Value>, high: impl Into<Value>);
        where_date(column: impl Into<String>, op: &str, value: chrono::NaiveDate);
        where_json(path: &str, op: &str, value: impl Into<Value>);
        or_where_json(path: &str, op: &str, value: impl Into<Value>);
        where_not_between(column: impl Into<String>, low: impl Into<Value>, high: impl Into<Value>);
        where_exists(query: QueryBuilder);
        where_not_exists(query: QueryBuilder);
        where_group(build: impl FnOnce(QueryBuilder) -> QueryBuilder);
        or_where_group(build: impl FnOnce(QueryBuilder) -> QueryBuilder);
        where_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        or_where_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        join(table: impl Into<String>, first: impl Into<String>, op: &str, second: impl Into<String>);
        left_join(table: impl Into<String>, first: impl Into<String>, op: &str, second: impl Into<String>);
        right_join(table: impl Into<String>, first: impl Into<String>, op: &str, second: impl Into<String>);
        cross_join(table: impl Into<String>);
        join_with(table: impl Into<String>, build: impl FnOnce(JoinClause) -> JoinClause);
        left_join_with(table: impl Into<String>, build: impl FnOnce(JoinClause) -> JoinClause);
        right_join_with(table: impl Into<String>, build: impl FnOnce(JoinClause) -> JoinClause);
        join_sub(query: QueryBuilder, alias: impl Into<String>, first: impl Into<String>, op: &str, second: impl Into<String>);
        left_join_sub(query: QueryBuilder, alias: impl Into<String>, first: impl Into<String>, op: &str, second: impl Into<String>);
        group_by(columns: impl IntoIterator<Item: Into<String>>);
        group_by_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        having(column: impl Into<String>, op: &str, value: impl Into<Value>);
        or_having(column: impl Into<String>, op: &str, value: impl Into<Value>);
        having_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        order_by(column: impl Into<String>);
        order_by_desc(column: impl Into<String>);
        order_by_raw(sql: impl Into<String>, binds: impl IntoIterator<Item = Value>);
        limit(limit: u64);
        offset(offset: u64);
        for_page(page: u64, per_page: u64);
        lock_for_update();
        shared_lock();
        skip_locked();
    }

    /// Filters on the primary key: `table.id = ?`, one condition per key column.
    pub fn where_key(mut self, key: M::Key) -> Self {
        for (column, value) in M::PRIMARY_KEY.iter().zip(M::key_values(key)) {
            let column = format!("{}.{column}", M::TABLE);
            self.builder = self.builder.where_(column, "=", value);
        }
        self
    }

    /// Filters on several primary keys, like Eloquent's `whereKey([...])`:
    /// `table.id IN (...)`, or `(a, b) IN (...)` for a composite key.
    pub fn where_keys(mut self, keys: impl IntoIterator<Item = M::Key>) -> Self {
        let columns = M::PRIMARY_KEY
            .iter()
            .map(|key| format!("{}.{key}", M::TABLE))
            .collect();
        let keys = keys.into_iter().map(M::key_values).collect();
        self.builder = super::relation::where_keys(self.builder, columns, keys);
        self
    }

    /// Eager-loads a relation path such as `"posts"` or `"posts.comments"`
    /// into the models' [`Rel`](crate::Rel) fields: one extra query per relation,
    /// whatever the number of models.
    pub fn with(mut self, relation: impl Into<String>) -> Self {
        self.eager.push(EagerLoad::path(relation));
        self
    }

    /// Eager-loads `relation` as [`with`](Self::with) does, with `build`
    /// changing the query of its last relation, like Eloquent's
    /// `with(['posts' => fn ($q) => ...])`:
    ///
    /// ```
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "users")]
    /// # #[derive(Debug, Clone, Default)]
    /// # struct User { id: i64, #[rustique(has_many(Post))] posts: Rel<Vec<Post>> }
    /// # #[rustique::model(table = "posts")]
    /// # #[derive(Debug, Clone, Default)]
    /// # struct Post { id: i64, user_id: i64 }
    /// let users = User::query().with_where("posts", |q| q.where_("posts.published", "=", true));
    /// ```
    ///
    /// For `"posts.comments"`, `build` changes the comments query; the posts
    /// load unfiltered. The query runs once for all parents, so a `limit`
    /// in `build` limits the related rows of all parents together.
    pub fn with_where(
        mut self,
        relation: impl Into<String>,
        build: impl Fn(QueryBuilder) -> QueryBuilder + Send + Sync + 'static,
    ) -> Self {
        self.eager.push(EagerLoad {
            constraint: Some(Arc::new(build)),
            ..EagerLoad::path(relation)
        });
        self
    }

    /// Eager-loads the `morph_to` relation `relation` as [`with`](Self::with)
    /// does, counting relations of the loaded models per type, like
    /// Eloquent's `morphWithCount`: `counts` pairs a morph alias with a
    /// relation of that type's model, whose `#[rustique(count)]` field is filled
    /// as by [`with_count`](Self::with_count).
    ///
    /// ```
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "posts")] #[derive(Debug, Clone, Default)]
    /// # struct Post { id: i64, #[rustique(has_many(Tag))] tags: Rel<Vec<Tag>>, #[rustique(count)] tags_count: Rel<u64> }
    /// # #[rustique::model(table = "tags")] #[derive(Debug, Clone, Default)] struct Tag { id: i64, post_id: i64 }
    /// # #[rustique::model(table = "videos")] #[derive(Debug, Clone, Default)] struct Video { id: i64 }
    /// # #[derive(Debug, Clone, rustique::MorphTo)] enum Commentable { Post(Post), Video(Video) }
    /// # #[rustique::model(table = "comments")] #[derive(Debug, Clone, Default)]
    /// # struct Comment { id: i64, commentable_type: String, commentable_id: i64,
    /// #   #[rustique(morph_to)] commentable: Rel<Option<Commentable>> }
    /// // Each post among the parents gets `tags_count`; videos get nothing.
    /// let comments = Comment::query().with_morph_count("commentable", &[("posts", "tags")]);
    /// ```
    ///
    /// A path such as `"post.commentable"` counts on the models of its last
    /// relation, which must be a `morph_to` relation. Counting the `morph_to`
    /// relation itself (at most one model) is not supported; Eloquent rejects
    /// it too.
    pub fn with_morph_count(
        mut self,
        relation: impl Into<String>,
        counts: &[(&str, &str)],
    ) -> Self {
        self.eager.push(EagerLoad {
            morph_counts: (counts.iter())
                .map(|(alias, relation)| ((*alias).to_owned(), (*relation).to_owned()))
                .collect(),
            ..EagerLoad::path(relation)
        });
        self
    }

    /// Keeps models with at least one related row matching `build`, like
    /// Laravel's `whereHas`. Pass `|q| q` to only require one related row.
    pub fn where_has(
        self,
        relation: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.has(relation, build, false)
    }

    /// Keeps models without related rows matching `build`, like Laravel's `whereDoesntHave`.
    pub fn where_doesnt_have(
        self,
        relation: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.has(relation, build, true)
    }

    /// Keeps models whose `morph_to` relation points at a model of one of
    /// `types` (morph aliases; empty for every type of the
    /// [`MorphTo`](crate::MorphTo) enum) with a row matching `build`, like
    /// Eloquent's `whereHasMorph`. `build` gets each type's query and alias:
    ///
    /// ```
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "posts")] #[derive(Debug, Clone, Default)] struct Post { id: i64 }
    /// # #[rustique::model(table = "videos")] #[derive(Debug, Clone, Default)] struct Video { id: i64 }
    /// # #[derive(Debug, Clone, rustique::MorphTo)] enum Commentable { Post(Post), Video(Video) }
    /// # #[rustique::model(table = "comments")] #[derive(Debug, Clone, Default)]
    /// # struct Comment { id: i64, commentable_type: String, commentable_id: i64,
    /// #   #[rustique(morph_to)] commentable: Rel<Option<Commentable>> }
    /// let on_posts = Comment::query().where_has_morph("commentable", &["posts"], |q, _| {
    ///     q.where_("posts.id", ">", 10)
    /// });
    /// ```
    pub fn where_has_morph(
        self,
        relation: &str,
        types: &[&str],
        build: impl Fn(QueryBuilder, &str) -> QueryBuilder,
    ) -> Self {
        self.has_morph(relation, types, build, false)
    }

    /// Keeps models whose `morph_to` relation points at a model of one of
    /// `types` without a row matching `build`, like Eloquent's
    /// `whereDoesntHaveMorph`; see [`where_has_morph`](Self::where_has_morph).
    pub fn where_doesnt_have_morph(
        self,
        relation: &str,
        types: &[&str],
        build: impl Fn(QueryBuilder, &str) -> QueryBuilder,
    ) -> Self {
        self.has_morph(relation, types, build, true)
    }

    fn has_morph(
        mut self,
        relation: &str,
        types: &[&str],
        build: impl Fn(QueryBuilder, &str) -> QueryBuilder,
        negated: bool,
    ) -> Self {
        let Some([type_column, id_column]) = M::morph_columns(relation) else {
            let error = Error::UnknownRelation {
                model: M::TABLE,
                relation: relation.to_owned(),
            };
            self.builder.fail(error.to_string());
            return self;
        };
        let targets: Vec<_> = M::morph_targets(relation)
            .into_iter()
            .filter(|t| types.is_empty() || types.contains(&t.alias))
            .collect();
        if targets.is_empty() {
            self.builder = self.builder.where_raw("0 = 1", []);
            return self;
        }
        let parent = |column: &str| format!("{}.{column}", M::TABLE);
        self.builder = self.builder.where_group(|group| {
            targets.iter().fold(group, |group, target| {
                group.or_where_group(|q| {
                    let mut related = (target.query)();
                    let mut table = target.table;
                    if table == M::TABLE {
                        related.alias_table(table, "orm_self");
                        table = "orm_self";
                    }
                    let key = target.key.first().copied().unwrap_or("id");
                    let related = related
                        .where_column(format!("{table}.{key}"), "=", parent(id_column))
                        .where_group(|r| build(r, target.alias));
                    let q = q.where_(parent(type_column), "=", target.alias);
                    if negated {
                        q.where_not_exists(related)
                    } else {
                        q.where_exists(related)
                    }
                })
            })
        });
        self
    }

    fn has(
        mut self,
        relation: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
        negated: bool,
    ) -> Self {
        let Some(relation) = self.relation(relation) else {
            return self;
        };
        // The group keeps `OR`s in `build` from escaping the correlation.
        let related = relation.correlated(M::TABLE).where_group(build);
        self.builder = if negated {
            self.builder.where_not_exists(related)
        } else {
            self.builder.where_exists(related)
        };
        self
    }

    /// Adds `relation_count`, the number of related rows, read into the
    /// model's `#[rustique(count)] relation_count: Rel<u64>` field.
    pub fn with_count(self, relation: &str) -> Self {
        self.with_count_where(relation, |q| q)
    }

    /// As [`with_count`](Self::with_count), counting only the related rows
    /// matching `build`.
    pub fn with_count_where(
        self,
        relation: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, Aggregate::Count, None, build)
    }

    /// Adds `relation_sum_column`, the sum of `column` over the related
    /// rows, like Eloquent's `withSum`; read it into an
    /// `#[rustique(aggregate)] posts_sum_votes: Rel<Option<i64>>` field (`NULL`
    /// without related rows). A qualified `column` (`posts.votes`) is named
    /// after its last part.
    pub fn with_sum(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, Aggregate::Sum, Some(column), |q| q)
    }

    /// As [`with_sum`](Self::with_sum), over the related rows matching
    /// `build` only, like Eloquent's `withSum(['posts' => fn ($q) => ...], 'votes')`.
    pub fn with_sum_where(
        self,
        relation: &str,
        column: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, Aggregate::Sum, Some(column), build)
    }

    /// Adds `relation_avg_column`; see [`with_sum`](Self::with_sum).
    pub fn with_avg(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, Aggregate::Avg, Some(column), |q| q)
    }

    /// As [`with_avg`](Self::with_avg), over the related rows matching `build`.
    pub fn with_avg_where(
        self,
        relation: &str,
        column: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, Aggregate::Avg, Some(column), build)
    }

    /// Adds `relation_min_column`; see [`with_sum`](Self::with_sum).
    pub fn with_min(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, Aggregate::Min, Some(column), |q| q)
    }

    /// As [`with_min`](Self::with_min), over the related rows matching `build`.
    pub fn with_min_where(
        self,
        relation: &str,
        column: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, Aggregate::Min, Some(column), build)
    }

    /// Adds `relation_max_column`; see [`with_sum`](Self::with_sum).
    pub fn with_max(self, relation: &str, column: &str) -> Self {
        self.with_aggregate(relation, Aggregate::Max, Some(column), |q| q)
    }

    /// As [`with_max`](Self::with_max), over the related rows matching `build`.
    pub fn with_max_where(
        self,
        relation: &str,
        column: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        self.with_aggregate(relation, Aggregate::Max, Some(column), build)
    }

    /// Adds `relation_exists`, whether any related row exists, like
    /// Eloquent's `withExists`; read it into an
    /// `#[rustique(aggregate)] posts_exists: Rel<bool>` field.
    pub fn with_exists(self, relation: &str) -> Self {
        self.with_exists_where(relation, |q| q)
    }

    /// As [`with_exists`](Self::with_exists), looking only for related rows
    /// matching `build`.
    pub fn with_exists_where(
        mut self,
        relation: &str,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        let (relation, alias) = split_alias(relation);
        let Some(found) = self.relation(relation) else {
            return self;
        };
        let query = found.correlated(M::TABLE).where_group(build);
        self.select_all();
        self.builder.columns.push(Expr::Exists {
            query: Box::new(query),
            alias: alias.map_or_else(|| format!("{relation}_exists"), str::to_owned),
        });
        self
    }

    /// Adds `(SELECT FUNCTION(column) ...) AS relation_function_column`
    /// (`relation_count` for counts).
    fn with_aggregate(
        mut self,
        relation: &str,
        function: Aggregate,
        column: Option<&str>,
        build: impl FnOnce(QueryBuilder) -> QueryBuilder,
    ) -> Self {
        let (relation, alias) = split_alias(relation);
        let Some(found) = self.relation(relation) else {
            return self;
        };
        let mut query = found.correlated(M::TABLE).where_group(build);
        query.columns = vec![Expr::Aggregate {
            function,
            column: column.map(str::to_owned),
            alias: None,
        }];
        let alias = match (alias, column) {
            (Some(alias), _) => alias.to_owned(),
            (None, Some(column)) => format!(
                "{relation}_{}_{}",
                function.as_sql().to_ascii_lowercase(),
                crate::query::output_name(column)
            ),
            (None, None) => format!("{relation}_count"),
        };
        self.select_all();
        self.builder = self.builder.select_sub(query, alias);
        self
    }

    /// Selects the model's own columns before columns are added to an
    /// empty select list.
    fn select_all(&mut self) {
        if self.builder.columns.is_empty() {
            self.builder = std::mem::take(&mut self.builder).select([format!("{}.*", M::TABLE)]);
        }
    }

    /// Looks up a relation, recording an error for unknown names.
    fn relation(&mut self, name: &str) -> Option<super::Relation> {
        let relation = M::relation(name);
        if relation.is_none() {
            let error = Error::UnknownRelation {
                model: M::TABLE,
                relation: name.to_owned(),
            };
            self.builder.fail(error.to_string());
        }
        relation
    }

    /// Includes soft-deleted rows.
    pub fn with_trashed(mut self) -> Self {
        self.trashed = Trashed::With;
        self
    }

    /// Returns only soft-deleted rows.
    pub fn only_trashed(mut self) -> Self {
        self.trashed = Trashed::Only;
        self
    }

    /// The plain builder, with the global scopes and the soft-delete filter applied.
    pub fn into_builder(self) -> QueryBuilder {
        let trashed = self.trashed;
        let mut builder = M::apply_global_scopes(self).builder;
        if builder.columns.is_empty() && !builder.joins.is_empty() {
            // Models only read their own columns; a joined table's `id` or
            // `created_at` would otherwise shadow theirs, and `distinct` and
            // counts would see the joined columns.
            builder = builder.select([format!("{}.*", M::TABLE)]);
        }
        if !M::SOFT_DELETES || trashed == Trashed::With {
            return builder;
        }
        // `a OR b AND deleted_at IS NULL` would only filter `b`.
        builder.group_or_wheres();
        let column = format!("{}.{}", M::TABLE, M::DELETED_AT);
        match trashed {
            Trashed::Only => builder.where_not_null(column),
            _ => builder.where_null(column),
        }
    }

    /// Compiles the query into a `SELECT` statement.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::to_sql`].
    pub fn to_sql(&self, dialect: Dialect) -> Result<Compiled> {
        self.clone().into_builder().to_sql(dialect)
    }

    /// Runs the query and returns every matching model.
    ///
    /// # Errors
    ///
    /// Compile, database and decode errors.
    pub async fn get(&self, db: &impl Executor) -> Result<Collection<M>> {
        // Each row becomes a model as it arrives, so the rows are never all held at once.
        let models = db
            .fetch_map(self.to_sql(db.dialect())?, self.row_to_model())
            .await?;
        self.finish(db, models).await
    }

    /// Builds models from `rows` and eager-loads their relations.
    pub(super) async fn hydrate(
        &self,
        db: &impl Executor,
        rows: Vec<Row>,
    ) -> Result<Collection<M>> {
        let models = rows
            .into_iter()
            .map(self.row_to_model())
            .collect::<Result<_>>()?;
        self.finish(db, models).await
    }

    /// Builds one model from one row of this query.
    fn row_to_model(&self) -> impl FnMut(Row) -> Result<M> + Send + 'static {
        // `select(["id", "name"])` or `with("posts:id,title")`: the other
        // columns take their `Default`, as Eloquent leaves them unset.
        let partial = !self.builder.columns.is_empty()
            && !(self.builder.columns.iter())
                .any(|c| matches!(c, Expr::Column(c) if c == "*" || c.ends_with(".*")));
        let aliases: Vec<String> = self.pivot.iter().map(|(a, _)| a.clone()).collect();
        let pivot_columns: Arc<[String]> =
            self.pivot.iter().map(|(_, c)| (*c).to_owned()).collect();
        move |mut row| {
            if partial {
                row.set_partial();
            }
            let pivot = if aliases.is_empty() {
                None
            } else {
                let values = (aliases.iter())
                    .map(|alias| row.take::<Value>(alias))
                    .collect::<Result<_>>()?;
                Some(Row::new(Arc::clone(&pivot_columns), values, row.dialect()))
            };
            let mut model: M = hydrate(row)?;
            model.meta_mut().pivot = pivot;
            Ok(model)
        }
    }

    /// Marks models loaded together as siblings and eager-loads their relations.
    async fn finish(&self, db: &impl Executor, models: Vec<M>) -> Result<Collection<M>> {
        let siblings = models.len() > 1;
        let mut models = Collection::from(models);
        for model in models.iter_mut() {
            model.meta_mut().siblings = siblings;
        }
        eager_load(db, &mut models, &self.eager).await?;
        Ok(models)
    }

    /// The first matching model, if any.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn first(&self, db: &impl Executor) -> Result<Option<M>> {
        let rows = Vec::from_iter(self.clone().into_builder().first(db).await?);
        Ok(self.hydrate(db, rows).await?.pop())
    }

    /// The first matching model.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing matches, plus the errors of [`get`](Self::get).
    pub async fn first_or_fail(&self, db: &impl Executor) -> Result<M> {
        self.first(db).await?.ok_or(Error::NotFound(M::TABLE))
    }

    /// The only matching model, like Eloquent's `sole`.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] when nothing matches, [`Error::MultipleRecords`]
    /// when more than one row does, plus the errors of [`get`](Self::get).
    pub async fn sole(&self, db: &impl Executor) -> Result<M> {
        let mut models = self.clone().limit(2).get(db).await?.into_inner();
        match models.len() {
            0 => Err(Error::NotFound(M::TABLE)),
            1 => Ok(models.remove(0)),
            _ => Err(Error::MultipleRecords(M::TABLE)),
        }
    }

    /// `column` of the first matching row; see [`QueryBuilder::value`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::value`].
    pub async fn value<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.clone().into_builder().value(db, column).await
    }

    /// The values of `column` from every matching row; see [`QueryBuilder::pluck`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::pluck`].
    pub async fn pluck<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<Vec<T>> {
        self.clone().into_builder().pluck(db, column).await
    }

    /// `(key, value)` pairs from every matching row; see [`QueryBuilder::pluck_keyed`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::pluck_keyed`].
    pub async fn pluck_keyed<K: FromValue, V: FromValue>(
        &self,
        db: &impl Executor,
        value: &str,
        key: &str,
    ) -> Result<Vec<(K, V)>> {
        self.clone()
            .into_builder()
            .pluck_keyed(db, value, key)
            .await
    }

    /// Page `page` (1-based) of `per_page` models, plus the total count.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn paginate(&self, db: &impl Executor, page: u64, per_page: u64) -> Result<Page<M>> {
        let (page, per_page) = (page.max(1), per_page.max(1));
        let total = self.count(db).await?;
        let items = self.clone().for_page(page, per_page).get(db).await?;
        Ok(Page {
            items,
            total,
            page,
            per_page,
        })
    }

    /// Page `page` (1-based) of `per_page` models without a total count;
    /// see [`QueryBuilder::simple_paginate`].
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn simple_paginate(
        &self,
        db: &impl Executor,
        page: u64,
        per_page: u64,
    ) -> Result<SimplePage<M>> {
        let (page, per_page) = (page.max(1), per_page.max(1));
        let mut query = self.clone();
        query.builder = query.builder.simple_page_rows(page, per_page);
        let models = query.get(db).await?.into_inner();
        Ok(SimplePage::new(models, page, per_page))
    }

    /// Page of `per_page` models after (or before) `cursor`; see
    /// [`QueryBuilder::cursor_paginate`]. The primary key is appended to
    /// the ordering when it is not part of it, so the order is unique;
    /// without ordering the models are paged by key.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::cursor_paginate`].
    pub async fn cursor_paginate(
        &self,
        db: &impl Executor,
        per_page: u64,
        cursor: Option<&str>,
    ) -> Result<CursorPage<M>> {
        let mut query = self.clone();
        for key in M::PRIMARY_KEY {
            let qualified = format!("{}.{key}", M::TABLE);
            let ordered = query.builder.orders.iter().any(|order| {
                matches!(order, Order::Column { column, .. } if *column == qualified || column == key)
            });
            if !ordered {
                query = query.order_by(qualified);
            }
        }
        let (rows, next_cursor, prev_cursor) = query
            .clone()
            .into_builder()
            .cursor_rows(db, per_page, cursor)
            .await?;
        Ok(CursorPage {
            items: query.hydrate(db, rows).await?,
            per_page: per_page.max(1),
            next_cursor,
            prev_cursor,
        })
    }

    /// Walks the matching models in chunks of `size`, ordered by primary key;
    /// see [`QueryBuilder::chunk_by_id`]. Each chunk eager-loads the
    /// [`with`](Self::with) relations.
    ///
    /// ```no_run
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "users")]
    /// # #[derive(Debug, Clone, Default)]
    /// # struct User { id: i64 }
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let mut chunks = User::query().chunk_by_id(500);
    /// while let Some(users) = chunks.next(db).await? {
    ///     println!("{} users", users.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Composite primary keys work too: they are ordered and compared
    /// column by column.
    pub fn chunk_by_id(&self, size: u64) -> ModelChunks<M> {
        let columns = M::PRIMARY_KEY
            .iter()
            .map(|key| format!("{}.{key}", M::TABLE))
            .collect();
        ModelChunks {
            rows: self.clone().into_builder().chunk_by_columns(size, columns),
            query: self.clone(),
        }
    }

    /// Walks the matching models in chunks of `size` with `LIMIT` / `OFFSET`;
    /// see [`QueryBuilder::chunk`]. Without ordering the models are ordered
    /// by primary key, as in Eloquent. Each chunk eager-loads the
    /// [`with`](Self::with) relations.
    pub fn chunk(&self, size: u64) -> ModelChunks<M> {
        let mut query = self.clone();
        if query.builder.orders.is_empty() {
            for key in M::PRIMARY_KEY {
                query = query.order_by(format!("{}.{key}", M::TABLE));
            }
        }
        ModelChunks {
            rows: query.clone().into_builder().chunk(size),
            query,
        }
    }

    /// Streams the matching models one at a time; see [`QueryBuilder::cursor`].
    ///
    /// ```no_run
    /// # use rustique::prelude::*;
    /// # #[rustique::model(table = "users")]
    /// # #[derive(Debug, Clone, Default)]
    /// # struct User { id: i64 }
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let mut users = User::query().cursor(db)?;
    /// while let Some(user) = users.next().await? {
    ///     println!("{}", user.id);
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::cursor`], plus [`Error::InvalidQuery`] after
    /// [`with`](Self::with): eager loading needs the models in batches, so
    /// use [`chunk_by_id`](Self::chunk_by_id) instead.
    pub fn cursor<'a>(&self, db: &'a impl Executor) -> Result<ModelCursor<'a, M>> {
        self.cursor_buffered(db, crate::db::cursor::BUFFER)
    }

    /// As [`cursor`](Self::cursor), reading up to `buffer` rows ahead; see
    /// [`QueryBuilder::cursor_buffered`].
    ///
    /// # Errors
    ///
    /// As [`cursor`](Self::cursor).
    pub fn cursor_buffered<'a>(
        &self,
        db: &'a impl Executor,
        buffer: usize,
    ) -> Result<ModelCursor<'a, M>> {
        if !self.eager.is_empty() {
            return Err(Error::InvalidQuery(
                "a cursor cannot eager-load relations; use `chunk_by_id`".to_owned(),
            ));
        }
        Ok(ModelCursor {
            rows: self.clone().into_builder().cursor_buffered(db, buffer)?,
            model: PhantomData,
        })
    }

    /// Whether any row matches.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::exists`].
    pub async fn exists(&self, db: &impl Executor) -> Result<bool> {
        self.clone().into_builder().exists(db).await
    }

    /// `COUNT(*)` of the matching rows.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::count`].
    pub async fn count(&self, db: &impl Executor) -> Result<u64> {
        self.clone().into_builder().count(db).await
    }

    /// `MAX(column)`; see [`QueryBuilder::max`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::max`].
    pub async fn max<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.clone().into_builder().max(db, column).await
    }

    /// `MIN(column)`; see [`QueryBuilder::min`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::min`].
    pub async fn min<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.clone().into_builder().min(db, column).await
    }

    /// `SUM(column)`; see [`QueryBuilder::sum`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::sum`].
    pub async fn sum<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.clone().into_builder().sum(db, column).await
    }

    /// `AVG(column)`; see [`QueryBuilder::avg`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::avg`].
    pub async fn avg<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.clone().into_builder().avg(db, column).await
    }

    /// Updates the matching rows, plus `updated_at` with `timestamps`, and
    /// returns how many changed. Loaded models are not refreshed.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::update`].
    pub async fn update(&self, db: &impl Executor, mut values: Vec<(&str, Value)>) -> Result<u64> {
        if M::TIMESTAMPS && !values.iter().any(|(column, _)| *column == M::UPDATED_AT) {
            values.push((M::UPDATED_AT, now()));
        }
        self.clone().into_builder().update(db, values).await
    }

    /// Adds `amount` to `column` of the matching rows, plus `updated_at` with
    /// `timestamps`; see [`QueryBuilder::increment`].
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::increment`].
    pub async fn increment(
        &self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value>,
    ) -> Result<u64> {
        let mut values = Vec::new();
        if M::TIMESTAMPS {
            values.push((M::UPDATED_AT, now()));
        }
        self.clone()
            .into_builder()
            .increment(db, column, amount, values)
            .await
    }

    /// Subtracts `amount` from `column` of the matching rows; see [`increment`](Self::increment).
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::decrement`].
    pub async fn decrement(
        &self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value>,
    ) -> Result<u64> {
        let amount = crate::query::negate(amount.into())?;
        self.increment(db, column, amount).await
    }

    /// Deletes the matching rows; with `soft_deletes`, sets their `deleted_at`.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::delete`].
    pub async fn delete(&self, db: &impl Executor) -> Result<u64> {
        if M::SOFT_DELETES {
            let now = now();
            let mut values = vec![(M::DELETED_AT, now.clone())];
            if M::TIMESTAMPS {
                values.push((M::UPDATED_AT, now));
            }
            self.update(db, values).await
        } else {
            self.force_delete(db).await
        }
    }

    /// Deletes the matching rows for real, even with `soft_deletes`.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::delete`].
    pub async fn force_delete(&self, db: &impl Executor) -> Result<u64> {
        self.clone().into_builder().delete(db).await
    }
}

/// Applies a `#[rustique::scopes]` scope, like Eloquent's `callScope`: the WHERE
/// clauses before it and those it adds are each wrapped in parentheses when
/// they contain an `OR`, so a scope's `OR` cannot escape the other filters.
#[doc(hidden)]
pub fn scope<M: Model>(query: Query<M>, apply: impl FnOnce(Query<M>) -> Query<M>) -> Query<M> {
    let before = query.builder.wheres.len();
    let mut query = apply(query);
    query.builder = regroup(std::mem::take(&mut query.builder), before);
    query
}

/// Applies `apply` to `builder` grouped like a [`scope`].
pub(crate) fn grouped(
    builder: QueryBuilder,
    apply: impl FnOnce(QueryBuilder) -> QueryBuilder,
) -> QueryBuilder {
    let before = builder.wheres.len();
    regroup(apply(builder), before)
}

/// Wraps the WHERE clauses before `before` and those after it each in
/// parentheses when they contain an `OR`.
fn regroup(mut builder: QueryBuilder, before: usize) -> QueryBuilder {
    if builder.wheres.len() > before {
        let added = builder.wheres.split_off(before);
        let original = std::mem::take(&mut builder.wheres);
        builder.wheres = [original, added]
            .into_iter()
            .flat_map(group_if_or)
            .collect();
    }
    builder
}

/// `clauses`, or one parenthesized group of them when one is joined with `OR`.
fn group_if_or(clauses: Vec<Clause>) -> Vec<Clause> {
    if !clauses.iter().any(|c| c.conjunction == Conjunction::Or) {
        return clauses;
    }
    vec![Clause {
        conjunction: clauses[0].conjunction,
        condition: Condition::Group(clauses),
    }]
}

/// Chunks of models from [`Query::chunk_by_id`]; call [`next`](Self::next)
/// until it returns `None`.
#[must_use]
pub struct ModelChunks<M> {
    rows: Chunks,
    /// For eager loading.
    query: Query<M>,
}

impl<M> fmt::Debug for ModelChunks<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelChunks")
            .field("rows", &self.rows)
            .finish_non_exhaustive()
    }
}

impl<M: Model> ModelChunks<M> {
    /// The next chunk, or `None` after the last one.
    ///
    /// # Errors
    ///
    /// As [`Query::get`].
    pub async fn next(&mut self, db: &impl Executor) -> Result<Option<Collection<M>>> {
        match self.rows.next(db).await? {
            Some(rows) => Ok(Some(self.query.hydrate(db, rows.into_inner()).await?)),
            None => Ok(None),
        }
    }
}

/// Models streamed one at a time, from [`Query::cursor`]; call
/// [`next`](Self::next) until it returns `None`.
#[must_use]
pub struct ModelCursor<'a, M> {
    rows: Cursor<'a>,
    model: PhantomData<fn() -> M>,
}

impl<M> fmt::Debug for ModelCursor<'_, M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModelCursor")
            .field("rows", &self.rows)
            .finish()
    }
}

impl<M: Model> ModelCursor<'_, M> {
    /// The next model, or `None` after the last one.
    ///
    /// # Errors
    ///
    /// As [`Cursor::next`], plus decode errors.
    pub async fn next(&mut self) -> Result<Option<M>> {
        let model = self.rows.next().await?.map(hydrate::<M>).transpose()?;
        Ok(model.map(|mut model| {
            // Loaded one by one, but in a loop like any list.
            model.meta_mut().siblings = true;
            model
        }))
    }
}

/// `"posts as published"` into `("posts", Some("published"))`, for the
/// `with_count` family, as Eloquent's `withCount('posts as published')`.
fn split_alias(relation: &str) -> (&str, Option<&str>) {
    match relation.to_ascii_lowercase().find(" as ") {
        Some(at) => (relation[..at].trim(), Some(relation[at + 4..].trim())),
        None => (relation, None),
    }
}
