//! SELECT statements.

use super::writer::Writer;
use super::{Compiled, Dialect, check_query, conditions, mysql, oracle};
use crate::query::{Aggregate, Expr, Join, JoinKind, Lock, Order, QueryBuilder, Raw};
use crate::{Error, Result};

pub(crate) fn select(dialect: Dialect, query: &QueryBuilder) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    write(&mut w, query);
    w.finish()
}

/// Writes a full SELECT; also used for subqueries, so bindings stay in order.
pub(super) fn write(w: &mut Writer, query: &QueryBuilder) {
    if w.dialect == Dialect::Oracle
        && let Some(lock) = query.lock
        && (query.limit.is_some() || query.offset.is_some())
        && oracle::can_lock_window(query, lock)
    {
        return locked_window(w, query, lock);
    }
    check_query(w, query);
    let union = !query.unions.is_empty();
    // Oracle cannot order a union of `SELECT *` by column name (ORA-00904),
    // but can order the union read as an inline view.
    let view = union && w.dialect == Dialect::Oracle && !query.orders.is_empty();
    if view {
        w.push("SELECT * FROM (");
    }
    if union {
        // Parenthesized, so the ordering and limits below apply to every part.
        w.push("(");
    }
    w.push(if query.distinct {
        "SELECT DISTINCT "
    } else {
        "SELECT "
    });
    if query.columns.is_empty() {
        w.push("*");
    } else {
        w.list(&query.columns, expr);
    }
    w.push(" FROM ");
    source(w, &query.table, query.from_sub.as_deref());
    for join in &query.joins {
        self::join(w, join);
    }
    conditions::clause(w, " WHERE ", &query.wheres);
    if !query.groups.is_empty() {
        w.push(" GROUP BY ");
        w.list(&query.groups, expr);
    }
    conditions::clause(w, " HAVING ", &query.havings);
    if union {
        w.push(")");
        for part in &query.unions {
            w.push(if part.all { " UNION ALL (" } else { " UNION (" });
            write(w, &part.query);
            w.push(")");
        }
        if query.lock.is_some() {
            w.fail(Error::InvalidQuery("a union cannot lock rows".to_owned()));
        }
    }
    if view {
        w.push(")");
    }
    if !query.orders.is_empty() {
        w.push(" ORDER BY ");
        w.list(&query.orders, order);
    }

    match w.dialect {
        Dialect::MySql => {
            mysql::limit_offset(w, query.limit, query.offset);
            if let Some(lock) = query.lock {
                mysql::lock(w, lock);
            }
        }
        Dialect::Oracle => {
            oracle::limit_offset(w, query.limit, query.offset);
            if let Some(lock) = query.lock {
                oracle::lock(w, lock, query.limit.is_some() || query.offset.is_some());
            }
        }
    }
}

/// Oracle rejects `FOR UPDATE` after a row limit (ORA-02014), so the limited
/// query only picks the rows and the outer one locks them:
/// `SELECT ... FROM t WHERE ROWID IN (SELECT ROWID FROM t WHERE ... FETCH NEXT 1 ROWS ONLY) ORDER BY ... FOR UPDATE`.
fn locked_window(w: &mut Writer, query: &QueryBuilder, lock: Lock) {
    check_query(w, query);
    w.push("SELECT ");
    if query.columns.is_empty() {
        w.push("*");
    } else {
        w.list(&query.columns, expr);
    }
    w.push(" FROM ");
    w.table(&query.table);
    w.push(" WHERE ROWID IN (");
    let mut window = query.clone();
    window.lock = None;
    window.columns = vec![Expr::Raw(Raw::new("ROWID", []))];
    write(w, &window);
    w.push(")");
    if !query.orders.is_empty() {
        w.push(" ORDER BY ");
        w.list(&query.orders, order);
    }
    oracle::lock(w, lock, false);
}

