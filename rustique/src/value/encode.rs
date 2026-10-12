//! `From` conversions from Rust types into [`Value`].

use chrono::{DateTime, NaiveDate, NaiveDateTime, NaiveTime, Utc};
use rust_decimal::Decimal;

use super::Value;

macro_rules! impl_from {
    ($($ty:ty => $variant:ident),* $(,)?) => {
        $(impl From<$ty> for Value {
            fn from(value: $ty) -> Self {
                Self::$variant(value.into())
            }
        })*
    };
}

impl_from! {
    bool => Bool,
    i8 => I64, i16 => I64, i32 => I64, i64 => I64,
    u8 => U64, u16 => U64, u32 => U64, u64 => U64,
    f32 => F64, f64 => F64,
    Decimal => Decimal,
    String => String, &str => String, &String => String,
    Vec<u8> => Bytes, &[u8] => Bytes,
    NaiveDate => Date, NaiveDateTime => DateTime, NaiveTime => Time,
    serde_json::Value => Json,
}

impl From<DateTime<Utc>> for Value {
    fn from(value: DateTime<Utc>) -> Self {
        Self::DateTime(crate::timezone::local(&value))
    }
}

impl<T: Into<Value>> From<Option<T>> for Value {
    fn from(value: Option<T>) -> Self {
        value.map_or(Self::Null, Into::into)
    }
}
