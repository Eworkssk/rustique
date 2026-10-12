//! Cursor (keyset) pagination: [`CursorPage`] and the opaque cursor strings.

use std::str::FromStr;

use serde_json::{Value as Json, json};

use super::execute::output_name;
use super::{Order, QueryBuilder};
use crate::db::Executor;
use crate::{Collection, Error, Result, Row, Value};

/// One page of results from `cursor_paginate`: no total, but each page is
/// one indexed range query however deep it is, unlike `OFFSET` paging.
#[derive(Debug, Clone, PartialEq)]
pub struct CursorPage<T> {
    /// Rows or models on this page.
    pub items: Collection<T>,
    /// Page size.
    pub per_page: u64,
    /// Cursor of the following page; `None` on the last page.
    pub next_cursor: Option<String>,
    /// Cursor of the preceding page; `None` on the first page.
    pub prev_cursor: Option<String>,
}

impl<T> CursorPage<T> {
    /// Whether pages follow this one.
    #[must_use]
    pub fn has_more_pages(&self) -> bool {
        self.next_cursor.is_some()
    }
}

/// Where a cursor points: past the row it holds (`next`), or before it.
#[derive(Debug, Clone, PartialEq)]
struct Cursor {
    values: Vec<Value>,
    next: bool,
}

impl Cursor {
    fn encode(&self) -> String {
        let values: Vec<Json> = self.values.iter().map(to_json).collect();
        let text = json!({ "n": self.next, "v": values }).to_string();
        text.bytes().map(|b| format!("{b:02x}")).collect()
    }

    fn decode(text: &str) -> Option<Self> {
        let bytes = (0..text.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
            .collect::<Option<Vec<u8>>>()?;
        let json: Json = serde_json::from_slice(&bytes).ok()?;
        let values = json.get("v")?.as_array()?;
        Some(Self {
            values: values.iter().map(from_json).collect::<Option<_>>()?,
            next: json.get("n")?.as_bool()?,
        })
    }
}

/// `[tag, payload]`, so each value decodes back into its own variant.
fn to_json(value: &Value) -> Json {
    match value {
        Value::Null => json!(["null"]),
        Value::Bool(b) => json!(["b", b]),
        Value::I64(n) => json!(["i", n]),
        Value::U64(n) => json!(["u", n]),
        Value::F64(n) => json!(["f", n]),
        Value::Decimal(d) => json!(["d", d.to_string()]),
        Value::String(s) => json!(["s", s]),
        Value::Bytes(b) => json!(["x", b]),
        Value::Date(d) => json!(["date", d.to_string()]),
        // `Display` puts a space before the time, which `FromStr` rejects.
        Value::DateTime(dt) => json!(["dt", dt.format("%Y-%m-%dT%H:%M:%S%.f").to_string()]),
        Value::Time(t) => json!(["t", t.to_string()]),
        Value::Json(j) => json!(["j", j]),
    }
}

fn from_json(json: &Json) -> Option<Value> {
    let parts = json.as_array()?;
    let tag = parts.first()?.as_str()?;
    let payload = parts.get(1);
    let text = || payload.and_then(Json::as_str);
    Some(match tag {
        "null" => Value::Null,
        "b" => Value::Bool(payload?.as_bool()?),
        "i" => Value::I64(payload?.as_i64()?),
        "u" => Value::U64(payload?.as_u64()?),
        "f" => Value::F64(payload?.as_f64()?),
        "d" => Value::Decimal(text()?.parse().ok()?),
        "s" => Value::String(text()?.to_owned()),
        "x" => Value::Bytes(serde_json::from_value(payload?.clone()).ok()?),
        "date" => Value::Date(FromStr::from_str(text()?).ok()?),
        "dt" => Value::DateTime(FromStr::from_str(text()?).ok()?),
        "t" => Value::Time(FromStr::from_str(text()?).ok()?),
        "j" => Value::Json(payload?.clone()),
        _ => return None,
    })
}

impl QueryBuilder {
    /// Page of `per_page` rows after (or before) `cursor`, like Laravel's
    /// `cursorPaginate`. Pass `None` for the first page, then a
    /// [`CursorPage::next_cursor`] or [`CursorPage::prev_cursor`].
    ///
    /// The query's [`order_by`](Self::order_by) columns define the order;
    /// together they must be unique (end with the key) and never NULL, and
    /// they must be selected. Each page filters on them, as in
    /// `WHERE (a > ? OR (a = ? AND id > ?))`, so deep pages stay fast with
    /// an index on them.
    ///
    /// ```no_run
    /// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
    /// let query = rustique::QueryBuilder::table("events").order_by_desc("created_at").order_by_desc("id");
    /// let first = query.cursor_paginate(db, 50, None).await?;
    /// if let Some(next) = &first.next_cursor {
    ///     let second = query.cursor_paginate(db, 50, Some(next)).await?;
    /// }
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get), plus [`Error::InvalidQuery`] for a query
    /// without column ordering, a cursor that does not decode or does not
    /// match the ordering, and [`Error::ColumnNotFound`] when an ordering
    /// column is not selected.
    pub async fn cursor_paginate(
        &self,
        db: &impl Executor,
        per_page: u64,
        cursor: Option<&str>,
    ) -> Result<CursorPage<Row>> {
        let (rows, next_cursor, prev_cursor) = self.cursor_rows(db, per_page, cursor).await?;
        Ok(CursorPage {
            items: rows.into(),
            per_page: per_page.max(1),
            next_cursor,
            prev_cursor,
        })
    }