/// `SELECT FUNCTION(column) AS aggregate` over the query's rows.
///
/// DISTINCT, GROUP BY, LIMIT and OFFSET change which rows are counted, so such
/// queries are wrapped: `SELECT COUNT(*) AS aggregate FROM (<query>) aggregate_table`.
pub(crate) fn aggregate(
    dialect: Dialect,
    query: &QueryBuilder,
    function: Aggregate,
    column: Option<&str>,
) -> Result<Compiled> {
    let mut w = Writer::new(dialect);
    let mut query = query.clone();
    query.lock = None; // Oracle rejects FOR UPDATE with aggregates; a count locks nothing useful
    let aggregate = Expr::Aggregate {
        function,
        column: column.map(str::to_owned),
        alias: Some("aggregate"),
    };

    let wrap = query.distinct
        || !query.unions.is_empty()
        || !query.groups.is_empty()
        || query.limit.is_some()
        || query.offset.is_some();
    if wrap {
        if query.columns.is_empty()
            && column.is_none()
            && !query.distinct
            && query.unions.is_empty()
        {
            // `SELECT *` over a join repeats column names, which MySQL rejects
            // in a derived table; counting rows needs no columns.
            query.columns = vec![Expr::Raw(Raw::new("1", []))];
        }
        w.push("SELECT ");
        expr(&mut w, &aggregate);
        w.push(" FROM (");
        write(&mut w, &query);
        w.push(") ");
        w.column("aggregate_table");
    } else {
        query.columns = vec![aggregate];
        query.orders.clear();
        write(&mut w, &query);
    }
    w.finish()
}

fn expr(w: &mut Writer, expr: &Expr) {
    match expr {
        Expr::Column(column) => w.column(column),
        Expr::Raw(raw) => {
            if w.raw(raw) {
                w.push("\n"); // the rest of the statement after a trailing `--` comment
            }
        }
        Expr::Sub { query, alias } => {
            w.push("(");
            write(w, query);
            w.push(") AS ");
            w.column(alias);
        }
        Expr::Exists { query, alias } => {
            w.push("CASE WHEN EXISTS (");
            write(w, query);
            w.push(") THEN 1 ELSE 0 END AS ");
            w.column(alias);
        }
        Expr::RowNumber {
            partition,
            order: orders,
            alias,
        } => {
            w.push("ROW_NUMBER() OVER (PARTITION BY ");
            w.list(partition, |w, column| w.column(column));
            w.push(" ORDER BY ");
            w.list(orders, order);
            w.push(") AS ");
            w.column(alias);
        }
        Expr::Aggregate {
            function,
            column,
            alias,
        } => {
            w.push(function.as_sql());
            w.push("(");
            match column {
                Some(column) => w.column(column),
                None => w.push("*"),
            }
            w.push(")");
            if let Some(alias) = alias {
                w.push(" AS ");
                w.column(alias);
            }
        }
    }
}

fn join(w: &mut Writer, join: &Join) {
    w.push(match join.kind {
        JoinKind::Inner => " INNER JOIN ",
        JoinKind::Left => " LEFT JOIN ",
        JoinKind::Right => " RIGHT JOIN ",
        JoinKind::Cross => " CROSS JOIN ",
    });
    source(w, &join.table, join.sub.as_deref());
    if join.kind == JoinKind::Cross {
        return;
    }
    if join.on.is_empty() {
        w.fail(Error::InvalidQuery(format!(
            "join on `{}` has no ON condition",
            join.table
        )));
    }
    conditions::clause(w, " ON ", &join.on);
}

/// `table [AS alias]`, or `(SELECT ...) alias` for a subquery.
fn source(w: &mut Writer, table: &str, sub: Option<&QueryBuilder>) {
    match sub {
        Some(query) => {
            w.push("(");
            write(w, query);
            w.push(") ");
            w.column(table);
        }
        None => w.table(table),
    }
}

fn order(w: &mut Writer, order: &Order) {
    match order {
        Order::Column { column, descending } => {
            w.column(column);
            w.push(if *descending { " DESC" } else { " ASC" });
        }
        Order::Raw(raw) => {
            if w.raw(raw) {
                w.push("\n"); // the rest of the statement after a trailing `--` comment
            }
        }
    }
}
