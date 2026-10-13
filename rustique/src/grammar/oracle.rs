//! Oracle 19c specifics.
//!
//! Identifiers are upper-cased and quoted: `email` becomes `"EMAIL"`, which
//! matches columns created unquoted (Oracle folds those to upper case) and
//! keeps reserved words such as `level` or `size` usable.

use super::dml;
use super::writer::Writer;
use crate::query::{Lock, QueryBuilder};
use crate::schema::{Column, ColumnType};
use crate::{Dialect, Error, Result, Value};

/// Oracle rejects `AS` before a table alias.
pub(super) const TABLE_ALIAS: &str = " ";

/// Oracle allows at most 1000 items in one `IN` list (ORA-01795).
pub(super) const MAX_IN_LIST: usize = 1000;

/// Upper-cases and double-quotes an identifier. Oracle identifiers cannot
/// contain `"` at all, so those are rejected rather than escaped.
pub(super) fn quote(out: &mut String, ident: &str) -> Result<()> {
    if ident.contains('"') {
        return Err(Error::InvalidQuery(format!(
            "Oracle identifier cannot contain `\"`: {ident}"
        )));
    }
    out.push('"');
    out.push_str(&ident.to_uppercase());
    out.push('"');
    Ok(())
}

/// Row limiting clause (12c+): `OFFSET n ROWS FETCH NEXT m ROWS ONLY`.
pub(super) fn limit_offset(w: &mut Writer, limit: Option<u64>, offset: Option<u64>) {
    if let Some(offset) = offset {
        w.push(" OFFSET ");
        w.push_u64(offset);
        w.push(" ROWS");
    }
    if let Some(limit) = limit {
        w.push(" FETCH NEXT ");
        w.push_u64(limit);
        w.push(" ROWS ONLY");
    }
}

/// Whether a limited `FOR UPDATE` query can lock through `ROWID IN (...)`
/// (see `select::locked_window`): one table, no grouping, and no `SKIP
/// LOCKED`, which would skip locked rows only after the limit picked them.
pub(super) fn can_lock_window(query: &QueryBuilder, lock: Lock) -> bool {
    !lock.skip_locked
        && query.from_sub.is_none()
        && query.joins.is_empty()
        && !query.distinct
        && query.groups.is_empty()
        && query.havings.is_empty()
        && query.unions.is_empty()
}

pub(super) fn lock(w: &mut Writer, lock: Lock, limited: bool) {
    if lock.shared {
        w.fail(Error::Unsupported {
            dialect: Dialect::Oracle,
            feature: "row-level shared locks (FOR SHARE)",
        });
    }
    if limited {
        w.fail(Error::Unsupported {
            dialect: Dialect::Oracle,
            feature: "FOR UPDATE with limit or offset on joins, grouping or SKIP LOCKED (ORA-02014)",
        });
    }
    w.push(" FOR UPDATE");
    if lock.skip_locked {
        w.push(" SKIP LOCKED");
    }
}

/// `INSERT INTO t ("ID", "A") VALUES ("T_SEQ".NEXTVAL, :1) RETURNING "ID" INTO :2`,
/// without the key column when there is no sequence, or with `DEFAULT` for
/// it when there are no other columns.
pub(super) fn insert_get_id(
    w: &mut Writer,
    table: &str,
    columns: &[&str],
    row: Vec<Value>,
    key: &str,
    sequence: Option<&str>,
) {
    insert_returning(w, table, columns, row, key, sequence);
    w.out_bind();
}

/// `DECLARE orm_id1 NUMBER; ... BEGIN INSERT ... RETURNING "ID" INTO orm_id1;
/// ... :3 := orm_id1; ... END;`: [`insert_get_id`] for several rows in one
/// round trip, the keys handed back as out binds after every in bind.
pub(super) fn insert_get_ids(
    w: &mut Writer,
    table: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    key: &str,
    sequence: Option<&str>,
) {
    let count = rows.len();
    w.push("DECLARE");
    for i in 1..=count {
        w.push(&format!(" orm_id{i} NUMBER;"));
    }
    w.push(" BEGIN ");
    for (i, row) in rows.into_iter().enumerate() {
        insert_returning(w, table, columns, row, key, sequence);
        w.push(&format!("orm_id{}; ", i + 1));
    }
    for i in 1..=count {
        w.out_bind();
        w.push(&format!(" := orm_id{i}; "));
    }
    w.push("END;");
}

/// `INSERT INTO t (...) VALUES (...) RETURNING "ID" INTO `, the target left to the caller.
fn insert_returning(
    w: &mut Writer,
    table: &str,
    columns: &[&str],
    row: Vec<Value>,
    key: &str,
    sequence: Option<&str>,
) {
    let generated = (sequence.is_some() || columns.is_empty()).then_some(key);
    w.push("INSERT INTO ");
    dml::target_with(
        w,
        table,
        generated.into_iter().chain(columns.iter().copied()),
    );
    w.push(" VALUES (");
    if generated.is_some() {
        match sequence {
            Some(sequence) => {
                w.table(sequence);
                w.push(".NEXTVAL");
            }
            None => w.push("DEFAULT"),
        }
        if !row.is_empty() {
            w.push(", ");
        }
    }
    w.list(row, Writer::bind);
    w.push(") RETURNING ");
    w.column(key);
    w.push(" INTO ");
}

