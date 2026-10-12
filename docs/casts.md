# Casts

A model field can be any type that implements both `rustique::FromValue` (read from a column) and `From<T> for rustique::Value` (bind as a parameter). This replaces Eloquent's `$casts`, accessors and mutators.

## Built-in types

Integers, floats, `bool`, `String`, `Vec<u8>`, `chrono` dates and times, `rust_decimal::Decimal`, `serde_json::Value`, and `Option<T>` of any of them. `NULL` decodes to `None`; into a non-`Option` field it is an `Error::Decode` (except on Oracle, where `NULL` into a `String` is `""`, since Oracle stores `''` as `NULL`).

## `#[derive(rustique::Cast)]`

```rust
# use rustique::prelude::*;
// text column: "pending_review", "approved", "gone"
#[derive(Debug, Clone, Copy, Default, PartialEq, rustique::Cast)]
pub enum Status {
    #[default]
    PendingReview,
    Approved,
    #[cast(rename = "gone")]
    Deleted,
}

// integer column: the discriminant
#[derive(Debug, Clone, Copy, Default, rustique::Cast)]
#[cast(int)]
pub enum Priority { #[default] Low = 1, High = 2 }

// newtype: stored as its inner value
#[derive(Debug, Clone, Default, rustique::Cast)]
pub struct Email(pub String);

// any serde type: stored as JSON
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize, rustique::Cast)]
#[cast(json)]
pub struct Settings { pub theme: String, pub beta: bool }

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
pub struct User {
    pub id: i64,
    pub status: Status,
    pub email: Email,
    pub settings: Settings,
    pub priority: Option<Priority>,
}

# async fn run(db: Db) -> rustique::Result<()> {
User::query().where_("status", "=", Status::Approved).get(&db).await?;   // casts work as bindings too
# Ok(()) }
```

Model fields need `Default` (models are built with `..Default::default()`), so give cast types a `Default`.

## Hand-written casts

```rust
use rustique::{FromValue, Value, ValueError};

pub struct Cents(pub i64);

impl From<Cents> for Value {
    fn from(c: Cents) -> Self { Value::I64(c.0) }
}

impl FromValue for Cents {
    fn from_value(value: Value) -> Result<Self, ValueError> {
        i64::from_value(value).map(Cents)
    }
}
```

Override `FromValue::from_oracle_null` to return a value for Oracle `NULL`s (as `String` does with `""`).

## `Value`

`Value` is the enum every binding and column passes through: `Null`, `Bool`, `I64`, `U64`, `F64`, `Decimal`, `String`, `Bytes`, `Date`, `DateTime`, `Time`, `Json`. `DateTime` carries no zone: wall-clock time in UTC, or in the zone set with `rustique::set_timezone` (see [timestamps](models.md#timestamps)).
