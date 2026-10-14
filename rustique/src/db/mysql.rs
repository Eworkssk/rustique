//! MySQL driver glue on top of `mysql_async`.
//!
//! Statements always use the binary protocol (prepared statements, cached per
//! connection), so values arrive typed instead of as text.

use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, NaiveDate, NaiveDateTime, NaiveTime, Timelike};
#[cfg(feature = "mysql-rustls")]
use mysql_async::ClientIdentity;
use mysql_async::consts::ColumnType;
use mysql_async::prelude::Queryable;
use mysql_async::{Column, Opts, OptsBuilder, Params, PoolConstraints, Value as MyValue};
use tokio::sync::Mutex;

use super::Executed;
use super::cursor::{Cursor, Sink};
use super::log::Peer;
use super::options::{ACQUIRE_TIMEOUT, PoolConfig, acquire_timed_out};
use crate::grammar::Compiled;
use crate::{Dialect, Error, Result, Row, Value, ValueError};

/// MySQL's id for the `binary` character set: BLOB / BINARY / VARBINARY columns.
const BINARY_CHARSET: u16 = 63;

/// Connection pool plus how long a query waits for one of its connections. Cheap to clone.
#[derive(Clone)]
pub(super) struct Pool {
    pool: mysql_async::Pool,
    acquire_timeout: Duration,
    /// See `DbBuilder::keep_sessions_after_raw_sql`.
    keep_sessions: bool,
    pub peer: Arc<Peer>,
}

/// The pool for `url`; `mysql_async` connects on first use.
pub(super) fn pool(url: &str, config: PoolConfig) -> Result<Pool> {
    let mut opts = Opts::from_url(url).map_err(|e| Error::Config(e.to_string()))?;
    #[cfg(feature = "mysql-rustls")]
    if let Some(tls) = config.tls {
        // Starts from the URL's TLS flags (`require_ssl=true&verify_ca=false`, ...), if any.
        let mut ssl = opts
            .ssl_opts()
            .cloned()
            .unwrap_or_default()
            .with_root_certs(tls.root_certs.into_iter().map(Into::into).collect());
        if tls.accept_invalid_certs {
            ssl = ssl.with_danger_accept_invalid_certs(true);
        }
        if let Some((cert, key)) = tls.client_identity {
            ssl = ssl.with_client_identity(Some(ClientIdentity::new(cert.into(), key.into())));
        }
        opts = OptsBuilder::from_opts(opts).ssl_opts(ssl).into();
    }
    // Without a TLS backend, `mysql_async` panics on `require_ssl`.
    #[cfg(not(feature = "mysql-rustls"))]
    if opts.ssl_opts().is_some() {
        return Err(Error::Config(
            "MySQL TLS (`require_ssl`) needs the `mysql-tls` feature".to_owned(),
        ));
    }

    let mut pool_opts = opts.pool_opts().clone();
    // Resetting a connection on its return to the pool also drops its cached
    // prepared statements, so every query would prepare again. Transactions
    // and raw SQL still reset (see `begin`, `checkout_for`);
    // `?reset_connection=true` restores it for all.
    if !url
        .split_once('?')
        .is_some_and(|(_, query)| query.contains("reset_connection="))
    {
        pool_opts = pool_opts.with_reset_connection(false);
    }
    if config.min_connections.is_some() || config.max_connections.is_some() {
        let current = pool_opts.constraints();
        let max = config.max_connections.map_or(current.max(), |n| n as usize);
        let min = config
            .min_connections
            .map_or(current.min().min(max), |n| n as usize);
        let constraints = PoolConstraints::new(min, max).ok_or_else(|| {
            Error::Config(format!(
                "min_connections ({min}) exceeds max_connections ({max})"
            ))
        })?;
        pool_opts = pool_opts.with_constraints(constraints);
    }
    if let Some(ttl) = config.idle_timeout {
        pool_opts = pool_opts.with_inactive_connection_ttl(ttl);
    }
    if let Some(ttl) = config.max_lifetime {
        pool_opts = pool_opts.with_abs_conn_ttl(Some(ttl));
    }
    opts = OptsBuilder::from_opts(opts)
        .pool_opts(pool_opts)
        // Also runs after each reset; `mysql_async` runs the list from the end.
        .setup(config.session_setup.into_iter().rev().collect())
        .into();

    let peer = Arc::new(Peer {
        dialect: Dialect::MySql,
        namespace: opts.db_name().map(str::to_owned),
        address: Some(opts.ip_or_hostname().to_owned()),
        port: Some(opts.tcp_port()),
    });
    Ok(Pool {
        peer,
        pool: mysql_async::Pool::new(opts),
        acquire_timeout: config.acquire_timeout.unwrap_or(ACQUIRE_TIMEOUT),
        keep_sessions: config.keep_sessions,
    })
}

