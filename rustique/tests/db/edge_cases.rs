//! Edge cases found in a bug hunt: values at the limits of each driver,
//! dialect quirks and builder combinations that used to fail.

use chrono::NaiveDateTime;
use rust_decimal::Decimal;
use rustique::{Dialect, Executor, QueryBuilder, Value};

use crate::{create_table, databases};

#[tokio::test]
async fn repeating_decimals_read_as_numbers() {
    for db in databases().await {
        create_table(&db, "orm_edge_avg", &[("n", "INT", "NUMBER(10)")]).await;
        let t = QueryBuilder::table("orm_edge_avg");
        t.insert(
            &db,
            &["n"],
            vec![vec![1.into()], vec![1.into()], vec![2.into()]],
        )
        .await
        .unwrap();
        // MySQL rounds AVG to 4 decimals (`div_precision_increment`).
        let avg: f64 = t.avg(&db, "n").await.unwrap();
        assert!((avg - 4.0 / 3.0).abs() < 1e-3, "{avg}");
        let avg: Decimal = t.avg(&db, "n").await.unwrap();
        assert!(avg > Decimal::ONE);
        let rows = db
            .select("SELECT 1/3 AS x, 0.5 AS h FROM DUAL", [])
            .await
            .unwrap();
        let x: f64 = rows[0].get("x").unwrap();
        assert!((x - 1.0 / 3.0).abs() < 1e-3, "{x}");
        assert_eq!(rows[0].get::<f64>("h").unwrap(), 0.5);
    }
}

#[tokio::test]
async fn large_text_and_binary_round_trip() {
    for db in databases().await {
        create_table(
            &db,
            "orm_edge_lob",
            &[("body", "LONGTEXT", "CLOB"), ("data", "LONGBLOB", "BLOB")],
        )
        .await;
        let t = QueryBuilder::table("orm_edge_lob");
        for size in [3_000_usize, 5_000, 40_000, 100_000] {
            let text = "é".repeat(size / 2);
            let bytes = vec![7_u8; size];
            t.insert(
                &db,
                &["body", "data"],
                vec![vec![text.clone().into(), bytes.clone().into()]],
            )
            .await
            .unwrap_or_else(|e| panic!("{} insert {size}: {e}", db.dialect()));
            let row = t
                .clone()
                .order_by_desc("id")
                .first(&db)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(row.get::<String>("body").unwrap(), text);
            assert_eq!(row.get::<Vec<u8>>("data").unwrap(), bytes);
        }
        // NULL into a BLOB, alone and in a batch.
        t.insert(&db, &["body", "data"], vec![vec![Value::Null, Value::Null]])
            .await
            .unwrap();
        let rows = vec![
            vec!["a".into(), Value::Null],
            vec!["x".repeat(50_000).into(), vec![1_u8; 50_000].into()],
        ];
        t.insert(&db, &["body", "data"], rows)
            .await
            .unwrap_or_else(|e| panic!("{} batch: {e}", db.dialect()));
        // Oracle binds these as LOBs: as VARCHAR2 only an INSERT's VALUES takes them.
        let last = t
            .clone()
            .order_by_desc("id")
            .limit(1)
            .pluck::<i64>(&db, "id")
            .await
            .unwrap();
        let updated = t
            .clone()
            .where_("id", "=", last[0])
            .update(
                &db,
                vec![
                    ("body", "y".repeat(40_000).into()),
                    ("data", vec![2_u8; 40_000].into()),
                ],
            )
            .await
            .unwrap_or_else(|e| panic!("{} update: {e}", db.dialect()));
        assert_eq!(updated, 1);
        let row = t
            .clone()
            .where_("id", "=", last[0])
            .first(&db)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get::<String>("body").unwrap().len(), 40_000);
        assert_eq!(row.get::<Vec<u8>>("data").unwrap(), vec![2_u8; 40_000]);
    }
}

