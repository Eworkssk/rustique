//! Query logging, in a test binary of its own: `tracing` caches per-callsite
//! interest process-wide, which races with subscribers set per test thread.

use std::io;
use std::sync::{Arc, Mutex};

use rustique::{Db, Executor, Model, Value};
use tracing::Instrument;
use tracing_subscriber::fmt::format::FmtSpan;

#[rustique::model(table = "users")]
#[derive(Debug, Clone, Default)]
struct User {
    id: i64,
}

/// Collects `tracing` output.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl io::Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn statements_are_logged() {
    let capture = Capture::default();
    let writer = capture.clone();
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        // Prints each span's fields when it closes, the recorded row count included.
        .with_span_events(FmtSpan::CLOSE)
        .with_writer(move || writer.clone())
        .init();

    // A value that does not fit its type is reported at ERROR, without the
    // value, with the model and a backtrace down to this caller.
    let row = rustique::Row::new(
        vec!["name".to_owned()].into(),
        vec![Value::from("ann@x.io")],
        rustique::Dialect::MySql,
    );
    let mut user = User::default();
    async {
        row.get::<i64>("name").unwrap_err();
        let error = user.set_value("id", Value::from("ann@x.io")).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("cannot decode column `users.id`"),
            "{error}"
        );
    }
    .instrument(tracing::info_span!("request"))
    .await;
    let log = String::from_utf8(std::mem::take(&mut *capture.0.lock().unwrap())).unwrap();
    assert!(
        log.contains("ERROR request: rustique::decode: error=cannot decode column `name`: expected i64, found string"),
        "{log}"
    );
    assert!(log.contains(r#"model="users""#), "{log}");
    assert!(
        log.contains(r#"exception.type="rustique::Error::Decode""#),
        "{log}"
    );
    assert!(log.contains("statements_are_logged"), "backtrace: {log}");
    assert!(!log.contains("ann@x.io"), "values stay out: {log}");

    for var in ["RUSTIQUE_TEST_MYSQL_URL", "RUSTIQUE_TEST_ORACLE_URL"] {
        let Ok(url) = std::env::var(var) else {
            continue;
        };
        let db = Db::connect(&url).await.unwrap();
        let sql = "SELECT 42 AS answer FROM orm_logging_missing WHERE 1 = ?";
        db.select(sql, [Value::from(1)])
            .instrument(tracing::info_span!("request"))
            .await
            .unwrap_err();
        db.select("SELECT 1 AS one FROM DUAL", []).await.unwrap();
        // Cursors record the rows they streamed, on a pool and in a transaction.
        let three =
            "SELECT 1 AS n FROM DUAL UNION ALL SELECT 2 FROM DUAL UNION ALL SELECT 3 FROM DUAL";
        let compiled = || rustique::Compiled {
            sql: three.to_owned(),
            binds: Vec::new(),
            batch: None,
            returning: 0,
        };
        let mut rows = db.cursor(compiled());
        while rows.next().await.unwrap().is_some() {}
        let tx = db.begin().await.unwrap();
        let mut rows = tx.cursor(compiled());
        while rows.next().await.unwrap().is_some() {}
        drop(rows);
        tx.rollback().await.unwrap();

        let log = String::from_utf8(std::mem::take(&mut *capture.0.lock().unwrap())).unwrap();
        assert!(log.contains("SELECT 42 AS answer"), "{var}: {log}");
        assert!(log.contains("query failed"), "{var}: {log}");
        // The statement's span is a child of the caller's, with OpenTelemetry attributes.
        assert!(log.contains("request:rustique.query{"), "{var}: {log}");
        assert!(log.contains(r#"otel.kind="client""#), "{var}: {log}");
        assert!(
            log.contains(r#"server.address="127.0.0.1""#),
            "{var}: {log}"
        );
        assert!(log.contains("server.port="), "{var}: {log}");
        assert!(log.contains("db.namespace="), "{var}: {log}");
        assert!(log.contains("db.response.returned_rows=1"), "{var}: {log}");
        let cursors = log.matches("db.response.returned_rows=3").count();
        assert_eq!(cursors, 2, "{var}: {log}");
        assert!(log.contains("ROLLBACK"), "{var}: {log}");
        assert!(
            !log.contains("binds"),
            "{var}: bindings only at TRACE: {log}"
        );
    }
}
