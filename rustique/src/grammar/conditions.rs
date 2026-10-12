//! WHERE / HAVING / ON condition lists, shared by every statement.

use super::writer::Writer;
use super::{oracle, select};
use crate::query::{Clause, Condition, Conjunction};
use crate::{Dialect, Value};

/// Writes `keyword` followed by the clauses; nothing when there are none.
pub(super) fn clause(w: &mut Writer, keyword: &str, clauses: &[Clause]) {
    if !clauses.is_empty() {
        w.push(keyword);
        list(w, clauses);
    }
}

pub(super) fn list(w: &mut Writer, clauses: &[Clause]) {
    for (i, clause) in clauses.iter().enumerate() {
        if i > 0 {
            w.push(match clause.conjunction {
                Conjunction::And => " AND ",
                Conjunction::Or => " OR ",
            });
        }
        condition(w, &clause.condition, clauses.len() > 1);
    }
}

/// `grouped`: other clauses share the list, so raw SQL is parenthesized
/// and an `OR` inside it cannot escape them (`a OR b AND deleted_at IS NULL`).
fn condition(w: &mut Writer, condition: &Condition, grouped: bool) {
    match condition {
        Condition::Basic { column, op, value } => {
            w.column(column);
            w.push(" ");
            w.push(op.as_sql());
            w.push(" ");
            w.bind(value.clone());
        }
        Condition::Column { first, op, second } => {
            w.column(first);
            w.push(" ");
            w.push(op.as_sql());
            w.push(" ");
            w.column(second);
        }
        Condition::In {
            column,
            values,
            negated,
        } => {
            let inline = inline_integers(values.len(), values.iter());
            if inline && w.dialect == Dialect::Oracle {
                w.column(column);
                return oracle_json_list(w, 1, values.iter().map(std::slice::from_ref), *negated);
            }
            in_list(
                w,
                values,
                *negated,
                |w| w.column(column),
                |w, value| value_or_literal(w, value, inline),
            );
        }
        Condition::InRows { columns, rows } => {
            let inline = inline_integers(rows.len(), rows.iter().flatten());
            if inline && w.dialect == Dialect::Oracle {
                w.push("(");
                w.list(columns, |w, column| w.column(column));
                w.push(")");
                return oracle_json_list(w, columns.len(), rows.iter().map(Vec::as_slice), false);
            }
            in_list(
                w,
                rows,
                false,
                |w| {
                    w.push("(");
                    w.list(columns, |w, column| w.column(column));
                    w.push(")");
                },
                |w, row| {
                    w.push("(");
                    w.list(row, |w, value| value_or_literal(w, value, inline));
                    w.push(")");
                },
            );
        }
        Condition::InSub {
            column,
            query,
            negated,
        } => {
            w.column(column);
            w.push(if *negated { " NOT IN (" } else { " IN (" });
            select::write(w, query);
            w.push(")");
        }
        Condition::Null { column, negated } => {
            w.column(column);
            w.push(if *negated { " IS NOT NULL" } else { " IS NULL" });
        }
        Condition::Between {
            column,
            low,
            high,
            negated,
        } => {
            w.column(column);
            w.push(if *negated {
                " NOT BETWEEN "
            } else {
                " BETWEEN "
            });
            w.bind(low.clone());
            w.push(" AND ");
            w.bind(high.clone());
        }
        Condition::Exists { query, negated } => {
            w.push(if *negated { "NOT EXISTS (" } else { "EXISTS (" });
            select::write(w, query);
            w.push(")");
        }
        Condition::Json {
            column,
            path,
            op,
            value,
        } => {
            match w.dialect {
                Dialect::MySql => {
                    w.push("JSON_UNQUOTE(JSON_EXTRACT(");
                    w.column(column);
                    w.push(&format!(", '{path}'))"));
                }
                Dialect::Oracle => {
                    w.push("JSON_VALUE(");
                    w.column(column);
                    w.push(&format!(", '{path}'"));
                    if matches!(
                        value,
                        Value::I64(_) | Value::U64(_) | Value::F64(_) | Value::Decimal(_)
                    ) {
                        w.push(" RETURNING NUMBER");
                    }
                    w.push(")");
                }
            }
            w.push(" ");
            w.push(op.as_sql());
            w.push(" ");
            w.bind(value.clone());
        }
        Condition::Group(clauses) => {
            w.push("(");
            list(w, clauses);
            w.push(")");
        }
        Condition::Raw(raw) if grouped => {
            w.push("(");
            if w.raw(raw) {
                w.push("\n");
            }
            w.push(")");
        }
        Condition::Raw(raw) => {
            if w.raw(raw) {
                w.push("\n"); // the rest of the statement after a trailing `--` comment
            }
        }
    }
}

