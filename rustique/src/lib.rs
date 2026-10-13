//! Eloquent-style ORM for MySQL 8.4+ and Oracle 19c+.
//!
//! Source layout, one concern per module:
//!
//! - `value/`: [`Value`], conversions from Rust types (`From`) and back ([`FromValue`]).
//! - `row.rs`: [`Row`], one result row with case-insensitive column lookup.
//! - `collection.rs`: [`Collection`], the `Vec` that multi-row queries return.
//! - `query/`: [`QueryBuilder`], the fluent builder, and the syntax tree it produces.
//! - `grammar/`: compiles a query into SQL for a [`Dialect`]; per-database quirks
//!   live in `grammar/mysql.rs` and `grammar/oracle.rs`.
//! - `model/`: the [`Model`] trait behind `#[rustique::model]`, and [`Query`],
//!   a query that returns models.
//! - `db/`: [`Db`] (connection pool), [`Transaction`], the [`Executor`] trait,
//!   [`Cursor`], query logging and the MySQL / Oracle drivers.
//! - `schema.rs`: [`Schema`] and [`Blueprint`], the schema builder.
//! - `migrate.rs`: [`Migrator`] and the [`migrations!`] macro.
//! - `cli.rs`: [`cli::run`], `migrate` / `make:migration` commands for the
//!   application's own binary.
//! - `timezone.rs`: [`set_timezone`], the application time zone.
//! - `error.rs`: the crate-wide [`Error`]; `backtrace.rs` trims backtraces for its reports.
//!
//! # Examples
//!
//! ```
//! use rustique::prelude::*;
//!
//! let query = QueryBuilder::table("users")
//!     .where_("email", "like", "%@example.com")
//!     .order_by_desc("id")
//!     .limit(10);
//!
//! let compiled = query.to_sql(Dialect::Oracle)?;
//! assert_eq!(
//!     compiled.sql,
//!     r#"SELECT * FROM "USERS" WHERE "EMAIL" LIKE :1 ORDER BY "ID" DESC FETCH NEXT 10 ROWS ONLY"#,
//! );
//! # Ok::<(), rustique::Error>(())
//! ```

#[cfg(not(any(feature = "mysql", feature = "oracle")))]
compile_error!("enable the `mysql` and/or `oracle` feature");

pub mod cli;

/// Compiles the snippets of `docs/` as doctests, so the docs cannot drift from the API.
#[cfg(doctest)]
mod docs {
    #[doc = include_str!("../docs/README.md")]
    mod README {}
    #[doc = include_str!("../docs/getting-started.md")]
    mod getting_started {}
    #[doc = include_str!("../docs/query-builder.md")]
    mod query_builder {}
    #[doc = include_str!("../docs/models.md")]
    mod models {}
    #[doc = include_str!("../docs/relationships.md")]
    mod relationships {}
    #[doc = include_str!("../docs/casts.md")]
    mod casts {}
    #[doc = include_str!("../docs/events.md")]
    mod events {}
    #[doc = include_str!("../docs/large-results.md")]
    mod large_results {}
    #[doc = include_str!("../docs/database.md")]
    mod database {}
    #[doc = include_str!("../docs/migrations.md")]
    mod migrations {}
    #[doc = include_str!("../docs/factories.md")]
    mod factories {}
    #[doc = include_str!("../docs/dialects.md")]
    mod dialects {}
}
mod backtrace;
mod collection;
mod db;
mod error;
mod grammar;
mod migrate;
mod model;
mod query;
mod row;
mod schema;
mod timezone;
mod value;

pub use collection::Collection;
#[cfg(feature = "mysql-rustls")]
pub use db::Tls;
pub use db::{
    AnyExecutor, Cursor, Db, DbBuilder, Executed, Executor, Savepoint, Transaction,
    TransactionError,
};
pub use error::{Error, Result};
pub use grammar::{Compiled, Dialect};
pub use migrate::{MIGRATIONS_TABLE, Migration, MigrationFn, MigrationStatus, Migrator};
pub use model::{
    BoxFuture, CREATED_AT, DELETED_AT, Factory, FromRelated, GlobalScope, HasFactory, Hooks,
    LazyLoading, Meta, Model, ModelChunks, ModelCursor, MorphTo, Observer, Pivot, Prunable, Query,
    Rel, UPDATED_AT, prevent_lazy_loading,
};
pub use query::{Chunks, CursorPage, JoinClause, Page, QueryBuilder, SimplePage};
pub use row::Row;
pub use rustique_macros::{Cast, MorphTo, model, scopes};
pub use schema::{Blueprint, Column, ForeignKey, Index, Schema};
pub use timezone::{now, set_timezone};
pub use value::{FromValue, Value, ValueError};

/// Used by code that `#[rustique::model]` and `#[derive(rustique::Cast)]` generate; not a stable API.
#[doc(hidden)]
pub mod __private {
    pub use crate::model::{
        Event, MorphTarget, Nested, OfMany, Relation, RelationKind, RelationScope, Through, load,
        load_morph, morph_models, morph_to, new_ulid, new_uuid, observe, query, scope,
    };
    pub use crate::value::parse_json;
    pub use {serde, serde_json};

    /// `Model::set_value`'s error for a value that does not fit its field.
    pub fn decode_error(
        model: &'static str,
        column: &str,
        source: crate::ValueError,
    ) -> crate::Error {
        crate::Error::decode(Some(model), column, source)
    }

    /// `deserialize_with` of `{Model}Fill` fields: a present key is `Some`,
    /// even when `null`, so `null` can clear a nullable column.
    pub fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        T: serde::Deserialize<'de>,
        D: serde::Deserializer<'de>,
    {
        T::deserialize(deserializer).map(Some)
    }
}

/// Common imports: `use rustique::prelude::*;`.
pub mod prelude {
    pub use crate::{
        Collection, Db, Dialect, Executor, FromValue, HasFactory, Model, QueryBuilder, Rel, Schema,
        Value,
    };
    pub use itertools::Itertools;
}
