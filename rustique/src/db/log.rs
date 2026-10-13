//! Query logging through `tracing`, target `rustique::query`: every statement at
//! `DEBUG` with its duration and row count, its bindings at `TRACE` (they may
//! hold personal data).
//!
//! Every statement also runs in an `INFO` span `rustique.query` carrying
//! OpenTelemetry's database attributes (`db.system`, `db.statement`,
//! `otel.kind`, ...). An application exporting `tracing` spans through
//! `tracing-opentelemetry` gets each statement as a client database span
//! under its current span, the way APM tools such as Elastic expect them.

use std::future::Future;
use std::time::Instant;

use tracing::field::{Empty, display};
use tracing::{Instrument, Level, Span};

use super::Executed;
use crate::grammar::Compiled;
use crate::{Dialect, Error, Result};

/// Row count of a statement's outcome, for the log.
pub(super) trait Outcome {
    fn rows(&self) -> u64;
}

impl<T> Outcome for Vec<T> {
    fn rows(&self) -> u64 {
        self.len() as u64
    }
}

impl Outcome for Executed {
    fn rows(&self) -> u64 {
        self.rows_affected
    }
}

/// The span of one statement, a child of the caller's current span.
fn span(dialect: Dialect, sql: &str) -> Span {
    tracing::info_span!(
        target: "rustique::query",
        "rustique.query",
        otel.name = sql.split_whitespace().next().unwrap_or_default(),
        otel.kind = "client",
        otel.status_code = Empty,
        otel.status_message = Empty,
        db.system = match dialect {
            Dialect::MySql => "mysql",
            Dialect::Oracle => "oracle",
        },
        db.statement = sql,
    )
}

/// Runs `run(query)` in its span and logs it.
pub(super) async fn logged<T: Outcome, F: Future<Output = Result<T>>>(
    dialect: Dialect,
    query: Compiled,
    run: impl FnOnce(Compiled) -> F,
) -> Result<T> {
    traced(span(dialect, &query.sql), timed(query, run)).await
}

/// Runs `statement` in `span`, marking the span failed if it fails.
pub(super) async fn traced<T>(span: Span, statement: impl Future<Output = Result<T>>) -> Result<T> {
    let result = statement.instrument(span.clone()).await;
    if let Err(error) = &result {
        fail(&span, error);
    }
    result
}

/// Marks a statement's span failed, for the exported status.
pub(super) fn fail(span: &Span, error: &Error) {
    span.record("otel.status_code", "error");
    span.record("otel.status_message", display(status_message(error)));
}

/// Error text for the exported span. Database messages can quote row data
/// (`Duplicate entry 'ann@x.io'`), so only their code leaves the process;
/// the full message stays in the `DEBUG` event.
fn status_message(error: &Error) -> String {
    match error {
        Error::Database {
            dialect,
            code: Some(code),
            ..
        } => format!("{dialect} error {code}"),
        Error::Database { dialect, .. } => format!("{dialect} error"),
        other => other.to_string(),
    }
}

async fn timed<T: Outcome, F: Future<Output = Result<T>>>(
    query: Compiled,
    run: impl FnOnce(Compiled) -> F,
) -> Result<T> {
    // `level_enabled!`, not `enabled!`: a callsite's cached interest can miss a
    // subscriber set later with `set_default`.
    if !tracing::level_enabled!(Level::DEBUG) {
        return run(query).await;
    }
    let sql = query.sql.clone();
    tracing::trace!(target: "rustique::query", sql, binds = ?query.binds);
    let start = Instant::now();
    let result = run(query).await;
    let elapsed = start.elapsed();
    match &result {
        Ok(outcome) => {
            tracing::debug!(target: "rustique::query", sql, ?elapsed, rows = outcome.rows())
        }
        Err(error) => {
            tracing::debug!(target: "rustique::query", sql, ?elapsed, %error, "query failed")
        }
    }
    result
}

/// Logs a statement without bindings or rows (`COMMIT`, a cursor's query, ...)
/// and returns its span, to instrument the statement with.
pub(super) fn statement(dialect: Dialect, sql: &str) -> Span {
    let span = span(dialect, sql);
    span.in_scope(|| tracing::debug!(target: "rustique::query", sql));
    span
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exported_status_leaves_database_messages_out() {
        let error = Error::Database {
            dialect: Dialect::MySql,
            code: Some(1062),
            message: "Duplicate entry 'ann@x.io' for key 'email'".to_owned(),
        };
        assert_eq!(status_message(&error), "MySQL error 1062");
    }
}