    /// The rows of a cursor page, and the next and previous cursors.
    pub(crate) async fn cursor_rows(
        &self,
        db: &impl Executor,
        per_page: u64,
        cursor: Option<&str>,
    ) -> Result<(Vec<Row>, Option<String>, Option<String>)> {
        let per_page = per_page.max(1);
        let orders = self.cursor_orders()?;
        let cursor = cursor
            .map(|text| {
                Cursor::decode(text)
                    .filter(|c| c.values.len() == orders.len())
                    .ok_or_else(|| Error::InvalidQuery("invalid pagination cursor".to_owned()))
            })
            .transpose()?;
        let forward = cursor.as_ref().is_none_or(|c| c.next);

        let mut query = self.clone();
        query.group_or_wheres();
        query.offset = None;
        query.limit = Some(per_page.saturating_add(1));
        if !forward {
            // Walk backwards from the cursor, then put the rows back in order.
            for order in &mut query.orders {
                if let Order::Column { descending, .. } = order {
                    *descending = !*descending;
                }
            }
        }
        if let Some(cursor) = &cursor {
            query = past(query, &orders, &cursor.values, forward);
        }
        let mut rows = query.get(db).await?.into_inner();
        let more = rows.len() as u64 > per_page;
        rows.truncate(per_page as usize);
        if !forward {
            rows.reverse();
        }

        let values = |row: &Row| -> Result<Vec<Value>> {
            orders
                .iter()
                .map(|(column, _)| row.get(output_name(column)))
                .collect()
        };
        let encode = |row: Option<&Row>, next: bool| -> Result<Option<String>> {
            row.map(|row| {
                Ok(Cursor {
                    values: values(row)?,
                    next,
                }
                .encode())
            })
            .transpose()
        };
        // Going forward there are rows before unless this is the first page;
        // going back there are rows after: the page the cursor came from.
        let (has_next, has_prev) = if forward {
            (more, cursor.is_some())
        } else {
            (true, more)
        };
        let next = if has_next {
            encode(rows.last(), true)?
        } else {
            None
        };
        let prev = if has_prev {
            encode(rows.first(), false)?
        } else {
            None
        };
        Ok((rows, next, prev))
    }

    /// The ordering as `(column, descending)`; raw orderings cannot be paged on.
    fn cursor_orders(&self) -> Result<Vec<(String, bool)>> {
        if self.orders.is_empty() {
            return Err(Error::InvalidQuery(
                "cursor pagination needs `order_by` on unique columns".to_owned(),
            ));
        }
        self.orders
            .iter()
            .map(|order| match order {
                Order::Column { column, descending } => Ok((column.clone(), *descending)),
                Order::Raw(_) => Err(Error::InvalidQuery(
                    "cursor pagination cannot use `order_by_raw`".to_owned(),
                )),
            })
            .collect()
    }
}

/// Rows past `values` in the order `orders`: `a > ? OR (a = ? AND b > ?)`,
/// with `<` for descending columns, flipped when walking backwards.
fn past(
    query: QueryBuilder,
    orders: &[(String, bool)],
    values: &[Value],
    forward: bool,
) -> QueryBuilder {
    let pairs = || orders.iter().zip(values.iter().cloned());
    query.where_group(|group| {
        (0..orders.len()).fold(group, |group, i| {
            group.or_where_group(|q| {
                let q = pairs().take(i).fold(q, |q, ((column, _), value)| {
                    q.where_(column.clone(), "=", value)
                });
                let ((column, descending), value) = pairs().nth(i).expect("i < len");
                let op = if *descending == forward { "<" } else { ">" };
                q.where_(column.clone(), op, value)
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Dialect;
    use chrono::NaiveDate;

    #[test]
    fn cursors_round_trip_every_value() {
        let day = NaiveDate::from_ymd_opt(2026, 10, 11).unwrap();
        let cursor = Cursor {
            values: vec![
                Value::Null,
                Value::I64(-3),
                Value::U64(u64::MAX),
                Value::F64(1.5),
                Value::Decimal("12.30".parse().unwrap()),
                Value::String("a\"b".into()),
                Value::Date(day),
                Value::DateTime(day.and_hms_micro_opt(1, 2, 3, 4).unwrap()),
                Value::Bool(true),
            ],
            next: false,
        };
        assert_eq!(Cursor::decode(&cursor.encode()), Some(cursor));
        assert_eq!(Cursor::decode("zz"), None);
        assert_eq!(Cursor::decode("7b7d"), None); // `{}`
    }

    #[test]
    fn pages_past_mixed_directions() {
        let orders = [("created_at".to_owned(), true), ("id".to_owned(), false)];
        let values = [Value::from(5), Value::from(9)];
        let query = past(QueryBuilder::table("t"), &orders, &values, true);
        assert_eq!(
            query.to_sql(Dialect::MySql).unwrap().sql,
            "SELECT * FROM `t` WHERE ((`created_at` < ?) OR (`created_at` = ? AND `id` > ?))"
        );
        let back = past(QueryBuilder::table("t"), &orders, &values, false);
        assert_eq!(
            back.to_sql(Dialect::MySql).unwrap().sql,
            "SELECT * FROM `t` WHERE ((`created_at` > ?) OR (`created_at` = ? AND `id` < ?))"
        );
    }
}
