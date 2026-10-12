use rustique::{Db, Dialect, Error, Executor};

/// `(env var, session setup, query of the setting)` per database.
const SETTINGS: [(&str, &str, &str); 2] = [
    (
        "RUSTIQUE_TEST_MYSQL_URL",
        "SET SESSION time_zone = '+05:00'",
        "SELECT @@session.time_zone AS tz",
    ),
    (
        "RUSTIQUE_TEST_ORACLE_URL",
        "ALTER SESSION SET TIME_ZONE = '+05:00'",
        "SELECT SESSIONTIMEZONE AS tz FROM dual",
    ),
];

async fn time_zone(db: &impl Executor, sql: &str) -> String {
    let rows = db.select(sql, []).await.unwrap();
    rows[0].get("tz").unwrap()
}

#[tokio::test]
async fn session_setup_holds_on_every_connection() {
    for (var, setup, query) in SETTINGS {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::builder(&url)
            .max_connections(2)
            .session_setup([setup])
            .connect()
            .await
            .unwrap();
        let dialect = db.dialect();

        assert_eq!(time_zone(&db, query).await, "+05:00", "{dialect}: new");
        // MySQL resets a connection after raw SQL, and the setup runs again.
        assert_eq!(time_zone(&db, query).await, "+05:00", "{dialect}: reset");

        let tx = db.begin().await.unwrap();
        assert_eq!(
            time_zone(&tx, query).await,
            "+05:00",
            "{dialect}: transaction"
        );
        tx.commit().await.unwrap();

        // Overwritten by raw SQL: MySQL resets that connection, Oracle replaces the session.
        let overwrite = match dialect {
            Dialect::MySql => "SET SESSION time_zone = '+00:00'",
            _ => "ALTER SESSION SET TIME_ZONE = '+00:00'",
        };
        db.statement(overwrite, []).await.unwrap();
        for _ in 0..3 {
            assert_eq!(
                time_zone(&db, query).await,
                "+05:00",
                "{dialect}: after raw SET"
            );
        }
    }
}

#[tokio::test]
async fn failing_session_setup_fails_the_query() {
    for (var, _, query) in SETTINGS {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::builder(&url)
            .session_setup(["SET SESSION no_such_setting = 1"])
            .connect_lazy()
            .unwrap();
        let error = db.select(query, []).await.unwrap_err();
        assert!(matches!(error, Error::Database { .. }), "{error}");
    }
}

#[tokio::test]
async fn connect_lazy_connects_on_first_query() {
    for url in [
        "mysql://user:secret@127.0.0.1:1/app",
        "oracle://user:secret@127.0.0.1:1/app",
    ] {
        let db = Db::builder(url).connect_lazy().unwrap();
        let error = db.select("SELECT 1 AS n FROM dual", []).await.unwrap_err();
        assert!(matches!(error, Error::Database { .. }), "{url}: {error}");
        assert!(Db::connect(url).await.is_err(), "{url}");
    }
}

#[test]
fn connect_lazy_checks_the_url_and_pool_sizes() {
    let error = |builder: rustique::DbBuilder| builder.connect_lazy().unwrap_err().to_string();

    assert!(error(Db::builder("postgres://localhost/app")).contains("unsupported URL scheme"));
    assert!(error(Db::builder("mysql://localhost/app").max_connections(0)).contains("at least 1"));
    assert!(
        error(Db::builder("oracle://user:secret@localhost/app").min_connections(11))
            .contains("exceeds")
    );
}
