//! Methods of [`QueryBuilder`] that run the query: `get`, `first`, `count`,
//! `insert`, `update`, `delete`, ...

use super::{Aggregate, Expr, QueryBuilder, Raw};
use crate::db::{Cursor, Executed, Executor};
use crate::grammar::{self, Compiled};
use crate::{Collection, Error, FromValue, Result, Row, Value};

impl QueryBuilder {
    /// Runs the query and returns every row.
    ///
    /// # Errors
    ///
    /// Compile errors (see [`to_sql`](Self::to_sql)) and database errors.
    pub async fn get(&self, db: &impl Executor) -> Result<Collection<Row>> {
        Ok(db.fetch_all(self.to_sql(db.dialect())?).await?.into())
    }

    /// Runs the query and streams its rows one at a time instead of
    /// collecting them, like Laravel's `cursor()`: memory stays flat for
    /// results of any size.
    ///
    /// ```no_run
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let mut rows = rustique::QueryBuilder::table("events").cursor(db)?;
    /// while let Some(row) = rows.next().await? {
    ///     let id: i64 = row.get("id")?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// Compile errors (see [`to_sql`](Self::to_sql)); database errors come
    /// from [`Cursor::next`].
    pub fn cursor<'a>(&self, db: &'a impl Executor) -> Result<Cursor<'a>> {
        Ok(db.cursor(self.to_sql(db.dialect())?))
    }

    /// As [`cursor`](Self::cursor), reading up to `buffer` rows ahead
    /// instead of 100: more for fewer round trips, fewer to hold less in
    /// memory.
    ///
    /// # Errors
    ///
    /// As [`cursor`](Self::cursor).
    pub fn cursor_buffered<'a>(&self, db: &'a impl Executor, buffer: usize) -> Result<Cursor<'a>> {
        Ok(db.cursor_buffered(self.to_sql(db.dialect())?, buffer))
    }

    /// Runs the query with `LIMIT 1` and returns the row, if any.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get). On Oracle, combining this with
    /// [`lock_for_update`](Self::lock_for_update) is rejected (ORA-02014).
    pub async fn first(&self, db: &impl Executor) -> Result<Option<Row>> {
        let rows = self.clone().limit(1).get(db).await?;
        Ok(rows.into_iter().next())
    }

    /// Returns whether the query matches at least one row.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn exists(&self, db: &impl Executor) -> Result<bool> {
        let mut query = self
            .clone()
            .limit(self.limit.map_or(1, |limit| limit.min(1)));
        // Union parts must keep matching columns, and HAVING may name a selected alias.
        if query.unions.is_empty() && query.havings.is_empty() {
            query.columns = vec![Expr::Raw(Raw::new("1", []))];
        }
        query.lock = None;
        Ok(!query.get(db).await?.is_empty())
    }

    /// `COUNT(*)` of the matching rows.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn count(&self, db: &impl Executor) -> Result<u64> {
        self.aggregate(db, Aggregate::Count, None).await
    }

    /// `MAX(column)`. Use `Option<T>`: it is `NULL` when no row matches.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get), plus [`Error::Decode`] when the result does not fit `T`.
    pub async fn max<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.aggregate(db, Aggregate::Max, Some(column)).await
    }

    /// `MIN(column)`. Use `Option<T>`: it is `NULL` when no row matches.
    ///
    /// # Errors
    ///
    /// As [`max`](Self::max).
    pub async fn min<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.aggregate(db, Aggregate::Min, Some(column)).await
    }

    /// `SUM(column)`. Use `Option<T>`: it is `NULL` when no row matches.
    ///
    /// # Errors
    ///
    /// As [`max`](Self::max).
    pub async fn sum<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.aggregate(db, Aggregate::Sum, Some(column)).await
    }

    /// `AVG(column)`. Use `Option<T>`: it is `NULL` when no row matches.
    ///
    /// # Errors
    ///
    /// As [`max`](Self::max).
    pub async fn avg<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        self.aggregate(db, Aggregate::Avg, Some(column)).await
    }

    /// The values of `column` from every matching row. `column` may be
    /// qualified (`users.email`) or aliased (`email as e`).
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get), plus [`Error::Decode`] when a value does not fit `T`.
    pub async fn pluck<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<Vec<T>> {
        let rows = self.clone().select([column]).get(db).await?;
        let name = output_name(column);
        rows.iter().map(|row| row.get(name)).collect()
    }

    /// `column` of the first matching row, like Laravel's `value`. Use
    /// `Option<T>`: it is `NULL` when no row matches. `column` may be
    /// qualified or aliased, as in [`pluck`](Self::pluck).
    ///
    /// # Errors
    ///
    /// As [`pluck`](Self::pluck).
    pub async fn value<T: FromValue>(&self, db: &impl Executor, column: &str) -> Result<T> {
        let row = self.clone().select([column]).first(db).await?;
        let name = output_name(column);
        match row {
            Some(row) => row.get(name),
            None => T::from_value(Value::Null).map_err(|source| Error::decode(None, name, source)),
        }
    }

    /// `(key, value)` pairs from every matching row, like Laravel's
    /// `pluck(value, key)`; collect them into a map if needed. Columns may be
    /// qualified or aliased, as in [`pluck`](Self::pluck).
    ///
    /// # Errors
    ///
    /// As [`pluck`](Self::pluck).
    pub async fn pluck_keyed<K: FromValue, V: FromValue>(
        &self,
        db: &impl Executor,
        value: &str,
        key: &str,
    ) -> Result<Vec<(K, V)>> {
        let rows = self.clone().select([key, value]).get(db).await?;
        let (key, value) = (output_name(key), output_name(value));
        rows.iter()
            .map(|row| Ok((row.get(key)?, row.get(value)?)))
            .collect()
    }

    /// Page `page` (1-based) of `per_page` rows, plus the total row count.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn paginate(
        &self,
        db: &impl Executor,
        page: u64,
        per_page: u64,
    ) -> Result<Page<Row>> {
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

    /// Page `page` (1-based) of `per_page` rows without counting them, like
    /// Laravel's `simplePaginate`: one query fetching one row more than
    /// asked tells whether a next page exists.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub async fn simple_paginate(
        &self,
        db: &impl Executor,
        page: u64,
        per_page: u64,
    ) -> Result<SimplePage<Row>> {
        let (page, per_page) = (page.max(1), per_page.max(1));
        let rows = self.simple_page_rows(page, per_page).get(db).await?;
        Ok(SimplePage::new(rows.into_inner(), page, per_page))
    }

    /// The query of a [`simple_paginate`](Self::simple_paginate) page: one row more than the page.
    pub(crate) fn simple_page_rows(&self, page: u64, per_page: u64) -> Self {
        self.clone()
            .offset((page - 1).saturating_mul(per_page))
            .limit(per_page.saturating_add(1))
    }

    /// Walks the matching rows in chunks of `size` with `LIMIT` / `OFFSET`,
    /// like Laravel's `chunk`. The query needs an [`order_by`](Self::order_by)
    /// on unique columns, or chunks may repeat or skip rows; rows that are
    /// inserted, deleted or changed so they move while chunking shift later
    /// chunks too, so prefer [`chunk_by_id`](Self::chunk_by_id) when changing
    /// the rows being walked.
    ///
    /// ```no_run
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let mut chunks = rustique::QueryBuilder::table("users").order_by("email").order_by("id").chunk(500);
    /// while let Some(rows) = chunks.next(db).await? {
    ///     println!("{} rows", rows.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// Replaces the query's limit and offset. Without ordering every chunk
    /// fails with [`Error::InvalidQuery`].
    pub fn chunk(&self, size: u64) -> Chunks {
        let mut query = self.clone();
        if query.orders.is_empty() {
            query.fail("`chunk` needs `order_by`; or use `chunk_by_id`".to_owned());
        }
        query.offset = None;
        Chunks {
            query: query.limit(size.max(1)),
            columns: Vec::new(),
            last: Vec::new(),
            offset: 0,
            done: false,
        }
    }

    /// Walks the matching rows in chunks of `size`, ordered by `column`, which
    /// must be unique and selected. Each chunk is one query filtered on
    /// `column > last value seen`, so rows changed between chunks are neither
    /// skipped nor repeated, unlike `OFFSET` paging.
    ///
    /// Replaces the query's ordering, limit and offset.
    ///
    /// ```no_run
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let mut chunks = rustique::QueryBuilder::table("users").chunk_by_id(500, "id");
    /// while let Some(rows) = chunks.next(db).await? {
    ///     println!("{} rows", rows.len());
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub fn chunk_by_id(&self, size: u64, column: impl Into<String>) -> Chunks {
        self.chunk_by_columns(size, vec![column.into()])
    }

    /// [`chunk_by_id`](Self::chunk_by_id) on several columns that are
    /// unique together, such as a composite key.
    pub(crate) fn chunk_by_columns(&self, size: u64, columns: Vec<String>) -> Chunks {
        let mut query = self.clone();
        // `a OR b AND id > ?` would only page `b`.
        query.group_or_wheres();
        query.orders.clear();
        query.offset = None;
        let query = columns
            .iter()
            .fold(query, |query, column| query.order_by(column.clone()));
        Chunks {
            query: query.limit(size.max(1)),
            columns,
            last: Vec::new(),
            offset: 0,
            done: false,
        }
    }

    async fn aggregate<T: FromValue>(
        &self,
        db: &impl Executor,
        function: Aggregate,
        column: Option<&str>,
    ) -> Result<T> {
        let compiled = grammar::aggregate(db.dialect(), self, function, column)?;
        let rows = db.fetch_all(compiled).await?;
        let row = rows
            .first()
            .ok_or_else(|| Error::ColumnNotFound("aggregate".to_owned()))?;
        row.get("aggregate")
    }

    /// Inserts `rows` into this query's table; each row has one value per column.
    ///
    /// Rows beyond MySQL's 65,535 placeholders, or Oracle's 1,000-row array
    /// binds, go into further statements, all or nothing; `last_insert_id`
    /// is then the first statement's.
    ///
    /// # Errors
    ///
    /// As [`to_insert_sql`](Self::to_insert_sql), plus database errors.
    pub async fn insert(
        &self,
        db: &impl Executor,
        columns: &[&str],
        rows: Vec<Vec<Value>>,
    ) -> Result<Executed> {
        let executed = self.insert_batches(db, columns, rows).await?;
        Ok(Executed {
            rows_affected: executed.iter().map(|e| e.rows_affected).sum(),
            last_insert_id: executed.first().and_then(|e| e.last_insert_id),
        })
    }

    /// [`insert`](Self::insert), one [`Executed`] per statement of at most
    /// [`grammar::rows_per_statement`] rows.
    pub(crate) async fn insert_batches(
        &self,
        db: &impl Executor,
        columns: &[&str],
        rows: Vec<Vec<Value>>,
    ) -> Result<Vec<Executed>> {
        let dialect = db.dialect();
        batched(db, columns, rows, |rows| {
            self.to_insert_sql(dialect, columns, rows)
        })
        .await
    }

    /// Inserts `rows`, updating the `update` columns of rows that clash on
    /// `unique_by`; see [`to_upsert_sql`](Self::to_upsert_sql). The affected
    /// row count is as the database reports it (MySQL counts an updated row twice).
    ///
    /// # Errors
    ///
    /// As [`to_upsert_sql`](Self::to_upsert_sql), plus database errors.
    pub async fn upsert(
        &self,
        db: &impl Executor,
        columns: &[&str],
        rows: Vec<Vec<Value>>,
        unique_by: &[&str],
        update: &[&str],
    ) -> Result<u64> {
        let dialect = db.dialect();
        let executed = batched(db, columns, rows, |rows| {
            self.to_upsert_sql(dialect, columns, rows, unique_by, update)
        })
        .await?;
        Ok(executed.iter().map(|e| e.rows_affected).sum())
    }

    /// Updates the matching rows and returns how many changed.
    ///
    /// # Errors
    ///
    /// As [`to_update_sql`](Self::to_update_sql), plus database errors.
    pub async fn update(&self, db: &impl Executor, values: Vec<(&str, Value)>) -> Result<u64> {
        let compiled = self.to_update_sql(db.dialect(), values)?;
        Ok(db.execute(compiled).await?.rows_affected)
    }

    /// Adds `amount` to `column` of the matching rows (`column = column + ?`),
    /// sets `values` too, and returns how many rows changed. A negative
    /// `amount` subtracts; see also [`decrement`](Self::decrement).
    ///
    /// # Errors
    ///
    /// As [`update`](Self::update).
    pub async fn increment(
        &self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value>,
        values: Vec<(&str, Value)>,
    ) -> Result<u64> {
        let compiled = grammar::increment(db.dialect(), self, column, amount.into(), values)?;
        Ok(db.execute(compiled).await?.rows_affected)
    }

    /// Subtracts `amount` from `column` of the matching rows; see [`increment`](Self::increment).
    ///
    /// # Errors
    ///
    /// As [`update`](Self::update), plus [`Error::InvalidQuery`] when
    /// `amount` is not a number.
    pub async fn decrement(
        &self,
        db: &impl Executor,
        column: &str,
        amount: impl Into<Value>,
        values: Vec<(&str, Value)>,
    ) -> Result<u64> {
        let amount = negate(amount.into())?;
        self.increment(db, column, amount, values).await
    }

    /// Deletes the matching rows and returns how many were removed.
    ///
    /// # Errors
    ///
    /// As [`to_delete_sql`](Self::to_delete_sql), plus database errors.
    pub async fn delete(&self, db: &impl Executor) -> Result<u64> {
        Ok(db
            .execute(self.to_delete_sql(db.dialect())?)
            .await?
            .rows_affected)
    }
}

