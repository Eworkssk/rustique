//! WHERE methods of [`QueryBuilder`], plus the condition constructors that
//! HAVING and [`JoinClause`](super::JoinClause) share.

use chrono::{NaiveDate, NaiveTime};

use super::{Clause, Condition, Conjunction, Operator, QueryBuilder, Raw};
use crate::Value;

/// `column op value`. `= NULL` and `<> NULL` become `IS [NOT] NULL`, as in Laravel.
pub(super) fn basic(column: String, op: &str, value: Value) -> Result<Condition, String> {
    let parsed = Operator::parse(op).ok_or_else(|| format!("unknown operator `{op}`"))?;
    match (parsed, value.is_null()) {
        (Operator::Eq | Operator::Is, true) => Ok(Condition::Null {
            column,
            negated: false,
        }),
        (Operator::NotEq | Operator::IsNot, true) => Ok(Condition::Null {
            column,
            negated: true,
        }),
        (Operator::Is | Operator::IsNot, false) => {
            Err(format!("operator `{op}` only accepts NULL"))
        }
        (_, true) => Err(format!("operator `{op}` cannot compare with NULL")),
        (op, false) => Ok(Condition::Basic { column, op, value }),
    }
}

/// `first op second`, comparing two columns.
pub(super) fn columns(first: String, op: &str, second: String) -> Result<Condition, String> {
    match Operator::parse(op) {
        None | Some(Operator::Is | Operator::IsNot) => {
            Err(format!("operator `{op}` cannot compare columns"))
        }
        Some(op) => Ok(Condition::Column { first, op, second }),
    }
}

/// `JSON_VALUE(column, '$."a"[0]') op value` from `column->a->0`.
fn json(path: &str, op: &str, value: Value) -> Result<Condition, String> {
    let mut parts = path.split("->");
    let column = parts.next().unwrap_or_default().trim().to_owned();
    let mut json_path = String::from("$");
    for part in parts {
        let part = part.trim();
        let plain = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        if !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()) {
            json_path.push_str(&format!("[{part}]"));
        } else if !part.is_empty() && part.chars().all(plain) {
            // Only these characters reach the SQL text, inside a quoted literal.
            json_path.push_str(&format!(".\"{part}\""));
        } else {
            return Err(format!(
                "JSON path segment `{part}` in `{path}` may only use letters, digits, `_` and `-`"
            ));
        }
    }
    if json_path == "$" {
        return Err(format!("`{path}` has no JSON path; write `column->key`"));
    }
    let op = match Operator::parse(op) {
        None | Some(Operator::Is | Operator::IsNot) => {
            return Err(format!("operator `{op}` cannot compare JSON values"));
        }
        Some(op) => op,
    };
    let value = match value {
        Value::Null => return Err("where_json cannot compare with NULL".to_owned()),
        // Both databases read JSON booleans as the text `true` / `false`.
        Value::Bool(b) => Value::String(b.to_string()),
        value => value,
    };
    Ok(Condition::Json {
        column,
        path: json_path,
        op,
        value,
    })
}

/// `column op date`, comparing the date part of a date-time column as a
/// range, so an index on `column` still applies.
fn date(column: String, op: &str, date: NaiveDate) -> Result<Condition, String> {
    let start = Value::DateTime(date.and_time(NaiveTime::MIN));
    let next = date
        .succ_opt()
        .map(|d| Value::DateTime(d.and_time(NaiveTime::MIN)))
        .ok_or_else(|| format!("where_date: no day follows {date}"))?;
    let compare = |op: &str, value: Value, conjunction| Clause {
        conjunction,
        condition: Condition::Basic {
            column: column.clone(),
            op: Operator::parse(op).expect("valid operator"),
            value,
        },
    };
    let (and, or) = (Conjunction::And, Conjunction::Or);
    Ok(match Operator::parse(op) {
        Some(Operator::Eq) => {
            Condition::Group(vec![compare(">=", start, and), compare("<", next, and)])
        }
        Some(Operator::NotEq) => {
            Condition::Group(vec![compare("<", start, and), compare(">=", next, or)])
        }
        Some(Operator::Lt) => compare("<", start, and).condition,
        Some(Operator::Le) => compare("<", next, and).condition,
        Some(Operator::Gt) => compare(">=", next, and).condition,
        Some(Operator::Ge) => compare(">=", start, and).condition,
        _ => return Err(format!("operator `{op}` cannot compare dates")),
    })
}

/// Appends a condition, or records why it could not be built.
pub(super) fn push(
    clauses: &mut Vec<Clause>,
    error: &mut Option<String>,
    conjunction: Conjunction,
    condition: Result<Condition, String>,
) {
    match condition {
        Ok(condition) => clauses.push(Clause {
            conjunction,
            condition,
        }),
        Err(message) => {
            error.get_or_insert(message);
        }
    }
}

