//! Database values and their conversions.
//!
//! - `encode.rs`: Rust type to [`Value`] (`From` impls), used for bindings.
//! - `decode.rs`: [`Value`] to Rust type ([`FromValue`]), used when reading rows.

mod decode;
mod encode;

pub(crate) use decode::integer;
pub use decode::{FromValue, ValueError};

use chrono::{NaiveDate, NaiveDateTime, NaiveTime};
use rust_decimal::Decimal;

/// One SQL value, used both for bindings and for row data.
///
/// Date and time values carry no time zone: wall-clock time in UTC, or in
/// the zone set with [`set_timezone`](crate::set_timezone).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum Value {
    /// SQL `NULL`.
    Null,
    /// Boolean. Stored as `TINYINT(1)` on MySQL and `NUMBER(1)` on Oracle.
    Bool(bool),
    /// Signed integer.
    I64(i64),
    /// Unsigned integer.
    U64(u64),
    /// Floating point number.
    F64(f64),
    /// Exact decimal number (`DECIMAL`, Oracle `NUMBER`).
    Decimal(Decimal),
    /// Text.
    String(String),
    /// Binary data.
    Bytes(Vec<u8>),
    /// Calendar date.
    Date(NaiveDate),
    /// Date and time, in the application time zone (UTC by default).
    DateTime(NaiveDateTime),
    /// Time of day.
    Time(NaiveTime),
    /// JSON document.
    Json(serde_json::Value),
}

impl Value {
    /// Returns `true` for [`Value::Null`].
    #[must_use]
    pub fn is_null(&self) -> bool {
        matches!(self, Self::Null)
    }

    /// Variant name, used in error messages.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Bool(_) => "bool",
            Self::I64(_) => "i64",
            Self::U64(_) => "u64",
            Self::F64(_) => "f64",
            Self::Decimal(_) => "decimal",
            Self::String(_) => "string",
            Self::Bytes(_) => "bytes",
            Self::Date(_) => "date",
            Self::DateTime(_) => "datetime",
            Self::Time(_) => "time",
            Self::Json(_) => "json",
        }
    }
}