#[tokio::test]
async fn lock_for_update_with_limit_and_offset() {
    for db in databases().await {
        create_table(
            &db,
            "orm_edge_lock",
            &[("name", "VARCHAR(20)", "VARCHAR2(20)")],
        )
        .await;
        let t = QueryBuilder::table("orm_edge_lock");
        t.insert(&db, &["name"], vec![vec!["a".into()], vec!["b".into()]])
            .await
            .unwrap();
        let tx = db.begin().await.unwrap();
        let row = t
            .clone()
            .where_("name", "=", "b")
            .lock_for_update()
            .first(&tx)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()))
            .unwrap();
        assert_eq!(row.get::<String>("name").unwrap(), "b");
        let rows = t
            .clone()
            .order_by_desc("name")
            .offset(1)
            .limit(1)
            .lock_for_update()
            .get(&tx)
            .await
            .unwrap();
        assert_eq!(rows[0].get::<String>("name").unwrap(), "a");
        // Both rows are locked for other transactions.
        let other = db.begin().await.unwrap();
        let free = t
            .clone()
            .lock_for_update()
            .skip_locked()
            .get(&other)
            .await
            .unwrap();
        assert!(free.is_empty(), "{}: {free:?}", db.dialect());
        other.rollback().await.unwrap();
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn exists_on_unions_and_havings() {
    for db in databases().await {
        create_table(
            &db,
            "orm_edge_ex",
            &[("name", "VARCHAR(20)", "VARCHAR2(20)")],
        )
        .await;
        let t = QueryBuilder::table("orm_edge_ex");
        t.insert(&db, &["name"], vec![vec!["a".into()], vec!["a".into()]])
            .await
            .unwrap();
        let union = t
            .clone()
            .select(["id", "name"])
            .where_("name", "=", "zzz")
            .union(t.clone().select(["id", "name"]));
        assert!(union.exists(&db).await.unwrap());
        assert_eq!(union.count(&db).await.unwrap(), 2);
        let dupes = t
            .clone()
            .select(["name"])
            .select_raw("COUNT(*) AS c", [])
            .group_by(["name"])
            .having_raw("COUNT(*) > ?", [1.into()]);
        assert!(dupes.exists(&db).await.unwrap());
        assert!(!t.clone().limit(0).exists(&db).await.unwrap());
    }
}

#[tokio::test]
async fn where_json_compares_each_json_type() {
    for db in databases().await {
        create_table(
            &db,
            "orm_edge_json",
            &[("doc", "JSON", "CLOB CHECK (doc IS JSON)")],
        )
        .await;
        let t = QueryBuilder::table("orm_edge_json");
        let doc = serde_json::json!({"on": true, "n": 10, "s": "x", "a-b": {"c": [1, 2]}});
        t.insert(&db, &["doc"], vec![vec![doc.into()]])
            .await
            .unwrap();
        for (path, op, value) in [
            ("doc->on", "=", Value::from(true)),
            ("doc->n", ">", Value::from(9)),
            ("doc->n", "=", Value::from(10)),
            ("doc->s", "=", Value::from("x")),
            ("doc->a-b->c->1", "=", Value::from(2)),
        ] {
            let n = t
                .clone()
                .where_json(path, op, value.clone())
                .count(&db)
                .await;
            assert_eq!(n.ok(), Some(1), "{} {path} {op} {value:?}", db.dialect());
        }
    }
}

#[tokio::test]
async fn mysql_zero_dates_and_long_times_read() {
    for db in databases().await {
        if db.dialect() != Dialect::MySql {
            continue;
        }
        let tx = db.begin().await.unwrap();
        tx.statement("SET SESSION sql_mode = ''", []).await.unwrap();
        tx.statement("DROP TABLE IF EXISTS orm_edge_zero", [])
            .await
            .unwrap();
        tx.statement(
            "CREATE TABLE orm_edge_zero (id INT PRIMARY KEY, d DATETIME NULL, dd DATE NULL, t TIME NULL)",
            [],
        )
        .await
        .unwrap();
        tx.statement(
            "INSERT INTO orm_edge_zero VALUES (1, '0000-00-00 00:00:00', '0000-00-00', '25:00:00')",
            [],
        )
        .await
        .unwrap();
        let row = QueryBuilder::table("orm_edge_zero")
            .first(&tx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.get::<Option<NaiveDateTime>>("d").unwrap(), None);
        assert_eq!(row.get::<Option<chrono::NaiveDate>>("dd").unwrap(), None);
        assert_eq!(row.get::<String>("t").unwrap(), "25:00:00");
        tx.rollback().await.unwrap();
    }
}

