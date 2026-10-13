//! [`Row`]: one row of a query result.

use std::sync::Arc;

use crate::{Dialect, Error, FromValue, Result, Value};

/// One row of a query result.
///
/// Column lookup ignores ASCII case, because Oracle reports column names in
/// upper case.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    columns: Arc<[String]>,
    values: Vec<Value>,
    dialect: Dialect,
    /// The query chose its columns, so a model built from the row takes
    /// `Default` values for the columns it lacks.
    partial: bool,
    /// Table of the model being built from the row, for error reports.
    model: Option<&'static str>,
    /// See [`take_field`](Self::take_field).
    in_order: InOrder,
}

/// How many leading columns [`Row::take_field`] consumed in order;
/// `usize::MAX` once a field was out of order. Lookup state, not row data,
/// so it never makes two rows unequal.
#[derive(Debug, Clone, Copy, Default)]
struct InOrder(usize);

impl PartialEq for InOrder {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Row {
    /// Creates a row. All rows of one result set share `columns`.
    ///
    /// # Panics
    ///
    /// Panics if `columns` and `values` differ in length.
    #[must_use]
    pub fn new(columns: Arc<[String]>, values: Vec<Value>, dialect: Dialect) -> Self {
        assert_eq!(
            columns.len(),
            values.len(),
            "a row needs one value per column"
        );
        Self {
            columns,
            values,
            dialect,
            partial: false,
            model: None,
            in_order: InOrder::default(),
        }
    }

    pub(crate) fn set_partial(&mut self) {
        self.partial = true;
    }

    pub(crate) fn set_model(&mut self, table: &'static str) {
        self.model = Some(table);
    }

    /// Column names, in result order.
    #[must_use]
    pub fn columns(&self) -> &[String] {
        &self.columns
    }

    /// Database the row came from.
    pub(crate) fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Values, in result order.
    #[must_use]
    pub fn values(&self) -> &[Value] {
        &self.values
    }

    /// Returns `column` converted to `T`.
    ///
    /// On Oracle, `NULL` first tries [`FromValue::from_oracle_null`], so an
    /// empty `VARCHAR2` (stored as `NULL`) reads as `""` into a `String`.
    ///
    /// # Errors
    ///
    /// [`Error::ColumnNotFound`] when the row has no such column,
    /// [`Error::Decode`] when the value does not fit `T`.
    pub fn get<T: FromValue>(&self, column: &str) -> Result<T> {
        let index = self.index(column)?;
        self.decode(column, self.values[index].clone())
    }

    /// As [`get`](Self::get), but moves the value out instead of cloning
    /// it, leaving `NULL` behind. Models are built from rows this way.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    pub fn take<T: FromValue>(&mut self, column: &str) -> Result<T> {
        let index = self.index(column)?;
        self.take_at(column, index)
    }

    fn take_at<T: FromValue>(&mut self, column: &str, index: usize) -> Result<T> {
        let value = std::mem::replace(&mut self.values[index], Value::Null);
        self.decode(column, value)
    }

    /// As [`take`](Self::take), but a column missing from a row whose query
    /// selected only some columns reads as `T::default()`. Models are built
    /// from rows this way.
    ///
    /// # Errors
    ///
    /// As [`get`](Self::get).
    #[doc(hidden)]
    pub fn take_field<T: FromValue + Default>(&mut self, column: &str) -> Result<T> {
        // While a model's fields follow the result's column order, each is the
        // next column, found without a search. The columns before it belong to
        // the model's other fields, so it is also the first column of that
        // name, the one a search finds (joins can repeat a name).
        let next = self.in_order.0;
        let index = if (self.columns.get(next)).is_some_and(|c| c.eq_ignore_ascii_case(column)) {
            self.in_order.0 += 1;
            Ok(next)
        } else {
            self.in_order.0 = usize::MAX;
            self.index(column)
        };
        match index.and_then(|index| self.take_at(column, index)) {
            Err(Error::ColumnNotFound(_)) if self.partial => Ok(T::default()),
            Err(error @ Error::ColumnNotFound(_)) => {
                error.report(self.model); // the model has a field its table lacks
                Err(error)
            }
            result => result,
        }
    }

    fn index(&self, column: &str) -> Result<usize> {
        self.columns
            .iter()
            .position(|c| c.eq_ignore_ascii_case(column))
            .ok_or_else(|| Error::ColumnNotFound(column.to_owned()))
    }

    fn decode<T: FromValue>(&self, column: &str, value: Value) -> Result<T> {
        if value.is_null()
            && self.dialect == Dialect::Oracle
            && let Some(empty) = T::from_oracle_null()
        {
            return Ok(empty);
        }
        T::from_value(value).map_err(|source| Error::decode(self.model, column, source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(dialect: Dialect) -> Row {
        let columns: Arc<[String]> = ["ID".to_owned(), "NAME".to_owned()].into();
        Row::new(columns, vec![Value::I64(1), Value::Null], dialect)
    }

    #[test]
    fn lookup_ignores_case() {
        assert_eq!(row(Dialect::MySql).get::<i64>("id").unwrap(), 1);
    }

    #[test]
    fn oracle_null_reads_as_empty_string() {
        assert_eq!(row(Dialect::Oracle).get::<String>("name").unwrap(), "");
        assert_eq!(
            row(Dialect::Oracle).get::<Option<String>>("name").unwrap(),
            None
        );
    }

    #[test]
    fn mysql_null_into_string_fails() {
        assert!(matches!(
            row(Dialect::MySql).get::<String>("name"),
            Err(Error::Decode { .. })
        ));
    }

    #[test]
    fn partial_rows_default_missing_fields() {
        let mut row = row(Dialect::MySql);
        assert!(row.take_field::<String>("email").is_err());
        row.set_partial();
        assert_eq!(row.take_field::<String>("email").unwrap(), "");
        assert_eq!(row.take_field::<i64>("id").unwrap(), 1);
    }

    #[test]
    fn model_rows_name_the_model_in_decode_errors() {
        let mut row = row(Dialect::MySql);
        row.set_model("users");
        let error = row.take_field::<String>("name").unwrap_err();
        assert!(matches!(
            error,
            Error::Decode {
                model: Some("users"),
                ..
            }
        ));
        assert!(
            error
                .to_string()
                .starts_with("cannot decode column `users.name`"),
            "{error}"
        );
    }

    #[test]
    fn fields_out_of_order_take_the_first_column_of_a_name() {
        let columns: Arc<[String]> = ["name", "id", "id"].map(str::to_owned).into();
        let values = vec![Value::from("a"), Value::I64(1), Value::I64(2)];
        let mut row = Row::new(columns, values, Dialect::MySql);
        // `id` first: out of order, so it is searched and the first `id` wins.
        assert_eq!(row.take_field::<i64>("id").unwrap(), 1);
        assert_eq!(row.take_field::<String>("name").unwrap(), "a");
    }

    #[test]
    fn missing_column_is_reported() {
        assert!(matches!(
            row(Dialect::MySql).get::<i64>("email"),
            Err(Error::ColumnNotFound(_))
        ));
    }
}