/// Runs `compile(rows)` as one statement, or in a transaction as several
/// when the rows exceed [`grammar::rows_per_statement`].
async fn batched(
    db: &impl Executor,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    compile: impl Fn(Vec<Vec<Value>>) -> Result<Compiled>,
) -> Result<Vec<Executed>> {
    let size = grammar::rows_per_statement(db.dialect(), columns.len());
    if rows.len() <= size {
        return Ok(vec![db.execute(compile(rows)?).await?]);
    }
    let mut rows = rows.into_iter();
    let mut statements = Vec::new();
    loop {
        let batch: Vec<_> = rows.by_ref().take(size).collect();
        if batch.is_empty() {
            break;
        }
        statements.push(compile(batch)?);
    }
    db.execute_all(statements).await
}

/// `-amount`, for `decrement`.
pub(crate) fn negate(amount: Value) -> Result<Value> {
    Ok(match amount {
        Value::I64(n) => n
            .checked_neg()
            .map(Value::I64)
            .ok_or_else(|| Error::InvalidQuery(format!("cannot decrement by {n}: too large")))?,
        Value::U64(n) => i64::try_from(n)
            .map(|n| Value::I64(-n))
            .map_err(|_| Error::InvalidQuery(format!("cannot decrement by {n}: too large")))?,
        Value::F64(n) => Value::F64(-n),
        Value::Decimal(n) => Value::Decimal(-n),
        other => {
            return Err(Error::InvalidQuery(format!(
                "cannot increment or decrement by {other:?}"
            )));
        }
    })
}

