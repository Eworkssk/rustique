//! [`FromValue`]: converting a [`Value`] back into a Rust type.

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

use super::Value;

/// A value's type or range does not fit the requested Rust type.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("expected {expected}, found {found}")]
pub struct ValueError {
    /// Rust type that was requested.
    pub expected: &'static str,
    /// What the value actually was.
    pub found: &'static str,
}

impl ValueError {
    fn new<T>(found: &'static str) -> Self {
        Self {
            expected: std::any::type_name::<T>(),
            found,
        }
    }
}

/// Converts a [`Value`] into a Rust type.
///
/// Implemented for integers, floats, `bool`, [`String`], `Vec<u8>`, [`Decimal`],
/// `chrono` types, `serde_json::Value` and `Option<T>`. For your own enums,
/// newtypes and JSON documents, `#[derive(rustique::Cast)]` implements it (and
/// `From<T> for Value`); implement it by hand for anything else.
pub trait FromValue: Sized {
    /// Converts `value`.
    ///
    /// # Errors
    ///
    /// Returns [`ValueError`] when the value's type or range does not fit `Self`.
    fn from_value(value: Value) -> Result<Self, ValueError>;

    /// Replacement for `NULL` read from Oracle, which stores `''` as `NULL`.
    ///
    /// `None` (the default) keeps `NULL` an error; [`String`] returns `""`.
    fn from_oracle_null() -> Option<Self> {
        None
    }
}

impl FromValue for Value {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        Ok(value)
    }
}

impl<T: FromValue> FromValue for Option<T> {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::Null => Ok(None),
            value => T::from_value(value).map(Some),
        }
    }
}

/// Integer view of a numeric value; `None` when the value is not a whole number.
pub(crate) fn integer(value: &Value) -> Option<i128> {
    match value {
        Value::Bool(b) => Some(i128::from(*b)),
        Value::I64(n) => Some(i128::from(*n)),
        Value::U64(n) => Some(i128::from(*n)),
        Value::Decimal(d) if d.fract().is_zero() => d.to_i128(),
        _ => None,
    }
}

macro_rules! impl_integer {
    ($($ty:ty),*) => {$(
        impl FromValue for $ty {
            fn from_value(value: Value) -> Result<Self, ValueError> {
                let n = integer(&value).ok_or_else(|| ValueError::new::<Self>(value.kind()))?;
                Self::try_from(n).map_err(|_| ValueError::new::<Self>("out-of-range integer"))
            }
        }
    )*};
}

impl_integer!(i8, i16, i32, i64, u8, u16, u32, u64);

impl FromValue for bool {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        // Neither MySQL (TINYINT(1)) nor Oracle 19c (NUMBER(1)) has a boolean column type.
        match &value {
            // MySQL BIT(n) columns.
            Value::Bytes(bits) => Ok(bits.iter().any(|&b| b != 0)),
            _ => integer(&value)
                .map(|n| n != 0)
                .ok_or_else(|| ValueError::new::<Self>(value.kind())),
        }
    }
}

impl FromValue for f64 {
    #[allow(clippy::cast_precision_loss)] // reading an integer column into f64 is an explicit request
    fn from_value(value: Value) -> Result<Self, ValueError> {
        let n = match &value {
            Value::F64(n) => Some(*n),
            Value::I64(n) => Some(*n as f64),
            Value::U64(n) => Some(*n as f64),
            Value::Decimal(d) => d.to_f64(),
            _ => None,
        };
        n.ok_or_else(|| ValueError::new::<Self>(value.kind()))
    }
}

impl FromValue for f32 {
    #[allow(clippy::cast_possible_truncation)] // the caller asked for f32 precision
    fn from_value(value: Value) -> Result<Self, ValueError> {
        f64::from_value(value).map(|n| n as f32)
    }
}

impl FromValue for Decimal {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        let d = match &value {
            Value::Decimal(d) => Some(*d),
            Value::I64(n) => Some(Self::from(*n)),
            Value::U64(n) => Some(Self::from(*n)),
            Value::F64(n) => Self::try_from(*n).ok(),
            Value::String(s) => s.parse().ok(),
            _ => None,
        };
        d.ok_or_else(|| ValueError::new::<Self>(value.kind()))
    }
}

impl FromValue for String {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::String(s) => Ok(s),
            // MySQL JSON columns; Oracle hands the same text over as a string.
            Value::Json(j) => Ok(j.to_string()),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }

    fn from_oracle_null() -> Option<Self> {
        Some(Self::new())
    }
}

impl FromValue for Vec<u8> {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::Bytes(b) => Ok(b),
            Value::String(s) => Ok(s.into_bytes()),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }
}

impl FromValue for NaiveDate {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::Date(d) => Ok(d),
            Value::DateTime(dt) => Ok(dt.date()),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }
}

impl FromValue for NaiveDateTime {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::DateTime(dt) => Ok(dt),
            Value::Date(d) => Ok(d.and_time(NaiveTime::MIN)),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }
}

impl FromValue for NaiveTime {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        match value {
            Value::Time(t) => Ok(t),
            // Oracle has no TIME type; times are stored as `HH:MM:SS[.f]` text.
            Value::String(s) => s
                .parse()
                .map_err(|_| ValueError::new::<Self>("text that is not a time of day")),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }
}

impl FromValue for DateTime<Utc> {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        NaiveDateTime::from_value(value).map(|dt| crate::timezone::utc(&dt))
    }
}

impl FromValue for serde_json::Value {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        // Oracle 19c stores JSON in CLOB / VARCHAR2, so text must parse too.
        let invalid = |_| ValueError::new::<Self>("invalid JSON text");
        match value {
            Value::Json(j) => Ok(j),
            Value::String(s) => serde_json::from_str(&s).map_err(invalid),
            Value::Bytes(b) => serde_json::from_slice(&b).map_err(invalid),
            other => Err(ValueError::new::<Self>(other.kind())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integers_accept_whole_decimals_and_reject_out_of_range() {
        assert_eq!(i64::from_value(Value::Decimal(Decimal::from(42))), Ok(42));
        assert!(i64::from_value(Value::Decimal(Decimal::new(15, 1))).is_err());
        assert_eq!(
            u8::from_value(Value::I64(300)).unwrap_err().found,
            "out-of-range integer"
        );
    }

    #[test]
    fn bool_reads_numeric_columns() {
        assert_eq!(bool::from_value(Value::I64(1)), Ok(true));
        assert_eq!(bool::from_value(Value::Decimal(Decimal::ZERO)), Ok(false));
    }

    #[test]
    fn option_maps_null_to_none() {
        assert_eq!(Option::<String>::from_value(Value::Null), Ok(None));
        assert_eq!(Option::<i64>::from_value(Value::I64(7)), Ok(Some(7)));
    }

    #[test]
    fn null_into_string_is_an_error() {
        assert_eq!(String::from_value(Value::Null).unwrap_err().found, "NULL");
    }

    #[test]
    fn json_parses_text_columns() {
        let json = serde_json::Value::from_value(Value::from(r#"{"a":1}"#)).unwrap();
        assert_eq!(json["a"], 1);
    }
}