#[tokio::test]
async fn mysql_runs_statements_it_cannot_prepare() {
    for db in databases().await {
        if db.dialect() != Dialect::MySql {
            continue;
        }
        create_table(&db, "orm_edge_proc", &[("n", "INT", "NUMBER(10)")]).await;
        db.statement("DROP PROCEDURE IF EXISTS orm_edge_add", [])
            .await
            .unwrap();
        // Error 1295 through the prepared statement protocol.
        db.statement(
            "CREATE PROCEDURE orm_edge_add(IN x INT) BEGIN INSERT INTO orm_edge_proc (n) VALUES (x); END",
            [],
        )
        .await
        .unwrap();
        db.statement("CALL orm_edge_add(?)", [5.into()])
            .await
            .unwrap();
        let tx = db.begin().await.unwrap();
        tx.statement("LOCK TABLES orm_edge_proc WRITE", [])
            .await
            .unwrap();
        tx.statement("UNLOCK TABLES", []).await.unwrap();
        tx.rollback().await.unwrap();
        let n: i64 = QueryBuilder::table("orm_edge_proc")
            .value(&db, "n")
            .await
            .unwrap();
        assert_eq!(n, 5);
    }
}

#[tokio::test]
async fn mixed_number_types_in_one_batch_column() {
    for db in databases().await {
        create_table(
            &db,
            "orm_edge_mix",
            &[("n", "DECIMAL(10,2)", "NUMBER(10,2)")],
        )
        .await;
        let t = QueryBuilder::table("orm_edge_mix");
        let rows = vec![
            vec![Value::from(1)],
            vec![Value::Decimal(Decimal::new(15, 1))],
            vec![Value::from(2.5)],
            vec![Value::from(3_u64)],
        ];
        let inserted = t.insert(&db, &["n"], rows).await;
        assert!(inserted.is_ok(), "{}: {inserted:?}", db.dialect());
        let sum: Decimal = t.sum(&db, "n").await.unwrap();
        assert_eq!(sum, Decimal::new(80, 1));
    }
}

#[tokio::test]
async fn huge_page_sizes_do_not_overflow() {
    for db in databases().await {
        create_table(&db, "orm_edge_page", &[("n", "INT", "NUMBER(10)")]).await;
        let t = QueryBuilder::table("orm_edge_page").order_by("id");
        let page = t.simple_paginate(&db, 1, u64::MAX).await;
        assert!(page.is_ok(), "{page:?}");
        let page = t.cursor_paginate(&db, u64::MAX, None).await;
        assert!(page.is_ok(), "{page:?}");
        let n = t.decrement(&db, "n", i64::MIN, vec![]).await;
        assert!(n.is_err());
    }
}

#[rustique::model(table = "orm_edge_morph_posts")]
#[derive(Debug, Clone, Default)]
struct MorphPost {
    id: i64,
}

#[derive(Debug, Clone, rustique::MorphTo)]
#[allow(dead_code)] // only `None` is expected here
enum Target {
    MorphPost(MorphPost),
}

#[rustique::model(table = "orm_edge_morph_notes")]
#[derive(Debug, Clone, Default)]
struct MorphNote {
    id: i64,
    target_type: String,
    target_id: i64,
    #[rustique(morph_to)]
    target: rustique::Rel<Option<Target>>,
}

/// Laravel rows often carry `''` and `0` instead of NULLs: no target, not an
/// unknown morph type.
#[tokio::test]
async fn empty_morph_type_points_nowhere() {
    use rustique::Model;
    for db in databases().await {
        create_table(&db, MorphPost::TABLE, &[("n", "INT", "NUMBER(10)")]).await;
        create_table(
            &db,
            MorphNote::TABLE,
            &[
                ("target_type", "VARCHAR(50)", "VARCHAR2(50)"),
                ("target_id", "BIGINT", "NUMBER(19)"),
            ],
        )
        .await;
        QueryBuilder::table(MorphNote::TABLE)
            .insert(
                &db,
                &["target_type", "target_id"],
                vec![vec!["".into(), 0.into()]],
            )
            .await
            .unwrap();
        let notes = MorphNote::query().with("target").get(&db).await.unwrap();
        assert!(matches!(notes[0].target, rustique::Rel::Loaded(None)));
        assert!(notes[0].target(&db).await.unwrap().is_none());
    }
}

