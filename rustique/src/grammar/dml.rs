//! INSERT, UPDATE and DELETE statements.

use super::writer::Writer;
use super::{Compiled, Dialect, check_query, conditions, mysql, oracle, select};
use crate::query::QueryBuilder;
use crate::{Error, Result, Value};

pub(crate) fn insert(
    dialect: Dialect,
    table: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
) -> Result<Compiled> {
    check_insert(table, columns, &rows)?;
    let mut w = Writer::new(dialect);
    match dialect {
        // No columns: `INSERT INTO t () VALUES ()` fills every column with its default.
        Dialect::MySql => mysql::insert(&mut w, table, columns, rows),
        Dialect::Oracle if columns.is_empty() => w.fail(Error::InvalidQuery(
            "an Oracle insert needs at least one column".to_owned(),
        )),
        Dialect::Oracle => oracle::insert(&mut w, table, columns, rows),
    }
    w.finish()
}

/// `INSERT INTO t (a, b) SELECT ...`, like Laravel's `insertUsing`; without
/// columns, `INSERT INTO t SELECT ...` fills every column in table order.
/// The source's bindings follow the target's, so its `IN` lists (Oracle's
/// JSON array included) work as in a plain `SELECT`.
pub(crate) fn insert_using(
    dialect: Dialect,
    table: &str,
    columns: &[&str],
    source: &QueryBuilder,
) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    if table.is_empty() {
        w.fail(Error::InvalidQuery("insert needs a table".to_owned()));
    }
    w.push("INSERT INTO ");
    if columns.is_empty() {
        w.table(table);
    } else {
        target(&mut w, table, columns);
    }
    w.push(" ");
    select::write(&mut w, source);
    w.finish()
}

/// Inserts `rows`; a row that clashes with an existing one on `unique_by`
/// updates that row's `update` columns instead. An empty `update` keeps the
/// existing row as it is.
///
/// MySQL matches on any unique index (`ON DUPLICATE KEY UPDATE`), Oracle on
/// `unique_by` (`MERGE`).
pub(crate) fn upsert(
    dialect: Dialect,
    table: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    unique_by: &[&str],
    update: &[&str],
) -> Result<Compiled> {
    check_insert(table, columns, &rows)?;
    if unique_by.is_empty() || !unique_by.iter().chain(update).all(|c| columns.contains(c)) {
        return Err(Error::InvalidQuery(
            "upsert needs `unique_by`, and `unique_by` and `update` must be inserted columns"
                .to_owned(),
        ));
    }
    // Oracle cannot update the columns it matches on (ORA-38104); they are equal anyway.
    let update: Vec<&str> = update
        .iter()
        .copied()
        .filter(|c| !unique_by.contains(c))
        .collect();
    let mut w = Writer::new(dialect);
    match dialect {
        Dialect::MySql => mysql::upsert(&mut w, table, columns, rows, unique_by, &update),
        Dialect::Oracle => oracle::upsert(&mut w, table, columns, rows, unique_by, &update),
    }
    w.finish()
}

fn check_insert(table: &str, columns: &[&str], rows: &[Vec<Value>]) -> Result<()> {
    if table.is_empty() || rows.is_empty() {
        return Err(Error::InvalidQuery(
            "insert needs a table and at least one row".to_owned(),
        ));
    }
    if let Some(row) = rows.iter().find(|row| row.len() != columns.len()) {
        return Err(Error::InvalidQuery(format!(
            "insert row has {} values for {} columns",
            row.len(),
            columns.len()
        )));
    }
    Ok(())
}

/// Inserts one row and reports the key the database generated for `key`:
/// MySQL through `LAST_INSERT_ID()`, Oracle through `RETURNING ... INTO`.
/// With `sequence`, Oracle fills `key` from `sequence.NEXTVAL`; MySQL ignores it.
/// Without columns, Oracle inserts `DEFAULT` into `key` (identity or trigger).
pub(crate) fn insert_get_id(
    dialect: Dialect,
    table: &str,
    columns: &[&str],
    row: Vec<Value>,
    key: &str,
    sequence: Option<&str>,
) -> Result<Compiled> {
    if dialect == Dialect::MySql {
        return insert(dialect, table, columns, vec![row]);
    }
    let mut w = Writer::new(dialect);
    if table.is_empty() || row.len() != columns.len() {
        w.fail(Error::InvalidQuery(format!(
            "insert needs a table and one value per column, got {} values for {} columns",
            row.len(),
            columns.len()
        )));
    }
    oracle::insert_get_id(&mut w, table, columns, row, key, sequence);
    w.finish()
}

/// Oracle: inserts `rows` in one PL/SQL block, one `INSERT ... RETURNING`
/// per row, handing back the key generated for each as an out bind. MySQL
/// has no equivalent; there, call [`insert_get_id`] per row.
pub(crate) fn insert_get_ids(
    dialect: Dialect,
    table: &str,
    columns: &[&str],
    rows: Vec<Vec<Value>>,
    key: &str,
    sequence: Option<&str>,
) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    if dialect != Dialect::Oracle {
        w.fail(Error::Unsupported {
            dialect,
            feature: "inserting several rows that each report their key",
        });
    }
    check_insert(table, columns, &rows)?;
    oracle::insert_get_ids(&mut w, table, columns, rows, key, sequence);
    w.finish()
}

pub(crate) fn update(
    dialect: Dialect,
    query: &QueryBuilder,
    values: Vec<(&str, Value)>,
) -> Result<Compiled> {
    update_adding(dialect, query, None, values)
}