impl QueryBuilder {
    fn push_where(
        mut self,
        conjunction: Conjunction,
        condition: Result<Condition, String>,
    ) -> Self {
        push(&mut self.wheres, &mut self.error, conjunction, condition);
        self
    }

    /// `WHERE column op value`. Operators: `=`, `<>`/`!=`, `<`, `<=`, `>`, `>=`,
    /// `like`, `not like`, and `is` / `is not` with `NULL`.
    pub fn where_(self, column: impl Into<String>, op: &str, value: impl Into<Value>) -> Self {
        self.push_where(Conjunction::And, basic(column.into(), op, value.into()))
    }

    /// `OR column op value`.
    pub fn or_where(self, column: impl Into<String>, op: &str, value: impl Into<Value>) -> Self {
        self.push_where(Conjunction::Or, basic(column.into(), op, value.into()))
    }

    /// `WHERE first op second`, comparing two columns.
    pub fn where_column(
        self,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.push_where(Conjunction::And, columns(first.into(), op, second.into()))
    }

    /// `OR first op second`.
    pub fn or_where_column(
        self,
        first: impl Into<String>,
        op: &str,
        second: impl Into<String>,
    ) -> Self {
        self.push_where(Conjunction::Or, columns(first.into(), op, second.into()))
    }

    fn in_list(
        self,
        conjunction: Conjunction,
        column: String,
        values: impl IntoIterator<Item: Into<Value>>,
        negated: bool,
    ) -> Self {
        let values = values.into_iter().map(Into::into).collect();
        self.push_where(
            conjunction,
            Ok(Condition::In {
                column,
                values,
                negated,
            }),
        )
    }

    /// `WHERE column IN (...)`. An empty list matches no rows.
    pub fn where_in(
        self,
        column: impl Into<String>,
        values: impl IntoIterator<Item: Into<Value>>,
    ) -> Self {
        self.in_list(Conjunction::And, column.into(), values, false)
    }

    /// `OR column IN (...)`.
    pub fn or_where_in(
        self,
        column: impl Into<String>,
        values: impl IntoIterator<Item: Into<Value>>,
    ) -> Self {
        self.in_list(Conjunction::Or, column.into(), values, false)
    }

    /// `WHERE column NOT IN (...)`. An empty list matches every row.
    pub fn where_not_in(
        self,
        column: impl Into<String>,
        values: impl IntoIterator<Item: Into<Value>>,
    ) -> Self {
        self.in_list(Conjunction::And, column.into(), values, true)
    }

    /// `OR column NOT IN (...)`.
    pub fn or_where_not_in(
        self,
        column: impl Into<String>,
        values: impl IntoIterator<Item: Into<Value>>,
    ) -> Self {
        self.in_list(Conjunction::Or, column.into(), values, true)
    }

    /// `WHERE (a, b) IN ((?, ?), ...)`: matches rows whose `columns` equal one
    /// of `rows`, for composite keys. An empty list matches no rows.
    pub fn where_rows_in(
        self,
        columns: impl IntoIterator<Item: Into<String>>,
        rows: impl IntoIterator<Item = Vec<Value>>,
    ) -> Self {
        let columns: Vec<String> = columns.into_iter().map(Into::into).collect();
        let rows: Vec<Vec<Value>> = rows.into_iter().collect();
        let condition = match rows.iter().find(|row| row.len() != columns.len()) {
            Some(row) => Err(format!(
                "where_rows_in row has {} values for {} columns",
                row.len(),
                columns.len()
            )),
            None => Ok(Condition::InRows { columns, rows }),
        };
        self.push_where(Conjunction::And, condition)
    }

    /// `WHERE column IN (subquery)`.
    pub fn where_in_sub(self, column: impl Into<String>, query: Self) -> Self {
        let condition = Condition::InSub {
            column: column.into(),
            query: Box::new(query),
            negated: false,
        };
        self.push_where(Conjunction::And, Ok(condition))
    }

    /// `WHERE column NOT IN (subquery)`.
    pub fn where_not_in_sub(self, column: impl Into<String>, query: Self) -> Self {
        let condition = Condition::InSub {
            column: column.into(),
            query: Box::new(query),
            negated: true,
        };
        self.push_where(Conjunction::And, Ok(condition))
    }

    /// `WHERE column IS NULL`.
    pub fn where_null(self, column: impl Into<String>) -> Self {
        self.push_where(
            Conjunction::And,
            Ok(Condition::Null {
                column: column.into(),
                negated: false,
            }),
        )
    }

    /// `OR column IS NULL`.
    pub fn or_where_null(self, column: impl Into<String>) -> Self {
        self.push_where(
            Conjunction::Or,
            Ok(Condition::Null {
                column: column.into(),
                negated: false,
            }),
        )
    }

    /// `WHERE column IS NOT NULL`.
    pub fn where_not_null(self, column: impl Into<String>) -> Self {
        self.push_where(
            Conjunction::And,
            Ok(Condition::Null {
                column: column.into(),
                negated: true,
            }),
        )
    }