/// Name of a selected column in result rows: its alias, or the part after the last dot.
pub(crate) fn output_name(column: &str) -> &str {
    let column = match column.to_ascii_lowercase().find(" as ") {
        Some(at) => &column[at + 4..],
        None => column,
    };
    column.rsplit('.').next().unwrap_or(column).trim()
}

/// One page of results, from `paginate`.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    /// Rows or models on this page.
    pub items: Collection<T>,
    /// Number of rows across all pages.
    pub total: u64,
    /// This page's number, starting at 1.
    pub page: u64,
    /// Page size.
    pub per_page: u64,
}

impl<T> Page<T> {
    /// Number of the last page; 1 when there are no rows.
    #[must_use]
    pub fn last_page(&self) -> u64 {
        self.total.div_ceil(self.per_page.max(1)).max(1)
    }

    /// Whether pages follow this one.
    #[must_use]
    pub fn has_more_pages(&self) -> bool {
        self.page < self.last_page()
    }
}

/// One page of results, from `simple_paginate`: no total, only whether
/// another page follows.
#[derive(Debug, Clone, PartialEq)]
pub struct SimplePage<T> {
    /// Rows or models on this page.
    pub items: Collection<T>,
    /// This page's number, starting at 1.
    pub page: u64,
    /// Page size.
    pub per_page: u64,
    /// Whether pages follow this one.
    pub has_more: bool,
}

