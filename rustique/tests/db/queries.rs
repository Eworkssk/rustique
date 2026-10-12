use rustique::{Db, Dialect, Error, Executor, QueryBuilder, Value};

use crate::{create_table, databases};

#[tokio::test]
async fn query_builder_crud() {
    for db in databases().await {
        create_table(
            &db,
            "orm_users",
            &[
                ("name", "VARCHAR(50)", "VARCHAR2(50 CHAR)"),
                ("age", "INT", "NUMBER(10)"),
            ],
        )
        .await;
        let users = QueryBuilder::table("orm_users");

        let rows = vec![
            vec!["Ann".into(), 30.into()],
            vec!["Bo".into(), 20.into()],
            vec!["Cy".into(), 40.into()],
        ];
        let inserted = users.insert(&db, &["name", "age"], rows).await.unwrap();
        assert_eq!(inserted.rows_affected, 3);

        // Multi-row insert must give every row its own id (Oracle uses INSERT ALL).
        let ids: Vec<i64> = users
            .clone()
            .order_by("id")
            .get(&db)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get("id").unwrap())
            .collect();
        assert_eq!(ids.len(), 3);
        assert!(
            ids.windows(2).all(|pair| pair[0] < pair[1]),
            "ids not unique: {ids:?}"
        );

        let adults: Vec<String> = users
            .clone()
            .where_("age", ">=", 30)
            .order_by("name")
            .get(&db)
            .await
            .unwrap()
            .iter()
            .map(|row| row.get("name").unwrap())
            .collect();
        assert_eq!(adults, ["Ann", "Cy"]);

        assert_eq!(users.count(&db).await.unwrap(), 3);
        assert_eq!(users.clone().limit(2).count(&db).await.unwrap(), 2);
        assert_eq!(users.max::<i64>(&db, "age").await.unwrap(), 40);
        assert_eq!(users.min::<i64>(&db, "age").await.unwrap(), 20);
        assert_eq!(users.sum::<i64>(&db, "age").await.unwrap(), 90);
        assert_eq!(users.avg::<f64>(&db, "age").await.unwrap(), 30.0);
        let nobody = users.clone().where_("age", ">", 100);
        assert_eq!(nobody.max::<Option<i64>>(&db, "age").await.unwrap(), None);
        assert!(users.exists(&db).await.unwrap());
        assert!(!nobody.exists(&db).await.unwrap());

        let bo = users.clone().where_("name", "=", "Bo");
        assert_eq!(bo.update(&db, vec![("age", 21.into())]).await.unwrap(), 1);
        let bo_row = bo.first(&db).await.unwrap().expect("Bo exists");
        assert_eq!(bo_row.get::<i64>("age").unwrap(), 21);

        assert_eq!(
            users
                .clone()
                .where_("age", ">", 35)
                .delete(&db)
                .await
                .unwrap(),
            1
        );
        assert_eq!(users.count(&db).await.unwrap(), 2);
    }
}

#[tokio::test]
async fn in_lists_longer_than_oracle_limit() {
    for db in databases().await {
        create_table(&db, "orm_in", &[("n", "INT", "NUMBER(10)")]).await;
        let table = QueryBuilder::table("orm_in");
        table
            .insert(&db, &["n"], vec![vec![7.into()]])
            .await
            .unwrap();

        let matching = table
            .clone()
            .where_in("n", 1..=1500)
            .count(&db)
            .await
            .unwrap();
        assert_eq!(matching, 1);
        let excluded = table
            .clone()
            .where_not_in("n", 1..=1500)
            .count(&db)
            .await
            .unwrap();
        assert_eq!(excluded, 0);
    }
}

#[tokio::test]
async fn duplicate_key_reports_native_error_code() {
    for db in databases().await {
        create_table(
            &db,
            "orm_dupes",
            &[("code", "VARCHAR(10) UNIQUE", "VARCHAR2(10) UNIQUE")],
        )
        .await;
        let table = QueryBuilder::table("orm_dupes");
        table
            .insert(&db, &["code"], vec![vec!["x".into()]])
            .await
            .unwrap();

        let error = table
            .insert(&db, &["code"], vec![vec!["x".into()]])
            .await
            .unwrap_err();
        let expected = if db.dialect() == Dialect::MySql {
            1062
        } else {
            1
        }; // ORA-00001
        assert!(
            matches!(error, Error::Database { code: Some(code), .. } if code == expected),
            "{error:?}"
        );
    }
}

/// Compile-time check: query futures can be spawned on a multi-threaded runtime.
#[allow(dead_code)]
fn futures_are_send(db: &Db) {
    fn assert_send<T: Send>(_: T) {}
    let query = QueryBuilder::table("t");
    assert_send(query.get(db));
    assert_send(query.count(db));
    assert_send(query.update(db, vec![("a", Value::Null)]));
    assert_send(db.select("SELECT 1 FROM DUAL", []));
    assert_send(db.begin());
}

// The only test that sets the process-wide pool.
#[tokio::test]
async fn global_db() {
    let Some(db) = databases().await.into_iter().next() else {
        return;
    };
    let dual = if db.dialect() == Dialect::MySql {
        ""
    } else {
        " FROM DUAL"
    };
    db.clone().set_global().unwrap();
    assert!(matches!(db.set_global(), Err(Error::Config(_))));

    let rows = Db::global()
        .select(&format!("SELECT ? AS n{dual}"), [7.into()])
        .await
        .unwrap();
    assert_eq!(rows[0].get::<i64>("n").unwrap(), 7);
}