/// `UPDATE t SET column = column + ?, other = ? WHERE ...`
pub(crate) fn increment(
    dialect: Dialect,
    query: &QueryBuilder,
    column: &str,
    amount: Value,
    values: Vec<(&str, Value)>,
) -> Result<Compiled> {
    update_adding(dialect, query, Some((column, amount)), values)
}

fn update_adding(
    dialect: Dialect,
    query: &QueryBuilder,
    add: Option<(&str, Value)>,
    values: Vec<(&str, Value)>,
) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    check_filter_only(&mut w, query, "update");
    if values.is_empty() && add.is_none() {
        w.fail(Error::InvalidQuery(
            "update needs at least one column".to_owned(),
        ));
    }
    w.push("UPDATE ");
    w.table(&query.table);
    w.push(" SET ");
    if let Some((column, amount)) = add {
        w.column(column);
        w.push(" = ");
        w.column(column);
        w.push(" + ");
        w.bind(amount);
        if !values.is_empty() {
            w.push(", ");
        }
    }
    w.list(values, |w, (column, value)| {
        w.column(column);
        w.push(" = ");
        w.bind(value);
    });
    conditions::clause(&mut w, " WHERE ", &query.wheres);
    w.finish()
}

pub(crate) fn delete(dialect: Dialect, query: &QueryBuilder) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    check_filter_only(&mut w, query, "delete");
    w.push("DELETE FROM ");
    w.table(&query.table);
    conditions::clause(&mut w, " WHERE ", &query.wheres);
    w.finish()
}

/// `t (A, B)`: the target of an INSERT.
pub(super) fn target(w: &mut Writer, table: &str, columns: &[&str]) {
    target_with(w, table, columns.iter().copied());
}

pub(super) fn target_with<'a>(
    w: &mut Writer,
    table: &str,
    columns: impl IntoIterator<Item = &'a str>,
) {
    w.table(table);
    w.push(" (");
    w.list(columns, Writer::column);
    w.push(")");
}

/// `(?, ?)`: one row of an INSERT.
pub(super) fn values(w: &mut Writer, row: Vec<Value>) {
    w.push("(");
    w.list(row, Writer::bind);
    w.push(")");
}

/// UPDATE and DELETE only use the WHERE clauses. Anything else would be
/// silently dropped and touch more rows than intended, so it is an error.
fn check_filter_only(w: &mut Writer, query: &QueryBuilder, statement: &str) {
    check_query(w, query);
    let extra = query.from_sub.is_some()
        || !query.joins.is_empty()
        || !query.groups.is_empty()
        || !query.havings.is_empty()
        || !query.orders.is_empty()
        || query.limit.is_some()
        || query.offset.is_some();
    if extra {
        w.fail(Error::InvalidQuery(format!(
            "{statement} does not support subqueries in FROM, joins, grouping, ordering or limits"
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn insert_get_id_per_dialect() {
        let row = || vec![Value::from("Ann")];

        let mysql =
            insert_get_id(Dialect::MySql, "users", &["name"], row(), "id", Some("seq")).unwrap();
        assert_eq!(mysql.sql, "INSERT INTO `users` (`name`) VALUES (?)");
        assert_eq!(mysql.returning, 0);

        let identity =
            insert_get_id(Dialect::Oracle, "users", &["name"], row(), "id", None).unwrap();
        assert_eq!(
            identity.sql,
            r#"INSERT INTO "USERS" ("NAME") VALUES (:1) RETURNING "ID" INTO :2"#
        );
        assert_eq!(identity.binds, row());
        assert_eq!(identity.returning, 1);

        let sequence = insert_get_id(
            Dialect::Oracle,
            "users",
            &["name"],
            row(),
            "id",
            Some("users_seq"),
        )
        .unwrap();
        assert_eq!(
            sequence.sql,
            r#"INSERT INTO "USERS" ("ID", "NAME") VALUES ("USERS_SEQ".NEXTVAL, :1) RETURNING "ID" INTO :2"#
        );

        let only_key =
            insert_get_id(Dialect::Oracle, "t", &[], vec![], "id", Some("t_seq")).unwrap();
        assert_eq!(
            only_key.sql,
            r#"INSERT INTO "T" ("ID") VALUES ("T_SEQ".NEXTVAL) RETURNING "ID" INTO :1"#
        );

        let identity_only = insert_get_id(Dialect::Oracle, "t", &[], vec![], "id", None).unwrap();
        assert_eq!(
            identity_only.sql,
            r#"INSERT INTO "T" ("ID") VALUES (DEFAULT) RETURNING "ID" INTO :1"#
        );
        let mysql_only = insert_get_id(Dialect::MySql, "t", &[], vec![], "id", None).unwrap();
        assert_eq!(mysql_only.sql, "INSERT INTO `t` () VALUES ()");
    }

    #[test]
    fn insert_get_ids_is_one_oracle_block() {
        let rows = vec![vec![Value::from("Ann")], vec![Value::from("Bo")]];
        let block = insert_get_ids(
            Dialect::Oracle,
            "users",
            &["name"],
            rows.clone(),
            "id",
            None,
        )
        .unwrap();
        assert_eq!(
            block.sql,
            "DECLARE orm_id1 NUMBER; orm_id2 NUMBER; BEGIN \
             INSERT INTO \"USERS\" (\"NAME\") VALUES (:1) RETURNING \"ID\" INTO orm_id1; \
             INSERT INTO \"USERS\" (\"NAME\") VALUES (:2) RETURNING \"ID\" INTO orm_id2; \
             :3 := orm_id1; :4 := orm_id2; END;"
        );
        assert_eq!(block.binds, rows.concat());
        assert_eq!(block.returning, 2);
        assert!(insert_get_ids(Dialect::MySql, "users", &["name"], rows, "id", None).is_err());
    }
}
