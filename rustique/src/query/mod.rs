//! The fluent query builder.
//!
//! - `mod.rs`: [`QueryBuilder`]: select list, grouping, ordering, limits, locks, compiling.
//! - `wheres.rs`: WHERE methods, plus condition constructors shared with HAVING and joins.
//! - `join.rs`: JOIN methods and [`JoinClause`].
//! - `execute.rs`: methods that run the query (`get`, `first`, `count`, `insert`, ...),
//!   [`Page`] and [`Chunks`].
//! - `paginate.rs`: cursor pagination, [`CursorPage`].
//! - `ast.rs`: the syntax tree the builder produces.
//! - `operator.rs`: the allowed comparison operators.

mod ast;
mod execute;
mod join;
mod operator;
mod paginate;
mod wheres;

pub(crate) use ast::{
    Aggregate, Clause, Condition, Conjunction, Expr, Join, JoinKind, Lock, LockWait, Order, Raw,
    Union,
};
pub use execute::{Chunks, Page, SimplePage};
pub(crate) use execute::{negate, output_name};
pub use join::JoinClause;
pub(crate) use operator::Operator;
pub use paginate::CursorPage;

use std::time::Duration;

use crate::grammar::{self, Compiled, Dialect};
use crate::{Result, Value};

/// Fluent SQL query builder, the equivalent of Laravel's `DB::table()`.
///
/// Builder methods never fail. A mistake such as an unknown operator is
/// remembered and reported when the query is compiled ([`to_sql`](Self::to_sql)).
///
/// Column and table names may be qualified (`users.email`), aliased
/// (`email as e`, `users as u`) or `*` (`users.*`); they are always quoted.
#[derive(Debug, Clone, Default)]
#[must_use]
pub struct QueryBuilder {
    /// The table, or the alias of `from_sub`.
    pub(crate) table: String,
    /// `FROM (SELECT ...) table`
    pub(crate) from_sub: Option<Box<QueryBuilder>>,
    pub(crate) columns: Vec<Expr>,
    pub(crate) distinct: bool,
    pub(crate) joins: Vec<Join>,
    pub(crate) wheres: Vec<Clause>,
    /// `Expr::Column` or `Expr::Raw`.
    pub(crate) groups: Vec<Expr>,
    pub(crate) havings: Vec<Clause>,
    pub(crate) orders: Vec<Order>,
    pub(crate) limit: Option<u64>,
    pub(crate) offset: Option<u64>,
    /// `limit_per`: columns whose groups `limit` / `offset` apply to.
    pub(crate) limit_per: Vec<String>,
    pub(crate) lock: Option<Lock>,
    pub(crate) unions: Vec<Union>,
    /// First mistake made while building, reported at compile time.
    pub(crate) error: Option<String>,
}

impl QueryBuilder {
    /// Starts a query on `table`.
    pub fn table(table: impl Into<String>) -> Self {
        Self {
            table: table.into(),
            ..Self::default()
        }
    }

    /// Starts a query on the rows of `query`, like Laravel's `fromSub`:
    /// `SELECT * FROM (SELECT ...) alias`.
    pub fn from_sub(query: Self, alias: impl Into<String>) -> Self {
        Self {
            from_sub: Some(Box::new(query)),
            ..Self::table(alias)
        }
    }

    /// Replaces the select list. An empty list selects `*`.
    pub fn select(mut self, columns: impl IntoIterator<Item: Into<String>>) -> Self {
        self.columns = columns
            .into_iter()
            .map(|c| Expr::Column(c.into()))
            .collect();
        self
    }

    /// Appends columns to the select list.
    pub fn add_select(mut self, columns: impl IntoIterator<Item: Into<String>>) -> Self {
        self.columns
            .extend(columns.into_iter().map(|c| Expr::Column(c.into())));
        self
    }

    /// Appends a raw expression to the select list; `?` marks bindings.
    pub fn select_raw(
        mut self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        self.columns.push(Expr::Raw(Raw::new(sql, binds)));
        self
    }

    /// Appends `(subquery) AS alias` to the select list.
    pub fn select_sub(mut self, query: Self, alias: impl Into<String>) -> Self {
        self.columns.push(Expr::Sub {
            query: Box::new(query),
            alias: alias.into(),
        });
        self
    }