pub(super) async fn checkout(pool: &Pool) -> Result<mysql_async::Conn> {
    match tokio::time::timeout(pool.acquire_timeout, pool.pool.get_conn()).await {
        Ok(conn) => conn.map_err(error),
        Err(_) => Err(acquire_timed_out(Dialect::MySql, pool.acquire_timeout)),
    }
}

/// A connection for one statement. Raw SQL (`Db::select`, `Db::statement`)
/// may change session state (`SET`, temporary tables, locks) that would leak
/// into later queries, so its connection is reset on return to the pool,
/// unless the pool keeps sessions (`DbBuilder::keep_sessions_after_raw_sql`).
pub(super) async fn checkout_for(pool: &Pool, raw: bool) -> Result<KillOnDrop> {
    let mut conn = checkout(pool).await?;
    if raw && !pool.keep_sessions {
        conn.reset_connection(true);
    }
    Ok(KillOnDrop(Some(conn)))
}

/// A pooled connection running one statement. Dropped before the statement
/// finished (its future was dropped), it has the server stop that statement
/// with `KILL QUERY` from a connection of its own, and only then goes back to
/// the pool, so the kill cannot reach the connection's next user. There,
/// `mysql_async` discards the rest of the result (ER_QUERY_INTERRUPTED) or,
/// if the reply was cut mid-packet, the connection.
pub(super) struct KillOnDrop(Option<mysql_async::Conn>);

impl KillOnDrop {
    pub async fn fetch_all<T>(
        mut self,
        query: Compiled,
        f: impl FnMut(Row) -> Result<T>,
    ) -> Result<Vec<T>> {
        let rows = fetch_all(self.conn(), query, f).await;
        drop(self.0.take()); // finished: back to the pool as it is
        rows
    }

    pub async fn execute(mut self, query: Compiled) -> Result<Executed> {
        let executed = execute(self.conn(), query).await;
        drop(self.0.take());
        executed
    }

    fn conn(&mut self) -> &mut mysql_async::Conn {
        self.0
            .as_mut()
            .expect("taken only once the statement is done")
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let Some(conn) = self.0.take() else {
            return;
        };
        // Without a runtime the statement runs on, as before.
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let (opts, id) = (conn.opts().clone(), conn.id());
        runtime.spawn(async move {
            // A new connection, not one from the pool: the pool may be full.
            if let Ok(mut killer) = mysql_async::Conn::new(opts).await {
                let _ = killer.query_drop(format!("KILL QUERY {id}")).await;
                let _ = killer.disconnect().await;
            }
            drop(conn);
        });
    }
}

pub(super) async fn disconnect(pool: Pool) -> Result<()> {
    pool.pool.disconnect().await.map_err(error)
}

pub(super) async fn begin(pool: &Pool) -> Result<Conn> {
    let mut conn = checkout(pool).await?;
    // COM_RESET_CONNECTION on return to the pool rolls back an abandoned transaction.
    conn.reset_connection(true);
    let sql = "START TRANSACTION";
    super::log::traced(super::log::statement(&pool.peer, sql), async {
        conn.query_drop(sql).await.map_err(error)
    })
    .await?;
    Ok(Conn(Arc::new(Mutex::new(conn))))
}

/// Streams the rows of `query` on a connection of its own.
pub(super) fn cursor<'a>(pool: &Pool, query: Compiled, buffer: usize) -> Cursor<'a> {
    let pool = pool.clone();
    Cursor::spawn(buffer, |sink| async move {
        stream(&mut checkout(&pool).await?, query, &sink).await
    })
}

/// Connection held by a transaction.
pub(super) struct Conn(Arc<Mutex<mysql_async::Conn>>);

impl Conn {
    pub async fn fetch_all<T>(
        &self,
        query: Compiled,
        f: impl FnMut(Row) -> Result<T>,
    ) -> Result<Vec<T>> {
        fetch_all(&mut *self.0.lock().await, query, f).await
    }

    pub async fn execute(&self, query: Compiled) -> Result<Executed> {
        execute(&mut *self.0.lock().await, query).await
    }

    /// Streams the rows of `query`, holding the connection until the cursor
    /// is done or dropped.
    pub fn cursor<'a>(&self, query: Compiled, buffer: usize) -> Cursor<'a> {
        let conn = Arc::clone(&self.0);
        Cursor::spawn(buffer, |sink| async move {
            stream(&mut *conn.lock_owned().await, query, &sink).await
        })
    }

