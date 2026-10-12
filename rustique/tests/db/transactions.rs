use rustique::{Db, Executor, QueryBuilder};

use crate::{create_table, databases};

async fn names(db: &impl rustique::Executor, table: &QueryBuilder) -> Vec<String> {
    let rows = table.clone().order_by("id").get(db).await.unwrap();
    rows.iter().map(|row| row.get("name").unwrap()).collect()
}

async fn setup(db: &Db, table: &str) -> QueryBuilder {
    create_table(db, table, &[("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)")]).await;
    QueryBuilder::table(table)
}

#[tokio::test]
async fn commit_and_rollback() {
    for db in databases().await {
        let table = setup(&db, "orm_tx").await;

        let tx = db.begin().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["discarded".into()]])
            .await
            .unwrap();
        assert_eq!(
            table.count(&tx).await.unwrap(),
            1,
            "visible inside the transaction"
        );
        assert_eq!(table.count(&db).await.unwrap(), 0, "invisible outside it");
        tx.rollback().await.unwrap();
        assert_eq!(table.count(&db).await.unwrap(), 0);

        let tx = db.begin().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["kept".into()]])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert_eq!(names(&db, &table).await, ["kept"]);
    }
}

#[tokio::test]
async fn dropped_transaction_rolls_back() {
    for db in databases().await {
        let table = setup(&db, "orm_tx_drop").await;

        let tx = db.begin().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["abandoned".into()]])
            .await
            .unwrap();
        drop(tx);

        // Autocommit statements on (probably) the same pooled connection must
        // not commit the abandoned insert.
        for _ in 0..5 {
            table
                .clone()
                .where_("name", "=", "nobody")
                .delete(&db)
                .await
                .unwrap();
        }
        assert_eq!(table.count(&db).await.unwrap(), 0);
    }
}

#[tokio::test]
async fn savepoints_roll_back_partially() {
    for db in databases().await {
        let table = setup(&db, "orm_tx_savepoint").await;

        let tx = db.begin().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["a".into()]])
            .await
            .unwrap();

        let savepoint = tx.savepoint().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["b".into()]])
            .await
            .unwrap();
        tx.rollback_to(&savepoint).await.unwrap();

        let savepoint = tx.savepoint().await.unwrap();
        table
            .insert(&tx, &["name"], vec![vec!["c".into()]])
            .await
            .unwrap();
        tx.release(savepoint).await.unwrap();

        tx.commit().await.unwrap();
        assert_eq!(names(&db, &table).await, ["a", "c"]);
    }
}

#[tokio::test]
async fn closure_commits_on_ok_and_rolls_back_on_err() {
    for db in databases().await {
        let table = setup(&db, "orm_tx_closure").await;

        let inserted = db
            .transaction(|tx| {
                let table = table.clone();
                Box::pin(async move {
                    table
                        .insert(tx, &["name"], vec![vec!["kept".into()]])
                        .await?;
                    table.count(tx).await
                })
            })
            .await
            .unwrap();
        assert_eq!(inserted, 1);

        #[derive(Debug)]
        enum AppError {
            #[allow(dead_code)]
            Orm(rustique::Error),
            Refused,
        }
        impl From<rustique::Error> for AppError {
            fn from(e: rustique::Error) -> Self {
                Self::Orm(e)
            }
        }
        let result: Result<(), AppError> = db
            .transaction(|tx| {
                let table = table.clone();
                Box::pin(async move {
                    table
                        .insert(tx, &["name"], vec![vec!["discarded".into()]])
                        .await?;
                    Err(AppError::Refused)
                })
            })
            .await;
        assert!(matches!(result, Err(AppError::Refused)), "{result:?}");
        assert_eq!(names(&db, &table).await, ["kept"]);

        // The future is `Send`, so it can run on a spawned task.
        let task_db = db.clone();
        tokio::spawn(async move {
            task_db
                .transaction(|tx| {
                    Box::pin(async move { tx.select("SELECT 1 FROM dual", []).await })
                })
                .await
        })
        .await
        .unwrap()
        .unwrap();
    }
}
