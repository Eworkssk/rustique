//! Integration tests against real databases (see `docker-compose.yml`).
//!
//! Each test runs on every database whose URL is set and passes trivially when
//! none is:
//!
//! ```text
//! RUSTIQUE_TEST_MYSQL_URL=mysql://orm:secret@127.0.0.1:13306/orm_test
//! RUSTIQUE_TEST_ORACLE_URL=oracle://orm:secret@127.0.0.1:11521/XEPDB1
//! ```
//!
//! Every test uses its own tables, so tests can run in parallel.

mod casts;
mod cursors;
mod edge_cases;
mod eloquent;
mod extras;
mod factories;
mod hardening;
mod helpers;
mod models;
mod morphs;
mod observers;
mod perf;
mod queries;
mod relation_constraints;
mod relations;
mod schema;
mod support;
mod transactions;
mod values;

use support::{create_table, databases, drop_table};
