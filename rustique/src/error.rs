//! The crate-wide [`Error`] type.

use crate::{Dialect, ValueError};

/// Result alias with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Everything that can go wrong in the ORM.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The query is malformed: unknown operator, raw SQL bind count mismatch, missing table, ...
    #[error("invalid query: {0}")]
    InvalidQuery(String),

    /// The query is valid but the target database cannot run it.
    #[error("{dialect} does not support {feature}")]
    Unsupported {
        /// Dialect that rejected the query.
        dialect: Dialect,
        /// What the query asked for.
        feature: &'static str,
    },

    /// The connection URL or configuration is invalid, or its driver is not compiled in.
    #[error("configuration error: {0}")]
    Config(String),

    /// The database or its driver reported an error.
    #[error("{dialect} error: {message}")]
    Database {
        /// Database that failed.
        dialect: Dialect,
        /// Native error code when the server sent one: MySQL `1062`, Oracle `1` for ORA-00001, ...
        code: Option<i32>,
        /// Driver message.
        message: String,
    },

    /// `find_or_fail` / `first_or_fail` found no row.
    #[error("no `{0}` row matches the query")]
    NotFound(&'static str),

    /// `sole` found more than one row.
    #[error("more than one `{0}` row matches the query")]
    MultipleRecords(&'static str),

    /// A [`Rel`](crate::Rel) field was read before its relation was loaded.
    #[error("relation not loaded; load it with `with` or `load`")]
    RelationNotLoaded,

    /// The model has no relation with this name.
    #[error("model `{model}` has no relation `{relation}`")]
    UnknownRelation {
        /// Table of the model.
        model: &'static str,
        /// Requested relation name.
        relation: String,
    },

    /// A `morph_to` type column holds a value that no variant of the
    /// [`MorphTo`](crate::MorphTo) enum has as its morph alias.
    #[error("no model has the morph alias `{0}`")]
    UnknownMorphType(String),

    /// The migrations table records a migration the [`Migrator`](crate::Migrator)
    /// does not know, so it cannot be rolled back.
    #[error("migration `{0}` has run but is not registered")]
    MigrationNotFound(String),

    /// A row has no column with the requested name.
    #[error("column `{0}` not found in row")]
    ColumnNotFound(String),

    /// A column value could not be converted into the requested Rust type.
    #[error("cannot decode column `{}`: {source}", qualified(*.model, .column))]
    Decode {
        /// Table of the model being built, when there is one.
        model: Option<&'static str>,
        /// Column that failed to decode.
        column: String,
        /// Why the value did not fit.
        #[source]
        source: ValueError,
    },
}

/// `users.age`, or `age` outside a model.
fn qualified(model: Option<&str>, column: &str) -> String {
    model.map_or_else(|| column.to_owned(), |model| format!("{model}.{column}"))
}

impl Error {
    /// [`Error::Decode`], [reported](Self::report): a value that does not
    /// fit its Rust type means the code and the schema disagree.
    pub(crate) fn decode(model: Option<&'static str>, column: &str, source: ValueError) -> Self {
        let error = Self::Decode {
            model,
            column: column.to_owned(),
            source,
        };
        error.report(model);
        error
    }

    /// Emits this error, which no caller expects (code and schema disagree),
    /// as an `ERROR` event on target `rustique::decode`: no message and an `error`
    /// field, which `tracing-opentelemetry` exports as an OpenTelemetry
    /// exception, with a [cleaned-up](crate::backtrace) backtrace as
    /// `exception.stacktrace`. Only type and column names, never values.
    pub(crate) fn report(&self, model: Option<&'static str>) {
        // Field values, the backtrace included, are only built when the event is enabled.
        tracing::error!(
            target: "rustique::decode",
            error = %self,
            exception.r#type = match self {
                Self::ColumnNotFound(_) => "rustique::Error::ColumnNotFound",
                _ => "rustique::Error::Decode",
            },
            model,
            exception.stacktrace = %crate::backtrace::caller(),
        );
    }

    /// Whether this is a unique-index violation: MySQL 1062, Oracle ORA-00001.
    #[must_use]
    pub fn is_unique_violation(&self) -> bool {
        matches!(
            self,
            Self::Database {
                dialect: Dialect::MySql,
                code: Some(1062),
                ..
            } | Self::Database {
                dialect: Dialect::Oracle,
                code: Some(1),
                ..
            }
        )
    }

    /// Whether a locking read gave up on rows another transaction holds:
    /// [`lock_for_update_no_wait`](crate::QueryBuilder::lock_for_update_no_wait)
    /// found them locked or [`lock_for_update_wait`](crate::QueryBuilder::lock_for_update_wait)
    /// ran out of time. MySQL 3572 (`NOWAIT`) / 1205 (`innodb_lock_wait_timeout`),
    /// Oracle ORA-00054 (`NOWAIT`) / ORA-30006 (`WAIT n`).
    #[must_use]
    pub fn is_lock_timeout(&self) -> bool {
        match self {
            Self::Database {
                dialect: Dialect::MySql,
                code: Some(code),
                ..
            } => matches!(code, 1205 | 3572),
            Self::Database {
                dialect: Dialect::Oracle,
                code: Some(code),
                ..
            } => matches!(code, 54 | 30006),
            _ => false,
        }
    }

    /// Whether this is a deadlock, lock wait timeout or serialization
    /// failure: the database gave up on the transaction because of a
    /// concurrent one, and running it again may succeed. MySQL 1213 / 1205,
    /// Oracle ORA-00060 / ORA-08177.
    #[must_use]
    pub fn is_concurrency_error(&self) -> bool {
        match self {
            Self::Database {
                dialect: Dialect::MySql,
                code: Some(code),
                ..
            } => matches!(code, 1205 | 1213),
            Self::Database {
                dialect: Dialect::Oracle,
                code: Some(code),
                ..
            } => matches!(code, 60 | 8177),
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_timeouts() {
        let error = |dialect, code| Error::Database {
            dialect,
            code: Some(code),
            message: String::new(),
        };
        for (dialect, code) in [
            (Dialect::MySql, 3572),
            (Dialect::MySql, 1205),
            (Dialect::Oracle, 54),
            (Dialect::Oracle, 30006),
        ] {
            assert!(error(dialect, code).is_lock_timeout(), "{dialect} {code}");
        }
        assert!(!error(Dialect::Oracle, 3572).is_lock_timeout());
        assert!(!error(Dialect::MySql, 1213).is_lock_timeout());
    }
}