    /// `OR column IS NOT NULL`.
    pub fn or_where_not_null(self, column: impl Into<String>) -> Self {
        self.push_where(
            Conjunction::Or,
            Ok(Condition::Null {
                column: column.into(),
                negated: true,
            }),
        )
    }

    /// `WHERE column BETWEEN low AND high`.
    pub fn where_between(
        self,
        column: impl Into<String>,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        let condition = Condition::Between {
            column: column.into(),
            low: low.into(),
            high: high.into(),
            negated: false,
        };
        self.push_where(Conjunction::And, Ok(condition))
    }

    /// `WHERE column NOT BETWEEN low AND high`.
    pub fn where_not_between(
        self,
        column: impl Into<String>,
        low: impl Into<Value>,
        high: impl Into<Value>,
    ) -> Self {
        let condition = Condition::Between {
            column: column.into(),
            low: low.into(),
            high: high.into(),
            negated: true,
        };
        self.push_where(Conjunction::And, Ok(condition))
    }

    /// `WHERE` the date part of `column` compares to `date`, like Laravel's
    /// `whereDate`. Compiled as a range on `column` (`= d` becomes
    /// `column >= d AND column < d + 1 day`), so indexes still apply.
    /// Operators: `=`, `<>`, `<`, `<=`, `>`, `>=`.
    pub fn where_date(self, column: impl Into<String>, op: &str, value: NaiveDate) -> Self {
        self.push_where(Conjunction::And, date(column.into(), op, value))
    }

    /// `WHERE` a value inside a JSON column compares to `value`, like
    /// Laravel's `where('settings->theme', ...)`: `"settings->theme"` reads
    /// key `theme`, `"tags->0"` the first array item. Path segments may use
    /// letters, digits, `_` and `-`.
    ///
    /// MySQL compiles `JSON_UNQUOTE(JSON_EXTRACT(column, path))`, Oracle
    /// `JSON_VALUE(column, path)` (`RETURNING NUMBER` for numeric values).
    /// Booleans compare as `"true"` / `"false"`; missing keys never match.
    pub fn where_json(self, path: &str, op: &str, value: impl Into<Value>) -> Self {
        self.push_where(Conjunction::And, json(path, op, value.into()))
    }

    /// `OR` a value inside a JSON column compares to `value`; see [`where_json`](Self::where_json).
    pub fn or_where_json(self, path: &str, op: &str, value: impl Into<Value>) -> Self {
        self.push_where(Conjunction::Or, json(path, op, value.into()))
    }

    /// `WHERE EXISTS (subquery)`.
    pub fn where_exists(self, query: Self) -> Self {
        self.push_where(
            Conjunction::And,
            Ok(Condition::Exists {
                query: Box::new(query),
                negated: false,
            }),
        )
    }

    /// `WHERE NOT EXISTS (subquery)`.
    pub fn where_not_exists(self, query: Self) -> Self {
        self.push_where(
            Conjunction::And,
            Ok(Condition::Exists {
                query: Box::new(query),
                negated: true,
            }),
        )
    }

    fn group(mut self, conjunction: Conjunction, build: impl FnOnce(Self) -> Self) -> Self {
        let nested = build(Self::default());
        if let Some(message) = nested.error {
            self.fail(message);
        }
        if nested.wheres.is_empty() {
            return self;
        }
        self.push_where(conjunction, Ok(Condition::Group(nested.wheres)))
    }

    /// Wraps the WHERE clauses in parentheses when they contain an `OR`, so a
    /// condition appended afterwards applies to all of them.
    pub(crate) fn group_or_wheres(&mut self) {
        if self
            .wheres
            .iter()
            .skip(1)
            .any(|c| c.conjunction == Conjunction::Or)
        {
            let wheres = std::mem::take(&mut self.wheres);
            self.wheres.push(Clause {
                conjunction: Conjunction::And,
                condition: Condition::Group(wheres),
            });
        }
    }

    /// `WHERE ( ... )`: conditions added inside `build` are wrapped in parentheses,
    /// like Laravel's `where(function ($q) { ... })`.
    pub fn where_group(self, build: impl FnOnce(Self) -> Self) -> Self {
        self.group(Conjunction::And, build)
    }

    /// `OR ( ... )`.
    pub fn or_where_group(self, build: impl FnOnce(Self) -> Self) -> Self {
        self.group(Conjunction::Or, build)
    }

    /// Raw `WHERE` SQL; each `?` outside quotes takes the next value of `binds`.
    pub fn where_raw(self, sql: impl Into<String>, binds: impl IntoIterator<Item = Value>) -> Self {
        self.push_where(Conjunction::And, Ok(Condition::Raw(Raw::new(sql, binds))))
    }

    /// Raw `OR` SQL.
    pub fn or_where_raw(
        self,
        sql: impl Into<String>,
        binds: impl IntoIterator<Item = Value>,
    ) -> Self {
        self.push_where(Conjunction::Or, Ok(Condition::Raw(Raw::new(sql, binds))))
    }
}