    /// Adds `DISTINCT`.
    pub fn distinct(mut self) -> Self {
        self.distinct = true;
        self
    }

    /// Appends `GROUP BY` columns.
    pub fn group_by(mut self, columns: impl IntoIterator<Item: Into<String>>) -> Self {
        self.groups
            .extend(columns.into_iter().map(|c| Expr::Column(c.into())));
        self
    }

    /// Appends a raw `GROUP BY` expression; `?` marks bindings.
    pub fn group_by_raw(
        mut self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        self.groups.push(Expr::Raw(Raw::new(sql, binds)));
        self
    }

    /// Adds `HAVING column op value`, joined with `AND`.
    pub fn having(mut self, column: impl Into<String>, op: &str, value: impl Into<Value>) -> Self {
        let condition = wheres::basic(column.into(), op, value.into());
        wheres::push(
            &mut self.havings,
            &mut self.error,
            Conjunction::And,
            condition,
        );
        self
    }

    /// Adds `HAVING column op value`, joined with `OR`.
    pub fn or_having(
        mut self,
        column: impl Into<String>,
        op: &str,
        value: impl Into<Value>,
    ) -> Self {
        let condition = wheres::basic(column.into(), op, value.into());
        wheres::push(
            &mut self.havings,
            &mut self.error,
            Conjunction::Or,
            condition,
        );
        self
    }

    /// Adds a raw `HAVING` expression, joined with `AND`; `?` marks bindings.
    pub fn having_raw(
        mut self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        let condition = Ok(Condition::Raw(Raw::new(sql, binds)));
        wheres::push(
            &mut self.havings,
            &mut self.error,
            Conjunction::And,
            condition,
        );
        self
    }

    /// Sorts ascending by `column`.
    pub fn order_by(mut self, column: impl Into<String>) -> Self {
        self.orders.push(Order::Column {
            column: column.into(),
            descending: false,
        });
        self
    }

    /// Sorts descending by `column`.
    pub fn order_by_desc(mut self, column: impl Into<String>) -> Self {
        self.orders.push(Order::Column {
            column: column.into(),
            descending: true,
        });
        self
    }

    /// Sorts by a raw expression; `?` marks bindings.
    pub fn order_by_raw(
        mut self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        self.orders.push(Order::Raw(Raw::new(sql, binds)));
        self
    }

    /// Sorts newest first: descending by `column`, like Laravel's `latest`.
    pub fn latest(self, column: impl Into<String>) -> Self {
        self.order_by_desc(column)
    }

    /// Sorts oldest first: ascending by `column`, like Laravel's `oldest`.
    pub fn oldest(self, column: impl Into<String>) -> Self {
        self.order_by(column)
    }

    /// Applies `build` only when `condition` holds, like Laravel's `when`:
    ///
    /// ```
    /// # use rustique::prelude::*;
    /// let search: Option<&str> = Some("ann");
    /// let query = QueryBuilder::table("users")
    ///     .when(search.is_some(), |q| q.where_("name", "like", format!("%{}%", search.unwrap())));
    /// ```
    pub fn when(self, condition: bool, build: impl FnOnce(Self) -> Self) -> Self {
        if condition { build(self) } else { self }
    }

    /// Appends `UNION query`: rows of both queries, without duplicates.
    ///
    /// This query's ordering, limit and offset apply to the combined rows,
    /// so order by output column names (`id`, not `users.id`). `query`
    /// keeps its own ordering and limit.
    pub fn union(mut self, query: Self) -> Self {
        self.unions.push(Union { query, all: false });
        self
    }

    /// Appends `UNION ALL query`: rows of both queries, duplicates kept.
    /// See [`union`](Self::union).
    pub fn union_all(mut self, query: Self) -> Self {
        self.unions.push(Union { query, all: true });
        self
    }

    /// Returns at most `limit` rows.
    pub fn limit(mut self, limit: u64) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skips the first `offset` rows.
    pub fn offset(mut self, offset: u64) -> Self {
        self.offset = Some(offset);
        self
    }