    /// COMMIT, ROLLBACK, SAVEPOINT, ...: not preparable, so sent as plain text.
    pub async fn control(&self, sql: &str) -> Result<()> {
        self.0.lock().await.query_drop(sql).await.map_err(error)
    }
}

/// The rows of `query`, each turned into a `T` by `f` as it arrives.
pub(super) async fn fetch_all<T>(
    conn: &mut mysql_async::Conn,
    query: Compiled,
    mut f: impl FnMut(Row) -> Result<T>,
) -> Result<Vec<T>> {
    let params = params(query.binds)?;
    let mut result = conn.exec_iter(query.sql, params).await.map_err(error)?;
    let columns = Columns::new(result.columns());
    let mut out = Vec::new();
    while let Some(row) = result.next().await.map_err(error)? {
        out.push(f(columns.row(row)?)?);
    }
    Ok(out)
}

/// Sends the rows of `query` to `sink` until they run out or the cursor is
/// dropped. An unread rest is discarded by `mysql_async` before the
/// connection's next use.
async fn stream(conn: &mut mysql_async::Conn, query: Compiled, sink: &Sink) -> Result<()> {
    let params = params(query.binds)?;
    let mut result = conn.exec_iter(query.sql, params).await.map_err(error)?;
    let columns = Columns::new(result.columns());
    while let Some(row) = result.next().await.map_err(error)? {
        if !sink.send(columns.row(row)?).await {
            break;
        }
    }
    Ok(())
}

/// Column metadata of a result, to decode its rows.
struct Columns {
    columns: Arc<[Column]>,
    names: Arc<[String]>,
}

impl Columns {
    fn new(columns: Option<Arc<[Column]>>) -> Self {
        let columns = columns.unwrap_or_else(|| Arc::from([]));
        let names = columns.iter().map(|c| c.name_str().into_owned()).collect();
        Self { columns, names }
    }

    fn row(&self, row: mysql_async::Row) -> Result<Row> {
        let values = row
            .unwrap()
            .into_iter()
            .zip(self.columns.iter())
            .map(|(value, column)| {
                from_mysql(value, column)
                    .map_err(|source| Error::decode(None, &column.name_str(), source))
            })
            .collect::<Result<_>>()?;
        Ok(Row::new(Arc::clone(&self.names), values, Dialect::MySql))
    }
}

pub(super) async fn execute(conn: &mut mysql_async::Conn, query: Compiled) -> Result<Executed> {
    if query.binds.is_empty() {
        match prepared(conn, &query.sql, Params::Empty).await {
            // CREATE TRIGGER, LOCK TABLES, ... cannot be prepared; without bindings
            // the text protocol runs them as they are.
            Err(e) if server_code(&e) == Some(UNPREPARABLE) => {}
            result => return result,
        }
        let result = conn.query_iter(query.sql).await.map_err(error)?;
        let executed = Executed {
            rows_affected: result.affected_rows(),
            last_insert_id: result.last_insert_id(),
        };
        result.drop_result().await.map_err(error)?;
        return Ok(executed);
    }
    let params = params(query.binds)?;
    prepared(conn, &query.sql, params).await
}

/// "This command is not supported in the prepared statement protocol yet".
const UNPREPARABLE: u16 = 1295;

fn server_code(e: &Error) -> Option<u16> {
    match e {
        Error::Database { code, .. } => code.and_then(|c| u16::try_from(c).ok()),
        _ => None,
    }
}

async fn prepared(conn: &mut mysql_async::Conn, sql: &str, params: Params) -> Result<Executed> {
    let result = conn.exec_iter(sql, params).await.map_err(error)?;
    let executed = Executed {
        rows_affected: result.affected_rows(),
        last_insert_id: result.last_insert_id(),
    };
    result.drop_result().await.map_err(error)?;
    Ok(executed)
}

fn error(e: mysql_async::Error) -> Error {
    let code = match &e {
        mysql_async::Error::Server(server) => Some(i32::from(server.code)),
        _ => None,
    };
    Error::Database {
        dialect: Dialect::MySql,
        code,
        message: e.to_string(),
    }
}

fn params(binds: Vec<Value>) -> Result<Params> {
    if binds.is_empty() {
        return Ok(Params::Empty);
    }
    binds
        .into_iter()
        .map(to_mysql)
        .collect::<Result<_>>()
        .map(Params::Positional)
}

