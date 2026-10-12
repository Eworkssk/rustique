//! SQL generation tests. Every query is compiled for both dialects; no database needed.

mod casts;
mod dml;
mod edge_cases;
mod eloquent;
mod extras;
mod factories;
mod helpers;
mod joins;
mod models;
mod relation_constraints;
mod relations;
mod schema;
mod select;
mod wheres;

use rustique::{Dialect, QueryBuilder, Value};

/// Compiles `query` for MySQL and Oracle and compares the SQL text.
#[track_caller]
fn assert_sql(query: &QueryBuilder, mysql: &str, oracle: &str) {
    assert_eq!(query.to_sql(Dialect::MySql).unwrap().sql, mysql, "MySQL");
    assert_eq!(query.to_sql(Dialect::Oracle).unwrap().sql, oracle, "Oracle");
}

/// Bindings are identical for both dialects; only placeholders differ.
#[track_caller]
fn binds(query: &QueryBuilder) -> Vec<Value> {
    let mysql = query.to_sql(Dialect::MySql).unwrap().binds;
    assert_eq!(query.to_sql(Dialect::Oracle).unwrap().binds, mysql);
    mysql
}

#[track_caller]
fn error(query: &QueryBuilder, dialect: Dialect) -> String {
    query.to_sql(dialect).unwrap_err().to_string()
}
