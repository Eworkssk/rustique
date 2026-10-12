use chrono::{NaiveDate, NaiveDateTime};
use rust_decimal::Decimal;
use rustique::{Dialect, Executor, QueryBuilder, Value};
use serde_json::json;

use crate::{create_table, databases};

#[tokio::test]
async fn values_round_trip() {
    for db in databases().await {
        create_table(
            &db,
            "orm_values",
            &[
                ("name", "VARCHAR(100)", "VARCHAR2(100 CHAR)"),
                ("blank", "VARCHAR(100)", "VARCHAR2(100 CHAR)"),
                ("price", "DECIMAL(10,2)", "NUMBER(10,2)"),
                ("ratio", "DOUBLE", "BINARY_DOUBLE"),
                ("active", "TINYINT(1)", "NUMBER(1)"),
                ("born", "DATE", "DATE"),
                ("seen_at", "DATETIME(6)", "TIMESTAMP(6)"),
                ("payload", "BLOB", "BLOB"),
                ("doc", "JSON", "CLOB CHECK (doc IS JSON)"),
                ("note", "TEXT", "CLOB"),
            ],
        )
        .await;

        let born = NaiveDate::from_ymd_opt(1990, 5, 17).unwrap();
        let seen: NaiveDateTime = NaiveDate::from_ymd_opt(2026, 10, 10)
            .unwrap()
            .and_hms_micro_opt(12, 34, 56, 789)
            .unwrap();
        let table = QueryBuilder::table("orm_values");
        let columns = [
            "name", "blank", "price", "ratio", "active", "born", "seen_at", "payload", "doc",
            "note",
        ];
        let row = vec![
            "Ann".into(),
            "".into(),
            Decimal::new(1999, 2).into(),
            0.5.into(),
            true.into(),
            born.into(),
            seen.into(),
            vec![1_u8, 2, 3].into(),
            json!({"a": 1}).into(),
            Value::Null,
        ];
        let inserted = table.insert(&db, &columns, vec![row]).await.unwrap();
        assert_eq!(inserted.rows_affected, 1);
        if db.dialect() == Dialect::MySql {
            assert_eq!(inserted.last_insert_id, Some(1));
        }

        let row = table.first(&db).await.unwrap().expect("one row");
        assert_eq!(row.get::<i64>("id").unwrap(), 1);
        assert_eq!(row.get::<String>("name").unwrap(), "Ann");
        // Oracle stores '' as NULL; it still reads back as "".
        assert_eq!(row.get::<String>("blank").unwrap(), "");
        assert_eq!(row.get::<Decimal>("price").unwrap(), Decimal::new(1999, 2));
        assert_eq!(row.get::<f64>("ratio").unwrap(), 0.5);
        assert!(row.get::<bool>("active").unwrap());
        assert_eq!(row.get::<NaiveDate>("born").unwrap(), born);
        assert_eq!(row.get::<NaiveDateTime>("seen_at").unwrap(), seen);
        assert_eq!(row.get::<Vec<u8>>("payload").unwrap(), [1, 2, 3]);
        assert_eq!(
            row.get::<serde_json::Value>("doc").unwrap(),
            json!({"a": 1})
        );
        assert_eq!(row.get::<Option<String>>("note").unwrap(), None);
    }
}

#[tokio::test]
async fn raw_queries_use_question_mark_placeholders() {
    for db in databases().await {
        let rows = db
            .select(
                "SELECT ? AS greeting, '?' AS literal FROM DUAL",
                ["hi".into()],
            )
            .await
            .unwrap();
        assert_eq!(rows[0].get::<String>("greeting").unwrap(), "hi");
        assert_eq!(rows[0].get::<String>("literal").unwrap(), "?");
    }
}

#[tokio::test]
async fn multi_row_insert_handles_leading_nulls_and_growing_strings() {
    for db in databases().await {
        create_table(
            &db,
            "orm_batch",
            &[
                ("label", "VARCHAR(500)", "VARCHAR2(500 CHAR)"),
                ("score", "INT", "NUMBER(10)"),
            ],
        )
        .await;
        let table = QueryBuilder::table("orm_batch");
        let long = "x".repeat(300);
        let rows = vec![
            vec!["a".into(), Value::Null],
            vec![long.clone().into(), 2.into()],
            vec![Value::Null, 3.into()],
        ];
        let inserted = table.insert(&db, &["label", "score"], rows).await.unwrap();
        assert_eq!(inserted.rows_affected, 3);

        let rows = table.clone().order_by("id").get(&db).await.unwrap();
        let scores: Vec<Option<i64>> = rows.iter().map(|r| r.get("score").unwrap()).collect();
        assert_eq!(scores, [None, Some(2), Some(3)]);
        assert_eq!(rows[1].get::<String>("label").unwrap(), long);
    }
}