#[rustique::model(table = "orm_edge_types")]
#[derive(Debug, Clone, Default, PartialEq)]
struct Typed {
    id: i64,
    opens: chrono::NaiveTime,
    payload: String,
    flag: bool,
}

/// A model reads back what it wrote on both databases: Oracle has no TIME
/// column (times are text), MySQL hands JSON columns over as JSON.
#[tokio::test]
async fn models_read_back_dialect_specific_columns() {
    use rustique::Model;
    for db in databases().await {
        create_table(
            &db,
            Typed::TABLE,
            &[
                ("opens", "TIME(6)", "VARCHAR2(20)"),
                ("payload", "JSON", "CLOB CHECK (payload IS JSON)"),
                ("flag", "BIT(1)", "NUMBER(1)"),
            ],
        )
        .await;
        let opens = chrono::NaiveTime::from_hms_micro_opt(8, 30, 0, 250).unwrap();
        let typed = Typed::create(
            &db,
            Typed {
                opens,
                payload: r#"{"a":1}"#.into(),
                flag: true,
                ..Default::default()
            },
        )
        .await
        .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()));
        let found = Typed::find_or_fail(&db, typed.id)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()));
        assert_eq!(found.opens, opens);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&found.payload).unwrap(),
            serde_json::json!({"a": 1})
        );
        assert!(found.flag);
    }
}

#[rustique::model(table = "orm_edge_upsert")]
#[derive(Debug, Clone, Default)]
struct Doc {
    id: i64,
    code: String,
    body: String,
}

#[tokio::test]
async fn upsert_long_text() {
    use rustique::Model;
    for db in databases().await {
        create_table(
            &db,
            Doc::TABLE,
            &[
                ("code", "VARCHAR(20) UNIQUE", "VARCHAR2(20) UNIQUE"),
                ("body", "LONGTEXT", "CLOB"),
            ],
        )
        .await;
        let doc = |body: String| Doc {
            code: "a".into(),
            body,
            ..Default::default()
        };
        for size in [10, 5_000, 40_000] {
            Doc::upsert(&db, &[doc("é".repeat(size))], &[Doc::CODE], &[Doc::BODY])
                .await
                .unwrap_or_else(|e| panic!("{} {size}: {e}", db.dialect()));
            let body: String = Doc::query().value(&db, Doc::BODY).await.unwrap();
            assert_eq!(body.chars().count(), size);
        }
    }
}

#[tokio::test]
async fn oracle_alternative_quoting_in_raw_sql() {
    for db in databases().await {
        if db.dialect() != Dialect::Oracle {
            continue;
        }
        let rows = db
            .select("SELECT q'[it's ?]' AS t, ? AS b FROM DUAL", [7.into()])
            .await
            .unwrap();
        assert_eq!(rows[0].get::<String>("t").unwrap(), "it's ?");
        assert_eq!(rows[0].get::<i64>("b").unwrap(), 7);
    }
}

#[tokio::test]
async fn where_in_beyond_the_binding_limit() {
    for db in databases().await {
        create_table(&db, "orm_edge_in", &[("n", "INT", "NUMBER(10)")]).await;
        let t = QueryBuilder::table("orm_edge_in");
        t.insert(&db, &["n"], (0..10).map(|n| vec![n.into()]).collect())
            .await
            .unwrap();
        // 70,000 integers: more than the 65,535 bindings of a statement.
        let found = t
            .clone()
            .where_in("n", 5..70_005)
            .where_("n", "<", 8)
            .count(&db)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()));
        assert_eq!(found, 3, "{}", db.dialect());
        let not_found = t
            .clone()
            .where_not_in("n", 3..70_000)
            .count(&db)
            .await
            .unwrap();
        assert_eq!(not_found, 3, "{}", db.dialect());
        let pairs = t
            .clone()
            .where_rows_in(
                ["id", "n"],
                (0..70_000).map(|n| vec![(n + 1).into(), n.into()]),
            )
            .count(&db)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()));
        assert_eq!(pairs, 10, "{}", db.dialect());
    }
}
