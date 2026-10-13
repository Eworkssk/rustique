use rustique::{FromValue, Model, QueryBuilder, Value};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::binds;

#[derive(Debug, Clone, Copy, PartialEq, Default, rustique::Cast)]
enum Status {
    #[default]
    Pending,
    PendingReview,
    #[cast(rename = "banned_perm")]
    Banned,
}

#[derive(Debug, Clone, Copy, PartialEq, Default, rustique::Cast)]
#[cast(int)]
enum Priority {
    #[default]
    Low = 1,
    High = 10,
    Urgent, // implicit discriminant: 11
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, rustique::Cast)]
#[cast(json)]
struct Settings {
    theme: String,
    tags: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Default, rustique::Cast)]
struct Email(String);

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
    email: Email,
    status: Status,
    priority: Priority,
    settings: Settings,
    archived_as: Option<Status>,
}

#[track_caller]
fn round_trip<T: FromValue + Into<Value> + Clone + PartialEq + std::fmt::Debug>(
    v: T,
    stored: Value,
) {
    assert_eq!(v.clone().into(), stored);
    assert_eq!(T::from_value(stored).unwrap(), v);
}

#[test]
fn string_enum() {
    round_trip(Status::Pending, Value::from("pending"));
    round_trip(Status::PendingReview, Value::from("pending_review"));
    round_trip(Status::Banned, Value::from("banned_perm"));
    assert_eq!(
        Status::from_value("Banned".into()).unwrap_err().to_string(),
        "expected sql::casts::Status, found unknown variant"
    );
    assert!(Status::from_value(Value::I64(1)).is_err());
}

#[test]
fn int_enum() {
    round_trip(Priority::Low, Value::I64(1));
    round_trip(Priority::High, Value::I64(10));
    round_trip(Priority::Urgent, Value::I64(11));
    // Oracle NUMBER comes back as a decimal
    assert_eq!(
        Priority::from_value(Value::Decimal(10.into())).unwrap(),
        Priority::High
    );
    assert!(Priority::from_value(Value::I64(2)).is_err());
}

#[test]
fn json_struct() {
    let settings = Settings {
        theme: "dark".into(),
        tags: vec!["a".into()],
    };
    round_trip(
        settings.clone(),
        // Serialized in field order, the text the column receives.
        Value::Json(r#"{"theme":"dark","tags":["a"]}"#.into()),
    );
    // Oracle 19c returns JSON stored in a CLOB as text
    let text = Value::from(r#"{"theme":"dark","tags":["a"]}"#);
    assert_eq!(Settings::from_value(text).unwrap(), settings);
    assert!(Settings::from_value(Value::from(json!({"theme": 1}))).is_err());
}

#[test]
fn newtype() {
    round_trip(Email("a@b.c".into()), Value::from("a@b.c"));
    assert_eq!(Email::from_oracle_null(), Some(Email(String::new())));
}

#[test]
fn option_and_null() {
    assert_eq!(Option::<Status>::from_value(Value::Null).unwrap(), None);
    assert_eq!(
        Value::from(Some(Status::Banned)),
        Value::from("banned_perm")
    );
    assert_eq!(Value::from(None::<Status>), Value::Null);
}

#[test]
fn model_fields_and_bindings() {
    let mut user = User {
        email: Email("a@b.c".into()),
        status: Status::Banned,
        priority: Priority::High,
        settings: Settings {
            theme: "dark".into(),
            tags: vec![],
        },
        ..Default::default()
    };
    assert_eq!(
        user.to_values(),
        [
            Value::I64(0),
            Value::from("a@b.c"),
            Value::from("banned_perm"),
            Value::I64(10),
            Value::Json(r#"{"theme":"dark","tags":[]}"#.into()),
            Value::Null,
        ]
    );
    user.set_value("archived_as", "pending".into()).unwrap();
    assert_eq!(user.archived_as, Some(Status::Pending));

    let query = QueryBuilder::table("users")
        .where_("status", "=", Status::Pending)
        .where_("priority", ">=", Priority::High);
    assert_eq!(binds(&query), [Value::from("pending"), Value::I64(10)]);
}
