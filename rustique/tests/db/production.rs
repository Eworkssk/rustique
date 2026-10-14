//! Bulk copies, sequences, lock waits and fetch sizes.

use std::time::Duration;

use rustique::{Db, Dialect, Error, Executor, QueryBuilder};

use crate::{create_table, databases};

#[tokio::test]
async fn insert_using_copies_rows_selected_by_a_long_in_list() {
    for db in databases().await {
        let columns = [("total", "INT", "NUMBER(10)")];
        create_table(&db, "orm_iu_orders", &columns).await;
        create_table(&db, "orm_iu_archive", &columns).await;
        let orders = QueryBuilder::table("orm_iu_orders");
        let rows = (1..=2000).map(|i| vec![(i * 10).into()]).collect();
        orders.insert(&db, &["total"], rows).await.unwrap();

        // Over 1,000 integers: MySQL inlines them, Oracle binds one JSON array.
        let ids: Vec<i64> = (1..=1500).collect();
        let source = orders
            .clone()
            .select(["id", "total"])
            .where_("total", ">", 100)
            .where_in("id", ids);
        let archive = QueryBuilder::table("orm_iu_archive");
        let copied = archive
            .insert_using(&db, &["id", "total"], source)
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", db.dialect()));
        assert_eq!(copied, 1490, "{}", db.dialect());
        assert_eq!(archive.count(&db).await.unwrap(), 1490);
        assert_eq!(archive.max::<i64>(&db, "id").await.unwrap(), 1500);

        // Inside a transaction too.
        let tx = db.begin().await.unwrap();
        let source = orders
            .clone()
            .select(["id", "total"])
            .where_("id", ">", 1500);
        let copied = archive.insert_using(&tx, &[], source).await.unwrap();
        assert_eq!(copied, 500);
        tx.rollback().await.unwrap();
        assert_eq!(archive.count(&db).await.unwrap(), 1490);
    }
}

#[tokio::test]
async fn next_sequence_values_in_one_query() {
    for db in databases().await {
        if db.dialect() == Dialect::MySql {
            let error = db.next_sequence_values("orm_seq", 3).await.unwrap_err();
            assert!(matches!(error, Error::Unsupported { .. }), "{error}");
            continue;
        }
        db.statement(
            "BEGIN EXECUTE IMMEDIATE 'DROP SEQUENCE orm_seq'; \
             EXCEPTION WHEN OTHERS THEN IF SQLCODE != -2289 THEN RAISE; END IF; END;",
            [],
        )
        .await
        .unwrap();
        db.statement("CREATE SEQUENCE orm_seq START WITH 100", [])
            .await
            .unwrap();
        let mut values = db.next_sequence_values("orm_seq", 1200).await.unwrap();
        assert_eq!(values.len(), 1200);
        values.sort_unstable();
        values.dedup();
        assert_eq!(values.len(), 1200, "values are unique");
        assert!(values[0] >= 100);
        assert!(
            db.next_sequence_values("orm_seq", 0)
                .await
                .unwrap()
                .is_empty()
        );

        let tx = db.begin().await.unwrap();
        let more = tx.next_sequence_values("orm_seq", 2).await.unwrap();
        assert!(more.iter().all(|v| *v > values[1199]), "{more:?}");
        tx.rollback().await.unwrap();

        let missing = db.next_sequence_values("orm_no_such_seq", 1).await;
        assert!(matches!(missing, Err(Error::Database { .. })));
    }
}

#[tokio::test]
async fn lock_waits_fail_with_a_lock_timeout() {
    for db in databases().await {
        create_table(
            &db,
            "orm_lock_wait",
            &[("name", "VARCHAR(20)", "VARCHAR2(20)")],
        )
        .await;
        let t = QueryBuilder::table("orm_lock_wait");
        t.insert(&db, &["name"], vec![vec!["a".into()]])
            .await
            .unwrap();
        let holder = db.begin().await.unwrap();
        t.clone().lock_for_update().get(&holder).await.unwrap();

        let other = db.begin().await.unwrap();
        let error = t
            .clone()
            .lock_for_update_no_wait()
            .get(&other)
            .await
            .unwrap_err();
        assert!(error.is_lock_timeout(), "{}: {error}", db.dialect());

        let wait = t
            .clone()
            .lock_for_update_wait(Duration::from_secs(1))
            .get(&other)
            .await;
        match db.dialect() {
            Dialect::MySql => assert!(matches!(wait, Err(Error::Unsupported { .. }))),
            _ => {
                let error = wait.unwrap_err();
                assert!(error.is_lock_timeout(), "{error}");
            }
        }
        other.rollback().await.unwrap();
        holder.rollback().await.unwrap();

        // Unlocked, both get the row at once.
        let tx = db.begin().await.unwrap();
        let rows = t.clone().lock_for_update_no_wait().get(&tx).await.unwrap();
        assert_eq!(rows.len(), 1);
        tx.rollback().await.unwrap();
        if db.dialect() == Dialect::Oracle {
            let tx = db.begin().await.unwrap();
            let rows = (t.clone().lock_for_update_wait(Duration::from_millis(10)))
                .get(&tx)
                .await
                .unwrap();
            assert_eq!(rows.len(), 1);
            tx.rollback().await.unwrap();
        }
    }
}

#[tokio::test]
async fn fetch_array_size_reads_every_row() {
    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::builder(&url)
            .fetch_array_size(500)
            .connect()
            .await
            .unwrap();
        let sql = match db.dialect() {
            Dialect::MySql => {
                "WITH RECURSIVE n (i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 987) \
                 SELECT i FROM n"
            }
            _ => "SELECT LEVEL AS i FROM DUAL CONNECT BY LEVEL <= 987",
        };
        assert_eq!(db.select(sql, []).await.unwrap().len(), 987, "{var}");

        let tx = db.begin().await.unwrap();
        assert_eq!(tx.select(sql, []).await.unwrap().len(), 987, "{var}");
        tx.rollback().await.unwrap();

        let compiled = rustique::Compiled {
            sql: sql.to_owned(),
            binds: Vec::new(),
            batch: None,
            returning: 0,
        };
        let mut cursor = db.cursor(compiled);
        let mut rows = 0;
        while cursor.next().await.unwrap().is_some() {
            rows += 1;
        }
        assert_eq!(rows, 987, "{var}");
    }
}