/// One row: `INSERT INTO t (A, B) VALUES (:1, :2)`.
/// Several rows: the same single-row statement, with the other rows' values
/// appended for array binding (see [`Compiled::batch`](super::Compiled::batch)).
/// Oracle 19c has no multi-row `VALUES`, and `INSERT ALL` gives every row the
/// same identity value.
pub(super) fn insert(w: &mut Writer, table: &str, columns: &[&str], mut rows: Vec<Vec<Value>>) {
    let rest = rows.split_off(1);
    w.push("INSERT INTO ");
    dml::target(w, table, columns);
    w.push(" VALUES ");
    w.list(rows, dml::values);
    if !rest.is_empty() {
        w.batch(rest);
    }
}

/// `MERGE INTO t USING (SELECT :1 AS A, ... FROM DUAL) s ON (t.A = s.A)
/// WHEN MATCHED THEN UPDATE ... WHEN NOT MATCHED THEN INSERT ...`, one row per
/// run; further rows are array-bound like [`insert`].
pub(super) fn upsert(
    w: &mut Writer,
    table: &str,
    columns: &[&str],
    mut rows: Vec<Vec<Value>>,
    unique_by: &[&str],
    update: &[&str],
) {
    let rest = rows.split_off(1);
    let row = rows.into_iter().flatten();
    let qualified = |alias: &str, column: &str| format!("{alias}.{column}");
    w.push("MERGE INTO ");
    w.table(&format!("{table} as t"));
    w.push(" USING (SELECT ");
    w.list(columns.iter().zip(row), |w, (column, value)| {
        w.bind(value);
        w.push(" AS ");
        w.column(column);
    });
    w.push(" FROM DUAL) ");
    w.column("s");
    w.push(" ON (");
    for (i, column) in unique_by.iter().enumerate() {
        if i > 0 {
            w.push(" AND ");
        }
        w.column(&qualified("t", column));
        w.push(" = ");
        w.column(&qualified("s", column));
    }
    w.push(")");
    if !update.is_empty() {
        w.push(" WHEN MATCHED THEN UPDATE SET ");
        w.list(update, |w, column| {
            w.column(&qualified("t", column));
            w.push(" = ");
            w.column(&qualified("s", column));
        });
    }
    w.push(" WHEN NOT MATCHED THEN INSERT (");
    w.list(columns, |w, column| w.column(column));
    w.push(") VALUES (");
    w.list(columns, |w, column| w.column(&qualified("s", column)));
    w.push(")");
    if !rest.is_empty() {
        w.batch(rest);
    }
}

pub(super) const HAS_TABLE: &str =
    "SELECT 1 FROM all_tables WHERE owner = NVL(?, USER) AND table_name = ?";

pub(super) const HAS_COLUMN: &str = "SELECT 1 FROM all_tab_columns \
     WHERE owner = NVL(?, USER) AND table_name = ? AND column_name = ?";

pub(super) fn column_type(ty: ColumnType) -> String {
    match ty {
        ColumnType::BigInteger => "NUMBER(19)".to_owned(),
        ColumnType::Integer => "NUMBER(10)".to_owned(),
        ColumnType::SmallInteger => "NUMBER(5)".to_owned(),
        ColumnType::Boolean => "NUMBER(1)".to_owned(),
        ColumnType::String(length) => format!("VARCHAR2({length} CHAR)"),
        ColumnType::Char(length) => format!("CHAR({length} CHAR)"),
        ColumnType::Text | ColumnType::LongText | ColumnType::Json => "CLOB".to_owned(),
        ColumnType::Double => "BINARY_DOUBLE".to_owned(),
        ColumnType::Decimal(precision, scale) => format!("NUMBER({precision}, {scale})"),
        ColumnType::Date => "DATE".to_owned(),
        ColumnType::DateTime => "TIMESTAMP(6)".to_owned(),
        ColumnType::Time => "VARCHAR2(18 CHAR)".to_owned(),
        ColumnType::Binary => "BLOB".to_owned(),
        ColumnType::Uuid => "CHAR(36)".to_owned(),
    }
}

/// 19c has no `BOOLEAN` or `JSON` column types; a check constraint stands in.
pub(super) fn check(w: &mut Writer, column: &Column) {
    let condition = match column.ty {
        ColumnType::Boolean => " IN (0, 1))",
        ColumnType::Json => " IS JSON)",
        _ => return,
    };
    w.push(" CHECK (");
    w.column(&column.name);
    w.push(condition);
}

/// Wraps a `DROP TABLE` in PL/SQL that ignores ORA-00942 (no such table):
/// 19c has no `DROP TABLE IF EXISTS`.
pub(super) fn ignore_missing_table(drop: &str) -> String {
    format!(
        "BEGIN EXECUTE IMMEDIATE '{}'; \
         EXCEPTION WHEN OTHERS THEN IF SQLCODE != -942 THEN RAISE; END IF; END;",
        drop.replace('\'', "''")
    )
}
