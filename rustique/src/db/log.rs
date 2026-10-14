//! Query logging through `tracing`, target `rustique::query`: every statement at
//! `DEBUG` with its duration and row count, its bindings at `TRACE` (they may
//! hold personal data).
//!
//! Every statement also runs in an `INFO` span `rustique.query` carrying
//! OpenTelemetry's database attributes (`db.system`, `db.statement`,
//! `db.namespace`, `server.address`, `otel.kind`, ...). An application exporting `tracing` spans through
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
    /// Span field the count goes into.
    const FIELD: &'static str;
    fn rows(&self) -> u64;
}

impl<T> Outcome for Vec<T> {
    const FIELD: &'static str = "db.response.returned_rows";
    fn rows(&self) -> u64 {
        self.len() as u64
    }
}

impl Outcome for Executed {
    // OpenTelemetry has no attribute for it.
    const FIELD: &'static str = "rustique.rows_affected";
    fn rows(&self) -> u64 {
        self.rows_affected
    }
}

/// The database a pool's statements go to, for their spans.
#[derive(Debug)]
pub(super) struct Peer {
    pub dialect: Dialect,
    /// `db.namespace`: MySQL database, Oracle service name.
    pub namespace: Option<String>,
    /// `server.address`: host name or IP.
    pub address: Option<String>,
    /// `server.port`.
    pub port: Option<u16>,
}

/// The span of one statement, a child of the caller's current span. `batch`:
/// rows of an array-bound statement ([`Compiled::batch`]).
fn span(peer: &Peer, sql: &str, batch: Option<usize>) -> Span {
    tracing::info_span!(
        target: "rustique::query",
        "rustique.query",
        otel.name = sql.split_whitespace().next().unwrap_or_default(),
        otel.kind = "client",
        otel.status_code = Empty,
        otel.status_message = Empty,
        db.system = match peer.dialect {
            Dialect::MySql => "mysql",
            Dialect::Oracle => "oracle",
        },
        db.statement = sql,
        db.namespace = peer.namespace.as_deref(),
        server.address = peer.address.as_deref(),
        server.port = peer.port,
        // OpenTelemetry: only for batches of 2 or more.
        db.operation.batch.size = batch.filter(|&rows| rows > 1).map(|rows| rows as u64),
        db.response.returned_rows = Empty,
        rustique.rows_affected = Empty,
    )
}

/// Runs `run(query)` in its span and logs it.
pub(super) async fn logged<T: Outcome, F: Future<Output = Result<T>>>(
    peer: &Peer,
    query: Compiled,
    run: impl FnOnce(Compiled) -> F,
) -> Result<T> {
    let span = span(peer, &query.sql, query.batch);
    let result = traced(span.clone(), timed(query, run)).await;
    if let Ok(outcome) = &result {
        span.record(T::FIELD, outcome.rows());
    }
    result
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
pub(super) fn statement(peer: &Peer, sql: &str) -> Span {
    let span = span(peer, sql, None);
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
