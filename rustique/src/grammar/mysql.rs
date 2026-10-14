//! MySQL 8.4 specifics.

use super::dml;
use super::writer::Writer;
use crate::query::{Lock, LockWait};
use crate::schema::ColumnType;
use crate::{Dialect, Error, Result, Value};

pub(super) const TABLE_ALIAS: &str = " AS ";

/// Backtick-quotes an identifier; embedded backticks are doubled.
pub(super) fn quote(out: &mut String, ident: &str) -> Result<()> {
    out.push('`');
    out.push_str(&ident.replace('`', "``"));
    out.push('`');
    Ok(())
}

pub(super) fn limit_offset(w: &mut Writer, limit: Option<u64>, offset: Option<u64>) {
    match (limit, offset) {
        (None, None) => {}
        (Some(limit), offset) => {
            w.push(" LIMIT ");
            w.push_u64(limit);
            if let Some(offset) = offset {
                w.push(" OFFSET ");
                w.push_u64(offset);
            }
        }
        (None, Some(offset)) => {
            // MySQL has no OFFSET without LIMIT; its manual suggests the largest u64.
            w.push(" LIMIT 18446744073709551615 OFFSET ");
            w.push_u64(offset);
        }
    }
}

pub(super) fn lock(w: &mut Writer, lock: Lock) {
    w.push(if lock.shared {
        " FOR SHARE"
    } else {
        " FOR UPDATE"
    });
    match lock.wait {
        Some(LockWait::Seconds(_)) => w.fail(Error::Unsupported {
            dialect: Dialect::MySql,
            feature: "FOR UPDATE WAIT (set the session's innodb_lock_wait_timeout instead)",
        }),
        Some(LockWait::NoWait) => w.push(" NOWAIT"),
        None => {}
    }
    super::lock_wait_or_skip(w, lock);
}

/// `INSERT INTO t (a, b) VALUES (?, ?), (?, ?)`
pub(super) fn insert(w: &mut Writer, table: &str, columns: &[&str], rows: Vec<Vec<Value>>) {
    w.push("INSERT INTO ");
    dml::target(w, table, columns);
    w.push(" VALUES ");
    w.list(rows, dml::values);
}

/// `INSERT ... AS new ON DUPLICATE KEY UPDATE b = new.b` (8.0.19+; `VALUES()` is deprecated).
pub(super) fn upsert(
    w: &mut Writer,
    table: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    unique_by: &[&str],
    update: &[&str],
) {
    insert(w, table, columns, rows);
    w.push(" AS ");
    w.column("new");
    w.push(" ON DUPLICATE KEY UPDATE ");
    if update.is_empty() {
        // `t.a = t.a` keeps the existing row; `INSERT IGNORE` would also hide other errors.
        let column = format!("{table}.{}", unique_by[0]);
        w.column(&column);
        w.push(" = ");
        w.column(&column);
    }
    w.list(update, |w, column| {
        w.column(column);
        w.push(" = ");
        w.column(&format!("new.{column}"));
    });
}

pub(super) const HAS_TABLE: &str = "SELECT 1 FROM information_schema.tables \
     WHERE table_schema = COALESCE(?, DATABASE()) AND table_name = ?";

pub(super) const HAS_COLUMN: &str = "SELECT 1 FROM information_schema.columns \
     WHERE table_schema = COALESCE(?, DATABASE()) AND table_name = ? AND column_name = ?";

pub(super) fn column_type(ty: ColumnType) -> String {
    match ty {
        ColumnType::BigInteger => "BIGINT".to_owned(),
        ColumnType::Integer => "INT".to_owned(),
        ColumnType::SmallInteger => "SMALLINT".to_owned(),
        ColumnType::Boolean => "TINYINT(1)".to_owned(),
        ColumnType::String(length) => format!("VARCHAR({length})"),
        ColumnType::Char(length) => format!("CHAR({length})"),
        ColumnType::Text => "TEXT".to_owned(),
        ColumnType::LongText => "LONGTEXT".to_owned(),
        ColumnType::Double => "DOUBLE".to_owned(),
        ColumnType::Decimal(precision, scale) => format!("DECIMAL({precision}, {scale})"),
        ColumnType::Date => "DATE".to_owned(),
        ColumnType::DateTime => "DATETIME(6)".to_owned(),
        ColumnType::Time => "TIME(6)".to_owned(),
        ColumnType::Json => "JSON".to_owned(),
        ColumnType::Binary => "LONGBLOB".to_owned(),
        ColumnType::Uuid => "CHAR(36)".to_owned(),
    }
}

/// TEXT, JSON and BLOB columns take a default only as an expression: `DEFAULT ('x')`.
pub(super) fn default_needs_parens(ty: ColumnType) -> bool {
    matches!(
        ty,
        ColumnType::Text | ColumnType::LongText | ColumnType::Json | ColumnType::Binary
    )
}
