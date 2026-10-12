use rustique::{Error, Model, QueryBuilder, Value};

use crate::{create_table, databases};

#[rustique::model(table = "orm_cursor_items")]
#[derive(Debug, Clone, Default)]
struct Item {
    id: i64,
    name: String,
}

/// More rows than a cursor reads ahead.
const ROWS: i64 = 250;

#[tokio::test]
async fn cursors_stream_rows_and_models() {
    for db in databases().await {
        let table = "orm_cursor_items";
        create_table(&db, table, &[("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)")]).await;
        let rows = (1..=ROWS)
            .map(|i| vec![Value::from(format!("n{i}"))])
            .collect();
        let query = QueryBuilder::table(table);
        query.insert(&db, &["name"], rows).await.unwrap();

        let mut cursor = query.clone().order_by("id").cursor(&db).unwrap();
        let mut ids = Vec::new();
        while let Some(row) = cursor.next().await.unwrap() {
            ids.push(row.get::<i64>("id").unwrap());
        }
        assert_eq!(ids, (1..=ROWS).collect::<Vec<_>>());
        assert!(cursor.next().await.unwrap().is_none(), "stays finished");

        // Any read-ahead, even 0 (taken as 1), yields every row.
        for buffer in [0, 1, 1000] {
            let mut cursor = query.clone().cursor_buffered(&db, buffer).unwrap();
            let mut count = 0;
            while cursor.next().await.unwrap().is_some() {
                count += 1;
            }
            assert_eq!(count, ROWS);
        }
        let mut items = Item::query().cursor_buffered(&db, 7).unwrap();
        let mut count = 0;
        while items.next().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, ROWS);

        // Stopping early leaves the pool usable.
        let mut cursor = query.clone().order_by("id").cursor(&db).unwrap();
        cursor.next().await.unwrap().unwrap();
        drop(cursor);
        assert_eq!(query.count(&db).await.unwrap(), ROWS as u64);

        let mut items = Item::query()
            .where_("id", "<=", 3)
            .order_by_desc("id")
            .cursor(&db)
            .unwrap();
        let mut names = Vec::new();
        while let Some(item) = items.next().await.unwrap() {
            assert!(item.exists());
            names.push(item.name);
        }
        assert_eq!(names, ["n3", "n2", "n1"]);

        let eager = Item::query().with("anything").cursor(&db);
        assert!(matches!(eager, Err(Error::InvalidQuery(_))));

        let mut missing = QueryBuilder::table("orm_cursor_missing")
            .cursor(&db)
            .unwrap();
        assert!(matches!(missing.next().await, Err(Error::Database { .. })));
        assert!(missing.next().await.unwrap().is_none());
    }
}

#[tokio::test]
async fn cursor_inside_transaction() {
    for db in databases().await {
        let table = "orm_cursor_tx";
        create_table(&db, table, &[("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)")]).await;
        let query = QueryBuilder::table(table);
        let tx = db.begin().await.unwrap();
        let rows = (1..=ROWS)
            .map(|i| vec![Value::from(format!("n{i}"))])
            .collect();
        query.insert(&tx, &["name"], rows).await.unwrap();

        // Sees the uncommitted rows; an abandoned cursor frees the connection.
        let mut cursor = query.clone().order_by("id").cursor(&tx).unwrap();
        let first = cursor.next().await.unwrap().unwrap();
        assert_eq!(first.get::<String>("name").unwrap(), "n1");
        drop(cursor);

        query
            .clone()
            .where_("id", ">", 10)
            .delete(&tx)
            .await
            .unwrap();
        let mut cursor = query.cursor(&tx).unwrap();
        let mut count = 0;
        while cursor.next().await.unwrap().is_some() {
            count += 1;
        }
        assert_eq!(count, 10);
        drop(cursor); // it borrows `tx`
        tx.commit().await.unwrap();
        assert_eq!(query.count(&db).await.unwrap(), 10);
    }
}

#[tokio::test]
async fn open_cursor_makes_transaction_statements_fail_fast() {
    for db in databases().await {
        let table = "orm_cursor_busy";
        create_table(&db, table, &[("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)")]).await;
        let query = QueryBuilder::table(table);
        let tx = db.begin().await.unwrap();
        // More rows than the cursor reads ahead, so waiting would deadlock.
        let rows = (0..500)
            .map(|i| vec![Value::from(format!("n{i}"))])
            .collect();
        query.insert(&tx, &["name"], rows).await.unwrap();

        let mut cursor = query.clone().cursor(&tx).unwrap();
        cursor.next().await.unwrap().unwrap();
        let busy = tokio::time::timeout(std::time::Duration::from_secs(5), query.count(&tx))
            .await
            .expect("fails instead of waiting");
        assert!(matches!(busy, Err(Error::InvalidQuery(_))), "{busy:?}");
        let mut second = query.clone().cursor(&tx).unwrap();
        assert!(matches!(second.next().await, Err(Error::InvalidQuery(_))));
        drop(second); // must not clear the first cursor's claim
        assert!(query.count(&tx).await.is_err());
        drop(cursor);
        assert_eq!(query.count(&tx).await.unwrap(), 500);

        // A cursor read to the end frees the transaction without a drop.
        let mut cursor = query.clone().cursor(&tx).unwrap();
        while cursor.next().await.unwrap().is_some() {}
        assert_eq!(query.count(&tx).await.unwrap(), 500);
        drop(cursor);
        tx.rollback().await.unwrap();
    }
}