/// `IN` lists longer than this skip per-item bindings when every value is
/// an integer, since a statement holds at most 65,535 bindings on both
/// databases. MySQL writes them as literals, like Laravel's
/// `whereIntegerInRaw`: a list this long gets a statement of its own anyway,
/// so binding it gains no reuse. Oracle also stops at 65,535 list items
/// written as literals (ORA-00913), so it binds the whole list as one JSON
/// array; see [`oracle_json_list`]. Shorter lists stay bound.
const INLINE_IN_LIST: usize = 1000;

/// ` [NOT] IN (SELECT c0, ... FROM JSON_TABLE(:n, '$[*]' COLUMNS (c0 NUMBER PATH '$[0]', ...)))`
/// with `rows` of `width` integers as one JSON array bind, which Oracle
/// sends as a CLOB once it outgrows a `VARCHAR2`.
fn oracle_json_list<'a>(
    w: &mut Writer,
    width: usize,
    rows: impl Iterator<Item = &'a [Value]>,
    negated: bool,
) {
    let mut json = String::from("[");
    for (i, row) in rows.enumerate() {
        if i > 0 {
            json.push(',');
        }
        json.push('[');
        for (j, value) in row.iter().enumerate() {
            if j > 0 {
                json.push(',');
            }
            json.push_str(&match value {
                Value::I64(n) => n.to_string(),
                Value::U64(n) => n.to_string(),
                _ => unreachable!("checked by `inline_integers`"),
            });
        }
        json.push(']');
    }
    json.push(']');
    let columns = (0..width).map(|i| format!("c{i}")).collect::<Vec<_>>();
    let paths = (columns.iter().enumerate())
        .map(|(i, c)| format!("{c} NUMBER PATH '$[{i}]'"))
        .collect::<Vec<_>>();
    w.push(if negated {
        " NOT IN (SELECT "
    } else {
        " IN (SELECT "
    });
    w.push(&columns.join(", "));
    w.push(" FROM JSON_TABLE(");
    w.bind(Value::String(json));
    w.push(", '$[*]' COLUMNS (");
    w.push(&paths.join(", "));
    w.push(")))");
}

/// Whether an `IN` list of `len` items with `values` is written as literals.
fn inline_integers<'a>(len: usize, mut values: impl Iterator<Item = &'a Value>) -> bool {
    len > INLINE_IN_LIST && values.all(|v| matches!(v, Value::I64(_) | Value::U64(_)))
}

/// A placeholder, or the integer itself when `inline`.
fn value_or_literal(w: &mut Writer, value: &Value, inline: bool) {
    match value {
        Value::I64(n) if inline => w.push(&n.to_string()),
        Value::U64(n) if inline => w.push_u64(*n),
        _ => w.bind(value.clone()),
    }
}

/// `lhs [NOT] IN (...)`. Lists longer than Oracle's limit are split into
/// `(c IN (...) OR c IN (...))`, or `(c NOT IN (...) AND c NOT IN (...))`.
fn in_list<T>(
    w: &mut Writer,
    values: &[T],
    negated: bool,
    lhs: impl Fn(&mut Writer),
    item: impl Fn(&mut Writer, &T),
) {
    if values.is_empty() {
        // `IN ()` is a syntax error; an empty list matches nothing, or everything when negated.
        w.push(if negated { "1 = 1" } else { "0 = 1" });
        return;
    }
    let max = match w.dialect {
        Dialect::MySql => usize::MAX,
        Dialect::Oracle => oracle::MAX_IN_LIST,
    };
    let split = values.len() > max;
    if split {
        w.push("(");
    }
    for (i, chunk) in values.chunks(max).enumerate() {
        if i > 0 {
            w.push(if negated { " AND " } else { " OR " });
        }
        lhs(w);
        w.push(if negated { " NOT IN (" } else { " IN (" });
        w.list(chunk, &item);
        w.push(")");
    }
    if split {
        w.push(")");
    }
}