impl<T> SimplePage<T> {
    /// The page from up to `per_page + 1` rows; the extra row only tells
    /// that more follow.
    pub(crate) fn new(mut items: Vec<T>, page: u64, per_page: u64) -> Self {
        let has_more = items.len() as u64 > per_page;
        items.truncate(per_page as usize);
        Self {
            items: items.into(),
            page,
            per_page,
            has_more,
        }
    }

    /// Whether pages follow this one.
    #[must_use]
    pub fn has_more_pages(&self) -> bool {
        self.has_more
    }
}

/// Chunks of rows from [`QueryBuilder::chunk_by_id`] or
/// [`QueryBuilder::chunk`]; call [`next`](Self::next) until it returns `None`.
#[derive(Debug, Clone)]
#[must_use]
pub struct Chunks {
    query: QueryBuilder,
    /// Keyset columns; empty when paging by offset.
    columns: Vec<String>,
    /// Values of `columns` in the last row seen; empty before the first chunk.
    last: Vec<Value>,
    /// Rows read so far, when paging by offset.
    offset: u64,
    done: bool,
}

impl Chunks {
    /// The next chunk, or `None` after the last one.
    ///
    /// # Errors
    ///
    /// As [`QueryBuilder::get`], plus [`Error::ColumnNotFound`] when the
    /// chunk column is not selected.
    pub async fn next(&mut self, db: &impl Executor) -> Result<Option<Collection<Row>>> {
        if self.done {
            return Ok(None);
        }
        let rows = self.after_last().get(db).await?;
        let size = self.query.limit.unwrap_or(1);
        self.done = (rows.len() as u64) < size;
        self.offset += rows.len() as u64;
        let Some(last) = rows.last() else {
            return Ok(None);
        };
        self.last = self
            .columns
            .iter()
            .map(|column| last.get(output_name(column)))
            .collect::<Result<_>>()?;
        Ok(Some(rows))
    }

