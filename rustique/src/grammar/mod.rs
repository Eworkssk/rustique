//! Compiles a [`QueryBuilder`] into SQL text plus bindings.
//!
//! - `mod.rs`: [`Dialect`], [`Compiled`] and checks shared by every statement.
//! - `writer.rs`: the SQL buffer: identifier quoting, placeholders, raw fragments.
//! - `select.rs`: SELECT statements.
//! - `conditions.rs`: WHERE / HAVING / ON lists.
//! - `dml.rs`: INSERT, UPDATE and DELETE.
//! - `schema.rs`: DDL from a [`Blueprint`](crate::Blueprint).
//! - `mysql.rs`, `oracle.rs`: everything that differs between the two databases.

mod conditions;
mod dml;
mod mysql;
mod oracle;
mod schema;
mod select;
mod writer;

use std::fmt;

pub(crate) use dml::{delete, increment, insert, insert_get_id, insert_get_ids, update, upsert};
pub(crate) use schema::{blueprint, drop_table, has_table, rename_table};
pub(crate) use select::{aggregate, select};

use crate::query::{QueryBuilder, Raw};
use crate::{Error, Result, Value};
use writer::Writer;

/// Raw SQL with `?` placeholders, rewritten to the dialect's placeholders.
pub(crate) fn raw(
    dialect: Dialect,
    sql: &str,
    binds: impl IntoIterator<Item = Value>,
) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    w.raw(&Raw::new(sql, binds));
    w.finish()
}

/// Oracle `LOCK TABLE` in exclusive mode, waiting `wait` seconds (`None`:
/// forever) before ORA-30006.
pub(crate) fn oracle_lock_table(table: &str, wait: Option<u64>) -> Result<String> {
    let mut sql = "LOCK TABLE ".to_owned();
    oracle::quote(&mut sql, table)?;
    sql.push_str(" IN EXCLUSIVE MODE");
    if let Some(wait) = wait {
        sql.push_str(&format!(" WAIT {wait}"));
    }
    Ok(sql)
}

/// SQL dialect a query is compiled for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Dialect {
    /// MySQL 8.4+.
    MySql,
    /// Oracle 19c+.
    Oracle,
}

impl fmt::Display for Dialect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::MySql => "MySQL",
            Self::Oracle => "Oracle",
        })
    }
}

/// Most bindings one statement may have: MySQL counts placeholders in a
/// 16-bit field, and the `oracle` crate numbers binds the same way.
pub(crate) const MAX_BINDS: usize = 65_535;

/// Most rows of `columns` columns one `INSERT` or upsert may hold. Oracle
/// array-binds one row per execution, but the client buffers every row of
/// the array (each string column sized for its longest value), so batches
/// are capped to keep memory flat.
pub(crate) fn rows_per_statement(dialect: Dialect, columns: usize) -> usize {
    match dialect {
        Dialect::MySql => MAX_BINDS / columns.max(1),
        Dialect::Oracle => ORACLE_BATCH_ROWS,
    }
}

/// Rows per Oracle array-bound execution.
const ORACLE_BATCH_ROWS: usize = 1000;

/// Most rows per PL/SQL block of [`insert_get_ids`]: a block with one
/// `INSERT` per row is parsed like any statement, and few distinct block
/// sizes keep the statement cache useful.
pub(crate) const ORACLE_BLOCK_ROWS: usize = 100;

/// SQL text with its bindings, in placeholder order.
#[derive(Debug, Clone, PartialEq)]
pub struct Compiled {
    /// SQL text: `?` placeholders on MySQL, `:1, :2, ...` on Oracle.
    pub sql: String,
    /// Values for the placeholders.
    pub binds: Vec<Value>,
    /// `Some(n)`: `binds` holds `n` rows, one after another, for the same
    /// single-row statement, run once with array binding. Oracle uses this
    /// for multi-row `INSERT`; everything else is `None`.
    pub batch: Option<usize>,
    /// Number of out binds (Oracle only): the last placeholders, with no
    /// entry in `binds`. `1` for a statement ending with `RETURNING column
    /// INTO :n`, whose value comes back as
    /// [`Executed::last_insert_id`](crate::Executed::last_insert_id); more
    /// for the PL/SQL block of `Model::insert_many`, which hands back one
    /// generated key per row.
    pub returning: usize,
}

/// Reports mistakes recorded while building and a missing table.
fn check_query(w: &mut Writer, query: &QueryBuilder) {
    if let Some(message) = &query.error {
        w.fail(Error::InvalidQuery(message.clone()));
    }
    if query.table.is_empty() {
        w.fail(Error::InvalidQuery("query has no table".to_owned()));
    }
}
