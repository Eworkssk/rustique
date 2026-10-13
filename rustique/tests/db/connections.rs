use rustique::{Db, Dialect, Error, Executor};

use crate::eloquent::session;

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
    assert!(error(Db::builder("mysql://localhost/app").timezone("x'y")).contains("time zone"));
}

#[tokio::test]
async fn timezone_holds_on_every_connection() {
    for (var, setup, query) in SETTINGS {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::builder(&url)
            .max_connections(2)
            .timezone("Europe/Bratislava")
            .connect()
            .await
            .unwrap();
        // A `session_setup` runs after `timezone`, so it wins.
        let both = Db::builder(&url)
            .timezone("Europe/Bratislava")
            .session_setup([setup])
            .connect()
            .await
            .unwrap();
        let dialect = db.dialect();
        for _ in 0..3 {
            assert_eq!(
                time_zone(&db, query).await,
                "Europe/Bratislava",
                "{dialect}"
            );
            assert_eq!(time_zone(&both, query).await, "+05:00", "{dialect}: both");
        }
        let tx = db.begin().await.unwrap();
        assert_eq!(
            time_zone(&tx, query).await,
            "Europe/Bratislava",
            "{dialect}: tx"
        );
        tx.commit().await.unwrap();
    }
}

#[tokio::test]
async fn sticky_reads_use_the_primary_after_a_write() {
    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        // One connection per pool, so a pool always answers with the same session.
        let pools = |sticky| {
            Db::builder(&url)
                .max_connections(1)
                .min_connections(1)
                .read_url(&url)
                .sticky(sticky)
                .connect()
        };
        let db = pools(true).await.unwrap();
        let plain = pools(false).await.unwrap();
        let write = match db.dialect() {
            Dialect::MySql => "DO 0",
            _ => "BEGIN NULL; END;",
        };
        let replica = session(&db).await;

        // Outside a scope, a write changes nothing.
        db.statement(write, []).await.unwrap();
        assert_eq!(session(&db).await, replica, "no scope");

        Db::sticky_scope(async {
            assert_eq!(session(&db).await, replica, "before a write");
            plain.statement(write, []).await.unwrap();
            assert_eq!(session(&db).await, replica, "another pool wrote");
            Db::sticky_scope(db.statement(write, [])).await.unwrap(); // nested
            let primary = session(&db.write()).await;
            assert_ne!(primary, replica);
            assert_eq!(session(&db).await, primary, "after a write");
            let plain_replica = session(&plain).await;
            assert_ne!(plain_replica, session(&plain.write()).await, "not sticky");
        })
        .await;

        Db::sticky_scope(async {
            assert_eq!(session(&db).await, replica, "a new scope");
            db.transaction(|_| Box::pin(async { Ok::<_, Error>(()) }))
                .await
                .unwrap();
            let primary = session(&db.write()).await;
            assert_eq!(session(&db).await, primary, "after a transaction");
        })
        .await;
        db.disconnect().await.unwrap();
        plain.disconnect().await.unwrap();
    }
}