    /// Applies [`limit`](Self::limit) and [`offset`](Self::offset) to each
    /// group of rows sharing `columns` instead of to the whole result, like
    /// Laravel's `groupLimit`: the rows are numbered with `ROW_NUMBER() OVER
    /// (PARTITION BY columns ORDER BY <the query's order>)` in a subquery and
    /// the outer query keeps the numbers in range, ordered by them. Needs an
    /// order. Eager loads use it for a constraint's `limit`, so
    /// `with_where("posts", |q| q.order_by_desc("id").limit(3))` loads 3 posts per user.
    /// Paging and chunking replace the limit, so they fail on such a query.
    pub fn limit_per(mut self, columns: impl IntoIterator<Item: Into<String>>) -> Self {
        self.limit_per = columns.into_iter().map(Into::into).collect();
        self
    }

    /// Selects page `page` (1-based) of `per_page` rows.
    pub fn for_page(self, page: u64, per_page: u64) -> Self {
        let offset = page.saturating_sub(1).saturating_mul(per_page);
        self.offset(offset).limit(per_page)
    }

    /// Adds `FOR UPDATE`.
    pub fn lock_for_update(mut self) -> Self {
        self.lock.get_or_insert_default().shared = false;
        self
    }

    /// Adds `FOR UPDATE WAIT n` (Oracle): fails with a lock timeout (see
    /// [`Error::is_lock_timeout`](crate::Error::is_lock_timeout)) when the rows
    /// stay locked by another transaction for `timeout`, rounded up to whole
    /// seconds. MySQL has no per-statement wait (its `innodb_lock_wait_timeout`
    /// is a session setting), so compiling for MySQL returns
    /// [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn lock_for_update_wait(mut self, timeout: Duration) -> Self {
        let seconds = timeout.as_secs() + u64::from(timeout.subsec_nanos() > 0);
        let lock = self.lock.get_or_insert_default();
        lock.shared = false;
        lock.wait = Some(LockWait::Seconds(seconds));
        self
    }

    /// Adds `FOR UPDATE NOWAIT`: fails with a lock timeout (see
    /// [`Error::is_lock_timeout`](crate::Error::is_lock_timeout)) instead of
    /// waiting when another transaction holds any of the rows.
    pub fn lock_for_update_no_wait(mut self) -> Self {
        let lock = self.lock.get_or_insert_default();
        lock.shared = false;
        lock.wait = Some(LockWait::NoWait);
        self
    }

    /// Adds `FOR SHARE` (MySQL): other transactions can read the rows but
    /// not change them. Oracle has no row-level shared lock, so compiling
    /// for Oracle returns [`Error::Unsupported`](crate::Error::Unsupported).
    pub fn shared_lock(mut self) -> Self {
        self.lock.get_or_insert_default().shared = true;
        self
    }

    /// Adds `SKIP LOCKED`, to `FOR UPDATE` unless [`shared_lock`](Self::shared_lock)
    /// is used. It cannot be combined with a wait
    /// ([`lock_for_update_no_wait`](Self::lock_for_update_no_wait) /
    /// [`lock_for_update_wait`](Self::lock_for_update_wait)): compiling fails
    /// with [`Error::InvalidQuery`](crate::Error::InvalidQuery).
    pub fn skip_locked(mut self) -> Self {
        self.lock.get_or_insert_default().skip_locked = true;
        self
    }

    /// Compiles the query into a `SELECT` statement.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) for mistakes made while
    /// building, [`Error::Unsupported`](crate::Error::Unsupported) when `dialect`
    /// cannot express the query.
    pub fn to_sql(&self, dialect: Dialect) -> Result<Compiled> {
        grammar::select(dialect, self)
    }

    /// Compiles an `INSERT` of `rows` into this query's table.
    /// Each row holds one value per entry of `columns`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) when there are no
    /// rows, a row length differs from `columns`, or (Oracle) there are no
    /// columns. MySQL inserts a row of defaults for no columns.
    pub fn to_insert_sql(
        &self,
        dialect: Dialect,
        columns: &[&str],
        rows: Vec<Vec<Value>>,
    ) -> Result<Compiled> {
        grammar::insert(dialect, &self.table, columns, rows)
    }

    /// Compiles `INSERT INTO table (columns) SELECT ...`, the rows of
    /// `source` inserted into this query's table, one `source` column per
    /// entry of `columns`. Without columns, `source` fills every column of
    /// the table in its order.
    ///
    /// # Errors
    ///
    /// As [`to_sql`](Self::to_sql) for `source`, plus
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) when this query has no table.
    pub fn to_insert_using_sql(
        &self,
        dialect: Dialect,
        columns: &[&str],
        source: &Self,
    ) -> Result<Compiled> {
        grammar::insert_using(dialect, &self.table, columns, source)
    }

    /// Compiles an upsert of `rows`: rows that clash with an existing row on
    /// `unique_by` update its `update` columns instead of being inserted. With
    /// an empty `update`, clashing rows are skipped.
    ///
    /// MySQL matches on any unique index (`INSERT ... ON DUPLICATE KEY UPDATE`),
    /// so `unique_by` should name one; Oracle matches on `unique_by` (`MERGE`).
    ///
    /// # Errors
    ///
    /// As [`to_insert_sql`](Self::to_insert_sql), plus
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) when `unique_by` is
    /// empty or `unique_by` / `update` name a column not in `columns`.
    pub fn to_upsert_sql(
        &self,
        dialect: Dialect,
        columns: &[&str],
        rows: Vec<Vec<Value>>,
        unique_by: &[&str],
        update: &[&str],
    ) -> Result<Compiled> {
        grammar::upsert(dialect, &self.table, columns, rows, unique_by, update)
    }

    /// Compiles an `UPDATE` setting `values` on the rows matching the WHERE clauses.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) when `values` is empty
    /// or the query has joins, grouping, ordering or limits.
    pub fn to_update_sql(&self, dialect: Dialect, values: Vec<(&str, Value)>) -> Result<Compiled> {
        grammar::update(dialect, self, values)
    }

    /// Compiles a `DELETE` of the rows matching the WHERE clauses.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) when the query has
    /// joins, grouping, ordering or limits.
    pub fn to_delete_sql(&self, dialect: Dialect) -> Result<Compiled> {
        grammar::delete(dialect, self)
    }

    /// Keeps the first mistake; later ones are usually caused by it.
    pub(crate) fn fail(&mut self, message: String) {
        self.error.get_or_insert(message);
    }

    /// Fails paging and chunking, which replace the limit that
    /// [`limit_per`](Self::limit_per) applies per group.
    pub(crate) fn reject_limit_per(&mut self, method: &str) {
        if !self.limit_per.is_empty() {
            self.fail(format!("`{method}` cannot be combined with `limit_per`"));
        }
    }

    /// Renames table `table` to `alias` in this query: `FROM table AS alias`
    /// (or the join of `table`), and every column qualified with `table.`.
    /// Subqueries and raw SQL are left alone. Lets a relation query on the
    /// parent's own table be correlated with the parent's rows.
    pub(crate) fn alias_table(&mut self, table: &str, alias: &str) {
        let rename = |column: &mut String| {
            if let Some(rest) = column.strip_prefix(table).and_then(|r| r.strip_prefix('.')) {
                *column = format!("{alias}.{rest}");
            }
        };
        if self.table == table {
            self.table = format!("{table} as {alias}");
        }
        for column in &mut self.columns {
            match column {
                Expr::Column(column) => rename(column),
                Expr::Aggregate {
                    column: Some(column),
                    ..
                } => rename(column),
                _ => {}
            }
        }
        for join in &mut self.joins {
            if join.table == table {
                join.table = format!("{table} as {alias}");
            }
            alias_clauses(&mut join.on, &rename);
        }
        alias_clauses(&mut self.wheres, &rename);
        alias_clauses(&mut self.havings, &rename);
        for group in &mut self.groups {
            if let Expr::Column(column) = group {
                rename(column);
            }
        }
        for order in &mut self.orders {
            if let Order::Column { column, .. } = order {
                rename(column);
            }
        }
    }
}

/// [`QueryBuilder::alias_table`] on a list of conditions.
fn alias_clauses(clauses: &mut [Clause], rename: &impl Fn(&mut String)) {
    for clause in clauses {
        match &mut clause.condition {
            Condition::Basic { column, .. }
            | Condition::In { column, .. }
            | Condition::InSub { column, .. }
            | Condition::Null { column, .. }
            | Condition::Between { column, .. }
            | Condition::Json { column, .. } => rename(column),
            Condition::Column { first, second, .. } => {
                rename(first);
                rename(second);
            }
            Condition::InRows { columns, .. } => columns.iter_mut().for_each(rename),
            Condition::Group(clauses) => alias_clauses(clauses, rename),
            Condition::Exists { .. } | Condition::Raw(_) => {}
        }
    }
}