    /// The query for rows after the last one seen. Several columns compare
    /// as `a > ? OR (a = ? AND b > ?)`: Oracle has no row-value `>`.
    fn after_last(&self) -> QueryBuilder {
        let query = self.query.clone();
        if self.columns.is_empty() {
            return if self.offset > 0 {
                query.offset(self.offset)
            } else {
                query
            };
        }
        let pairs = || self.columns.iter().cloned().zip(self.last.iter().cloned());
        match self.last.len() {
            0 => query,
            1 => pairs().fold(query, |q, (column, last)| q.where_(column, ">", last)),
            n => query.where_group(|group| {
                (0..n).fold(group, |group, i| {
                    group.or_where_group(|q| {
                        let q = pairs()
                            .take(i)
                            .fold(q, |q, (column, last)| q.where_(column, "=", last));
                        let (column, last) = pairs().nth(i).expect("i < n");
                        q.where_(column, ">", last)
                    })
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_names() {
        assert_eq!(output_name("email"), "email");
        assert_eq!(output_name("users.email"), "email");
        assert_eq!(output_name("users.email AS e"), "e");
    }

    #[test]
    fn chunks_compare_composite_keys() {
        let mut chunks = QueryBuilder::table("t").chunk_by_columns(2, vec!["a".into(), "b".into()]);
        chunks.last = vec![Value::from(1), Value::from(2)];
        let sql = chunks.after_last().to_sql(crate::Dialect::MySql).unwrap();
        assert_eq!(
            sql.sql,
            "SELECT * FROM `t` WHERE ((`a` > ?) OR (`a` = ? AND `b` > ?)) \
             ORDER BY `a` ASC, `b` ASC LIMIT 2"
        );
        assert_eq!(sql.binds, [1.into(), 1.into(), 2.into()]);
    }

    #[test]
    fn page_counts() {
        let page = |total, page| Page::<()> {
            items: Collection::default(),
            total,
            page,
            per_page: 10,
        };
        assert_eq!(page(0, 1).last_page(), 1);
        assert_eq!(page(20, 1).last_page(), 2);
        assert_eq!(page(21, 2).last_page(), 3);
        assert!(page(21, 2).has_more_pages());
        assert!(!page(21, 3).has_more_pages());
    }
}
