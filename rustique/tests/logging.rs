//! Query logging, in a test binary of its own: `tracing` caches per-callsite
//! interest process-wide, which races with subscribers set per test thread.

use std::io;
use std::sync::{Arc, Mutex};

use rustique::{Db, Executor, Model, Value};
use tracing::Instrument;

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
        let tx = db.begin().await.unwrap();
        tx.rollback().await.unwrap();

        let log = String::from_utf8(std::mem::take(&mut *capture.0.lock().unwrap())).unwrap();
        assert!(log.contains("SELECT 42 AS answer"), "{var}: {log}");
        assert!(log.contains("query failed"), "{var}: {log}");
        // The statement's span is a child of the caller's, with OpenTelemetry attributes.
        assert!(log.contains("request:rustique.query{"), "{var}: {log}");
        assert!(log.contains(r#"otel.kind="client""#), "{var}: {log}");
        assert!(log.contains("ROLLBACK"), "{var}: {log}");
        assert!(
            !log.contains("binds"),
            "{var}: bindings only at TRACE: {log}"
        );
    }
}