fn to_mysql(value: Value) -> Result<MyValue> {
    Ok(match value {
        Value::Null => MyValue::NULL,
        Value::Bool(b) => MyValue::Int(i64::from(b)),
        Value::I64(n) => MyValue::Int(n),
        Value::U64(n) => MyValue::UInt(n),
        Value::F64(n) => MyValue::Double(n),
        Value::Decimal(d) => MyValue::Bytes(d.to_string().into_bytes()),
        Value::String(s) => MyValue::Bytes(s.into_bytes()),
        Value::Bytes(b) => MyValue::Bytes(b),
        Value::Date(d) => datetime(d.and_time(NaiveTime::MIN))?,
        Value::DateTime(dt) => datetime(dt)?,
        Value::Time(t) => MyValue::Time(
            false,
            0,
            small(t.hour()),
            small(t.minute()),
            small(t.second()),
            t.nanosecond() / 1000,
        ),
        Value::Json(j) => MyValue::Bytes(j.into_bytes()),
    })
}

fn datetime(dt: NaiveDateTime) -> Result<MyValue> {
    let year = u16::try_from(dt.year())
        .map_err(|_| Error::InvalidQuery(format!("year {} is outside MySQL's range", dt.year())))?;
    Ok(MyValue::Date(
        year,
        small(dt.month()),
        small(dt.day()),
        small(dt.hour()),
        small(dt.minute()),
        small(dt.second()),
        dt.nanosecond() / 1000,
    ))
}

fn small(n: u32) -> u8 {
    u8::try_from(n).expect("chrono date and time components fit in u8")
}

fn from_mysql(value: MyValue, column: &Column) -> Result<Value, ValueError> {
    let invalid = |expected, found| ValueError { expected, found };
    Ok(match value {
        MyValue::NULL => Value::Null,
        MyValue::Int(n) => Value::I64(n),
        MyValue::UInt(n) => Value::U64(n),
        MyValue::Float(n) => Value::F64(f64::from(n)),
        MyValue::Double(n) => Value::F64(n),
        // `0000-00-00` (allowed without NO_ZERO_DATE): no date, like JDBC's `convertToNull`.
        MyValue::Date(0, 0, 0, ..) => Value::Null,
        MyValue::Date(year, month, day, hour, minute, second, micros) => {
            let date = NaiveDate::from_ymd_opt(year.into(), month.into(), day.into())
                .ok_or(invalid("date", "zero or invalid date"))?;
            if column.column_type() == ColumnType::MYSQL_TYPE_DATE {
                Value::Date(date)
            } else {
                let time = NaiveTime::from_hms_micro_opt(
                    hour.into(),
                    minute.into(),
                    second.into(),
                    micros,
                )
                .ok_or(invalid("datetime", "invalid time of day"))?;
                Value::DateTime(date.and_time(time))
            }
        }
        // A duration outside a day (`-01:00:00`, `25:00:00`) stays text, as MySQL prints it.
        MyValue::Time(negative, days, hour, minute, second, micros) if negative || days > 0 => {
            let hours = days * 24 + u32::from(hour);
            let sign = if negative { "-" } else { "" };
            let fraction = if micros > 0 {
                format!(".{micros:06}")
            } else {
                String::new()
            };
            Value::String(format!(
                "{sign}{hours:02}:{minute:02}:{second:02}{fraction}"
            ))
        }
        MyValue::Time(_, _, hour, minute, second, micros) => {
            NaiveTime::from_hms_micro_opt(hour.into(), minute.into(), second.into(), micros)
                .map(Value::Time)
                .ok_or(invalid("time of day", "invalid time"))?
        }
        MyValue::Bytes(bytes) => match column.column_type() {
            // MySQL only stores valid JSON; it is parsed when decoded into a field.
            ColumnType::MYSQL_TYPE_JSON => Value::Json(
                String::from_utf8(bytes).map_err(|_| invalid("json", "non-UTF-8 text"))?,
            ),
            ColumnType::MYSQL_TYPE_DECIMAL | ColumnType::MYSQL_TYPE_NEWDECIMAL => {
                let text =
                    String::from_utf8(bytes).map_err(|_| invalid("decimal", "non-UTF-8 text"))?;
                // DECIMAL(65) can exceed rust_decimal's 28 digits; keep the text then.
                text.parse().map_or(Value::String(text), Value::Decimal)
            }
            _ if column.character_set() == BINARY_CHARSET => Value::Bytes(bytes),
            _ => Value::String(
                String::from_utf8(bytes).map_err(|_| invalid("string", "non-UTF-8 text"))?,
            ),
        },
    })
}
