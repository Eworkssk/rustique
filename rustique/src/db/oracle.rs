//! Oracle driver glue on top of the `oracle` crate (ODPI-C).
//!
//! The driver is blocking, so every call runs on tokio's blocking thread pool.
//! A semaphore with one permit per pooled connection makes callers wait for a
//! connection asynchronously, so a busy pool never parks more blocking threads
//! than it has connections.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use chrono::{DateTime, NaiveDateTime, Utc};
use oracle::pool::{CloseMode, GetMode, PoolBuilder};
use oracle::sql_type::{OracleType, ToSql};
use oracle::{Connection, SqlValue};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use super::Executed;
use super::cursor::{Cursor, Sink};
use super::options::{ACQUIRE_TIMEOUT, PoolConfig, acquire_timed_out};
use crate::grammar::Compiled;
use crate::{Dialect, Error, Result, Row, Value, ValueError};

/// Pool size unless [`DbBuilder::max_connections`](super::DbBuilder::max_connections) says otherwise.
const MAX_CONNECTIONS: u32 = 10;

/// Session pool. Cheap to clone.
#[derive(Clone)]
pub(super) struct Pool {
    sessions: Arc<Sessions>,
    /// One permit per connection, held while a caller uses one.
    permits: Arc<Semaphore>,
    acquire_timeout: Duration,
    /// See `DbBuilder::keep_sessions_after_raw_sql`.
    keep_sessions: bool,
}

/// The ODPI-C session pool, created on first use, so a lazy `Db` neither
/// connects nor loads Instant Client before its first query.
struct Sessions {
    config: Config,
    settings: PoolConfig,
    max: u32,
    acquire_timeout: Duration,
    created: Mutex<Option<oracle::pool::Pool>>,
}

impl Pool {
    /// A pool for `url` that connects on first use.
    pub fn new(url: &str, settings: PoolConfig) -> Result<Self> {
        #[cfg(feature = "mysql-rustls")]
        if settings.tls.is_some() {
            return Err(Error::Config(
                "Oracle TLS goes in the URL: oracle://user:password@tcps://host:2484/service?wallet_location=/path".to_owned(),
            ));
        }
        let config = Config::parse(url)?;
        let max = settings.max_connections.unwrap_or(MAX_CONNECTIONS);
        if let Some(min) = settings.min_connections
            && min > max
        {
            return Err(Error::Config(format!(
                "min_connections ({min}) exceeds max_connections ({max})"
            )));
        }
        let acquire_timeout = settings.acquire_timeout.unwrap_or(ACQUIRE_TIMEOUT);
        let keep_sessions = settings.keep_sessions;
        Ok(Self {
            sessions: Arc::new(Sessions {
                config,
                settings,
                max,
                acquire_timeout,
                created: Mutex::new(None),
            }),
            permits: Arc::new(Semaphore::new(max as usize)),
            acquire_timeout,
            keep_sessions,
        })
    }

    /// Creates the session pool now, which opens `min_connections` sessions.
    pub async fn open(&self) -> Result<()> {
        let sessions = Arc::clone(&self.sessions);
        blocking(move || sessions.pool().map(drop)).await
    }

    pub async fn close(self) -> Result<()> {
        blocking(move || match &*self.sessions.created() {
            Some(pool) => pool.close(&CloseMode::Default).map_err(error),
            None => Ok(()),
        })
        .await
    }

    /// Waits for a free connection without holding a thread.
    async fn permit(&self) -> Result<OwnedSemaphorePermit> {
        let permits = Arc::clone(&self.permits);
        match tokio::time::timeout(self.acquire_timeout, permits.acquire_owned()).await {
            Ok(permit) => Ok(permit.expect("the semaphore is never closed")),
            Err(_) => Err(acquire_timed_out(Dialect::Oracle, self.acquire_timeout)),
        }
    }

    /// Runs `f` on a pooled connection in autocommit mode. `discard`: the
    /// session leaves the pool afterwards (see [`changes_session`]).
    async fn run<T: Send + 'static>(
        &self,
        discard: bool,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let permit = self.permit().await?;
        let sessions = Arc::clone(&self.sessions);
        cancellable(move |call| {
            let _permit = permit; // dropped after `conn`, once it is back in the pool
            let conn = Arc::new(autocommit(sessions.get()?, true));
            let result = call.run(&conn, true, f);
            if discard {
                let _ = conn.close_with_mode(oracle::conn::CloseMode::Drop);
            }
            result
        })
        .await
    }

    /// `raw`: `query` is raw SQL from `Db::select`; see [`changes_session`].
    pub async fn fetch_all<T: Send + 'static>(
        &self,
        query: Compiled,
        raw: bool,
        f: impl FnMut(Row) -> Result<T> + Send + 'static,
    ) -> Result<Vec<T>> {
        let discard = raw && !self.keep_sessions && changes_session(&query.sql);
        self.run(discard, move |conn| fetch_all(conn, &query, f))
            .await
    }

    /// `raw`: `query` is raw SQL from `Db::statement`; see [`changes_session`].
    pub async fn execute(&self, query: Compiled, raw: bool) -> Result<Executed> {
        let discard = raw && !self.keep_sessions && changes_session(&query.sql);
        self.run(discard, move |conn| execute(conn, &query)).await
    }

    /// Streams the rows of `query` on a connection of its own.
    pub fn cursor<'a>(&self, query: Compiled, buffer: usize) -> Cursor<'a> {
        let pool = self.clone();
        Cursor::spawn(buffer, |sink| async move {
            let permit = pool.permit().await?;
            blocking(move || {
                let _permit = permit;
                let conn = autocommit(pool.sessions.get()?, true);
                stream(&conn, &query, &sink)
            })
            .await
        })
    }

    pub async fn begin(&self) -> Result<Conn> {
        let permit = self.permit().await?;
        let sessions = Arc::clone(&self.sessions);
        let keep = self.keep_sessions;
        blocking(move || {
            // Oracle starts the transaction with the first statement.
            let conn = autocommit(sessions.get()?, false);
            Ok(Conn(Arc::new(Session {
                conn: Arc::new(conn),
                lock: Mutex::new(()),
                changed: AtomicBool::new(false),
                keep,
                permit: Some(permit),
            })))
        })
        .await
    }
}

impl Sessions {
    fn created(&self) -> MutexGuard<'_, Option<oracle::pool::Pool>> {
        self.created.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The ODPI-C pool, created by the first call. Blocking.
    fn pool(&self) -> Result<oracle::pool::Pool> {
        let mut created = self.created();
        if let Some(pool) = &*created {
            return Ok(pool.clone());
        }
        let config = &self.config;
        let mut builder = PoolBuilder::new(&config.user, &config.password, &config.connect);
        builder.max_connections(self.max);
        if let Some(min) = self.settings.min_connections {
            builder.min_connections(min);
        }
        // Callers wait on the semaphore; this only covers a connection
        // that is still on its way back to the pool.
        builder.get_mode(GetMode::TimedWait(self.acquire_timeout));
        if let Some(timeout) = self.settings.idle_timeout {
            builder.timeout(timeout).map_err(error)?;
        }
        if let Some(lifetime) = self.settings.max_lifetime {
            builder.max_lifetime_connection(lifetime).map_err(error)?;
        }
        let pool = builder.build().map_err(error)?;
        *created = Some(pool.clone());
        Ok(pool)
    }

    /// A pooled session; a new one first runs `DbBuilder::session_setup`. Blocking.
    fn get(&self) -> Result<Connection> {
        let conn = self.pool()?.get().map_err(error)?;
        if conn.is_new_connection() {
            for sql in &self.settings.session_setup {
                if let Err(e) = conn.execute(sql, &[]) {
                    // Never pooled half set up.
                    let _ = conn.close_with_mode(oracle::conn::CloseMode::Drop);
                    return Err(error(e));
                }
            }
        }
        Ok(conn)
    }
}

/// Whether raw `sql` may change session state that later users of the pooled
/// session would inherit: `ALTER SESSION` (time zone, NLS, `CURRENT_SCHEMA`),
/// `SET ROLE`, PL/SQL package state and contexts. Oracle has no reset short of
/// a new session, so such a session is dropped from the pool after use.
/// Queries and DML keep their session; anything else, including SQL that
/// starts with a comment or a parenthesis, does not.
fn changes_session(sql: &str) -> bool {
    let word: String = (sql.trim_start().chars())
        .take_while(char::is_ascii_alphabetic)
        .collect();
    !["SELECT", "WITH", "INSERT", "UPDATE", "DELETE", "MERGE"]
        .iter()
        .any(|keyword| keyword.eq_ignore_ascii_case(&word))
}

fn autocommit(mut conn: Connection, on: bool) -> Connection {
    conn.set_autocommit(on);
    conn
}

/// Connection held by a transaction.
pub(super) struct Conn(Arc<Session>);

struct Session {
    conn: Arc<Connection>,
    /// One call at a time; only locked on blocking threads.
    lock: Mutex<()>,
    /// Raw SQL that [`changes_session`] ran: the session is dropped at the end.
    changed: AtomicBool,
    /// `DbBuilder::keep_sessions_after_raw_sql`: never set `changed`.
    keep: bool,
    /// Released after `conn` goes back to the pool (fields drop in order), or
    /// is dropped.
    permit: Option<OwnedSemaphorePermit>,
}

impl Drop for Session {
    fn drop(&mut self) {
        if *self.changed.get_mut() {
            let conn = Arc::clone(&self.conn);
            let permit = self.permit.take();
            // A network round trip, so not on this (maybe async) thread.
            in_background(move || {
                let _ = conn.close_with_mode(oracle::conn::CloseMode::Drop);
                drop(permit);
            });
        }
    }
}

impl Conn {
    /// Records raw SQL run in the transaction; see [`changes_session`].
    pub fn note_raw(&self, sql: &str) {
        if !self.0.keep && changes_session(sql) {
            self.0.changed.store(true, Ordering::Relaxed);
        }
    }

    async fn run<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Connection) -> Result<T> + Send + 'static,
    ) -> Result<T> {
        let session = Arc::clone(&self.0);
        cancellable(move |call| {
            let _guard = lock(&session.lock)?;
            call.run(&session.conn, false, f)
        })
        .await
    }

    pub async fn fetch_all<T: Send + 'static>(
        &self,
        query: Compiled,
        f: impl FnMut(Row) -> Result<T> + Send + 'static,
    ) -> Result<Vec<T>> {
        self.run(move |conn| fetch_all(conn, &query, f)).await
    }

    pub async fn execute(&self, query: Compiled) -> Result<Executed> {
        self.run(move |conn| execute(conn, &query)).await
    }

    /// Streams the rows of `query`, holding the connection until the
    /// cursor is done or dropped.
    pub fn cursor<'a>(&self, query: Compiled, buffer: usize) -> Cursor<'a> {
        let session = Arc::clone(&self.0);
        Cursor::spawn_blocking(buffer, move |sink| {
            let _guard = lock(&session.lock)?;
            stream(&session.conn, &query, sink)
        })
    }

    /// COMMIT, ROLLBACK, SAVEPOINT, ...
    pub async fn control(&self, sql: &str) -> Result<()> {
        let sql = sql.to_owned();
        self.run(move |conn| conn.execute(&sql, &[]).map(drop).map_err(error))
            .await
    }

    /// Rolls back without waiting, for a transaction dropped unfinished. The
    /// connection returns to the pool only after the rollback, when the last
    /// `Arc` is dropped. (ODPI-C would also roll back on release.)
    pub fn rollback_in_background(&self) {
        let session = Arc::clone(&self.0);
        in_background(move || {
            if let Ok(_guard) = lock(&session.lock) {
                // Nobody is left to report to; a broken session is discarded by the pool.
                let _ = session.conn.rollback();
            }
        });
    }
}

fn lock(mutex: &Mutex<()>) -> Result<MutexGuard<'_, ()>> {
    mutex.lock().map_err(|_| Error::Database {
        dialect: Dialect::Oracle,
        code: None,
        message: "connection unusable: a previous call panicked while using it".to_owned(),
    })
}

/// Runs `f` on a blocking thread without waiting for it, or right away
/// outside a runtime.
fn in_background(f: impl FnOnce() + Send + 'static) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => drop(runtime.spawn_blocking(f)),
        Err(_) => f(),
    }
}

/// Runs blocking `f` like [`blocking`], but dropping the returned future
/// also stops the database call: one not started yet never starts, one
/// running is interrupted with OCIBreak (the server ends it with ORA-01013).
async fn cancellable<T: Send + 'static>(
    f: impl FnOnce(&Arc<Call>) -> Result<T> + Send + 'static,
) -> Result<T> {
    let call = Arc::new(Call(Mutex::new(CallState::Idle)));
    let _cancel = CancelOnDrop(Arc::clone(&call));
    blocking(move || f(&call)).await
}

/// The database call of one [`cancellable`] future.
struct Call(Mutex<CallState>);

enum CallState {
    Idle,
    Running(Arc<Connection>),
    Cancelled,
}

impl Call {
    fn state(&self) -> MutexGuard<'_, CallState> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Runs `f` on `conn`, unless the caller is already gone. With `discard`,
    /// a cancelled call's session is closed instead of going back to the
    /// pool, so no late break can reach its next user.
    fn run<T>(
        &self,
        conn: &Arc<Connection>,
        discard: bool,
        f: impl FnOnce(&Connection) -> Result<T>,
    ) -> Result<T> {
        {
            let mut state = self.state();
            if matches!(*state, CallState::Cancelled) {
                return Err(Error::Database {
                    dialect: Dialect::Oracle,
                    code: None,
                    message: "cancelled".to_owned(),
                });
            }
            *state = CallState::Running(Arc::clone(conn));
        }
        let result = f(conn);
        // Waits for a break in flight, which holds the lock.
        let state = std::mem::replace(&mut *self.state(), CallState::Idle);
        if discard && matches!(state, CallState::Cancelled) {
            let _ = conn.close_with_mode(oracle::conn::CloseMode::Drop);
        }
        result
    }
}

/// Cancels its [`Call`] when the future holding it is dropped; a no-op once
/// the call has finished.
struct CancelOnDrop(Arc<Call>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        let previous = std::mem::replace(&mut *self.0.state(), CallState::Cancelled);
        if let CallState::Running(conn) = previous {
            let call = Arc::clone(&self.0);
            // A network round trip, so not on this async thread.
            in_background(move || {
                let state = call.state();
                // `Call::run` sets Idle under this lock before the connection is
                // used again, so a break sent while Cancelled can only hit this
                // call or an idle connection (harmless), never the next statement.
                if matches!(*state, CallState::Cancelled) {
                    let _ = conn.break_execution();
                }
            });
        }
    }
}

/// Runs blocking driver code off the async threads. Panics propagate to the caller.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    match tokio::task::spawn_blocking(f).await {
        Ok(result) => result,
        Err(e) if e.is_panic() => std::panic::resume_unwind(e.into_panic()),
        Err(e) => Err(Error::Database {
            dialect: Dialect::Oracle,
            code: None,
            message: e.to_string(),
        }),
    }
}

fn error(e: oracle::Error) -> Error {
    Error::Database {
        dialect: Dialect::Oracle,
        code: e.oci_code(),
        message: e.to_string(),
    }
}

/// The rows of `query`, each turned into a `T` by `f` as it arrives.
fn fetch_all<T>(
    conn: &Connection,
    query: &Compiled,
    mut f: impl FnMut(Row) -> Result<T>,
) -> Result<Vec<T>> {
    if query.returning > 0 {
        return fetch_out_binds(conn, query)?.into_iter().map(f).collect();
    }
    let mut out = Vec::new();
    let mut failed = None;
    for_each_row(conn, query, |row| match f(row) {
        Ok(value) => {
            out.push(value);
            true
        }
        Err(error) => {
            failed = Some(error);
            false
        }
    })?;
    failed.map_or(Ok(out), Err)
}

/// Runs a PL/SQL block whose last `query.returning` placeholders are
/// numeric out binds, and returns their values as rows of one `ID` column.
fn fetch_out_binds(conn: &Connection, query: &Compiled) -> Result<Vec<Row>> {
    let out = OracleType::Number(0, 0);
    let mut params: Vec<&dyn ToSql> = query.binds.iter().map(|v| v as &dyn ToSql).collect();
    params.extend(std::iter::repeat_n(&out as &dyn ToSql, query.returning));
    let mut statement = conn.statement(&query.sql).build().map_err(error)?;
    statement.execute(&params).map_err(error)?;
    let names: Arc<[String]> = ["ID".to_owned()].into();
    (1..=query.returning)
        .map(|i| {
            let id: Option<u64> = statement.bind_value(query.binds.len() + i).map_err(error)?;
            let value = id.map_or(Value::Null, Value::U64);
            Ok(Row::new(Arc::clone(&names), vec![value], Dialect::Oracle))
        })
        .collect()
}

/// Sends the rows of `query` to `sink` until they run out or the cursor is dropped.
fn stream(conn: &Connection, query: &Compiled, sink: &Sink) -> Result<()> {
    for_each_row(conn, query, |row| sink.blocking_send(Ok(row)).is_ok())
}

/// Runs `query` and passes each row to `f`, stopping when it returns `false`.
fn for_each_row(conn: &Connection, query: &Compiled, mut f: impl FnMut(Row) -> bool) -> Result<()> {
    let params: Vec<&dyn ToSql> = query.binds.iter().map(|v| v as &dyn ToSql).collect();
    let mut statement = conn.statement(&query.sql).build().map_err(error)?;
    let rows = statement.query(&params).map_err(error)?;
    let names: Arc<[String]> = rows
        .column_info()
        .iter()
        .map(|c| c.name().to_owned())
        .collect();
    let types: Vec<OracleType> = rows
        .column_info()
        .iter()
        .map(|c| c.oracle_type().clone())
        .collect();

    for row in rows {
        let row = row.map_err(error)?;
        let values = row
            .sql_values()
            .iter()
            .zip(&types)
            .zip(names.iter())
            .map(|((value, oracle_type), name)| from_oracle(value, oracle_type, name))
            .collect::<Result<_>>()?;
        if !f(Row::new(Arc::clone(&names), values, Dialect::Oracle)) {
            break;
        }
    }
    Ok(())
}

fn execute(conn: &Connection, query: &Compiled) -> Result<Executed> {
    if let Some(rows) = query.batch {
        return execute_batch(conn, query, rows);
    }
    let mut params: Vec<&dyn ToSql> = query.binds.iter().map(|v| v as &dyn ToSql).collect();
    let out = None::<i64>;
    if query.returning > 0 {
        params.push(&out);
    }
    let mut statement = conn.statement(&query.sql).build().map_err(error)?;
    statement.execute(&params).map_err(error)?;
    let last_insert_id = if query.returning > 0 {
        let returned: Vec<Option<u64>> = statement.returned_values(params.len()).map_err(error)?;
        returned.into_iter().next().flatten()
    } else {
        None
    };
    Ok(Executed {
        rows_affected: statement.row_count().map_err(error)?,
        last_insert_id,
    })
}

/// Runs one statement for `rows` rows of binds with array binding, in a single
/// round trip, so autocommit commits all rows or none.
fn execute_batch(conn: &Connection, query: &Compiled, rows: usize) -> Result<Executed> {
    let width = query.binds.len() / rows;
    let mut batch = conn
        .batch(&query.sql, rows)
        .with_row_counts()
        .build()
        .map_err(error)?;
    // The driver types each column from its first row. Use the first non-NULL
    // value instead, so a leading NULL does not turn a number column into text,
    // or the longest text or binary value, so one long value makes the column a LOB.
    for column in 0..width {
        let values = || query.binds.iter().skip(column).step_by(width);
        let sample = values()
            .filter_map(|v| match v {
                Value::String(s) => Some((s.len(), v)),
                Value::Bytes(b) => Some((b.len(), v)),
                _ => None,
            })
            .max_by_key(|(len, _)| *len)
            .map(|(_, v)| v)
            .or_else(|| values().find(|v| !v.is_null()));
        if let Some(value) = sample {
            let oracle_type = value.oratype(conn).map_err(error)?;
            batch.set_type(column + 1, &oracle_type).map_err(error)?;
        }
    }
    for row in query.binds.chunks(width) {
        let params: Vec<&dyn ToSql> = row.iter().map(|v| v as &dyn ToSql).collect();
        batch.append_row(&params).map_err(error)?;
    }
    // Appending the last row already ran the batch; this only covers an empty one.
    batch.execute().map_err(error)?;
    // Per row, so a `MERGE` that skips a row counts it as 0.
    let rows_affected = batch.row_counts().map_err(error)?.iter().sum();
    Ok(Executed {
        rows_affected,
        last_insert_id: None,
    })
}

fn from_oracle(value: &SqlValue, oracle_type: &OracleType, column: &str) -> Result<Value> {
    if value.is_null().map_err(error)? {
        return Ok(Value::Null);
    }
    let value = match oracle_type {
        // Up to 18 digits always fits in i64.
        OracleType::Number(1..=18, 0) => Value::I64(value.get().map_err(error)?),
        OracleType::Number(..) | OracleType::Float(_) => {
            let text: String = value.get().map_err(error)?;
            // NUMBER(38) can exceed rust_decimal's 28 digits; keep the text then.
            text.parse().map_or(Value::String(text), Value::Decimal)
        }
        OracleType::BinaryFloat | OracleType::BinaryDouble => {
            Value::F64(value.get().map_err(error)?)
        }
        OracleType::Varchar2(_)
        | OracleType::NVarchar2(_)
        | OracleType::Char(_)
        | OracleType::NChar(_)
        | OracleType::Long
        | OracleType::CLOB
        | OracleType::NCLOB
        | OracleType::Rowid
        | OracleType::Json => Value::String(value.get().map_err(error)?),
        OracleType::Raw(_) | OracleType::LongRaw | OracleType::BLOB => {
            Value::Bytes(value.get().map_err(error)?)
        }
        OracleType::Date | OracleType::Timestamp(_) => {
            Value::DateTime(value.get::<NaiveDateTime>().map_err(error)?)
        }
        OracleType::TimestampTZ(_) | OracleType::TimestampLTZ(_) => Value::DateTime(
            crate::timezone::local(&value.get::<DateTime<Utc>>().map_err(error)?),
        ),
        OracleType::Boolean => Value::Bool(value.get().map_err(error)?),
        _ => {
            return Err(Error::decode(
                None,
                column,
                ValueError {
                    expected: "a supported Oracle type",
                    found: "unsupported Oracle type",
                },
            ));
        }
    };
    Ok(value)
}

impl ToSql for Value {
    fn oratype(&self, conn: &Connection) -> oracle::Result<OracleType> {
        match self {
            Self::Null => Ok(varchar("")),
            Self::Bool(_) | Self::I64(_) => 0_i64.oratype(conn),
            Self::U64(n) => n.oratype(conn),
            Self::F64(n) => n.oratype(conn),
            // NUMBER bound as text is parsed by the client with `.` as the
            // decimal separator, whatever NLS_NUMERIC_CHARACTERS says.
            Self::Decimal(_) => Ok(OracleType::Number(0, -127)),
            // Longer values only reach a LOB column, and as VARCHAR2 / RAW
            // binds SQL accepts them in an INSERT's VALUES only (ORA-01461).
            Self::String(s) if s.len() > MAX_INLINE_BIND => Ok(OracleType::CLOB),
            Self::Bytes(b) if b.len() > MAX_INLINE_BIND => Ok(OracleType::BLOB),
            Self::String(s) => Ok(varchar(s)),
            Self::Bytes(b) => b.oratype(conn),
            Self::Date(d) => d.oratype(conn),
            Self::DateTime(dt) => dt.oratype(conn),
            // Oracle has no TIME type; times travel as `HH:MM:SS[.f]` text.
            Self::Time(t) => Ok(varchar(&t.to_string())),
            Self::Json(j) if j.len() > MAX_INLINE_BIND => Ok(OracleType::CLOB),
            Self::Json(j) => Ok(varchar(j)),
        }
    }

    fn to_sql(&self, value: &mut SqlValue) -> oracle::Result<()> {
        match self {
            Self::Null => value.set_null(),
            Self::Bool(b) => value.set(&i64::from(*b)),
            Self::I64(n) => value.set(n),
            Self::U64(n) => value.set(n),
            Self::F64(n) => value.set(n),
            Self::Decimal(d) => value.set(&d.to_string()),
            Self::String(s) => value.set(s),
            Self::Bytes(b) => value.set(b),
            Self::Date(d) => value.set(d),
            Self::DateTime(dt) => value.set(dt),
            Self::Time(t) => value.set(&t.to_string()),
            Self::Json(j) => value.set(j),
        }
    }
}

/// Longest text or binary value bound as `VARCHAR2` / `RAW`; longer ones bind as LOBs.
const MAX_INLINE_BIND: usize = 32_767;

/// `VARCHAR2` bind type for `text`. The `oracle` crate binds Rust strings as
/// `NVARCHAR2`, which makes Oracle compare `SYS_OP_C2C(column)` and skip
/// indexes on `VARCHAR2` columns.
fn varchar(text: &str) -> OracleType {
    OracleType::Varchar2(u32::try_from(text.len()).unwrap_or(u32::MAX))
}

/// Parsed `oracle://user:password@host:port/service`.
#[derive(Debug, PartialEq)]
struct Config {
    user: String,
    password: String,
    /// EZConnect string: `//host:port/service`, or the address as given
    /// when it names a protocol (`tcps://host:2484/service`).
    connect: String,
}

impl Config {
    fn parse(url: &str) -> Result<Self> {
        let invalid =
            || Error::Config("expected oracle://user:password@host:port/service".to_owned());
        let rest = url.strip_prefix("oracle://").ok_or_else(invalid)?;
        // The password may contain `@`, so split at the last one.
        let (credentials, address) = rest.rsplit_once('@').ok_or_else(invalid)?;
        let (user, password) = credentials.split_once(':').unwrap_or((credentials, ""));
        if user.is_empty() || address.is_empty() {
            return Err(invalid());
        }
        Ok(Self {
            user: percent_decode(user)?,
            password: percent_decode(password)?,
            // EZConnect Plus: `tcps://...` (TLS) and its `?wallet_location=...` options pass through.
            connect: if address.contains("://") {
                address.to_owned()
            } else {
                format!("//{address}")
            },
        })
    }
}

fn percent_decode(text: &str) -> Result<String> {
    let invalid = || Error::Config("invalid percent-encoding in Oracle URL credentials".to_owned());
    let mut bytes = Vec::with_capacity(text.len());
    let mut rest = text.as_bytes();
    while let [first, tail @ ..] = rest {
        if *first == b'%' {
            let hex = tail
                .get(..2)
                .and_then(|h| std::str::from_utf8(h).ok())
                .ok_or_else(invalid)?;
            bytes.push(u8::from_str_radix(hex, 16).map_err(|_| invalid())?);
            rest = &tail[2..];
        } else {
            bytes.push(*first);
            rest = tail;
        }
    }
    String::from_utf8(bytes).map_err(|_| invalid())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_with_encoded_password() {
        let config = Config::parse("oracle://app:p%40ss:w@db.local:1521/ORCLPDB1").unwrap();
        assert_eq!(
            config,
            Config {
                user: "app".into(),
                password: "p@ss:w".into(),
                connect: "//db.local:1521/ORCLPDB1".into()
            }
        );
    }

    #[test]
    fn passes_tls_connect_strings_through() {
        let url = "oracle://app:pw@tcps://db.local:2484/ORCLPDB1?wallet_location=/w";
        let config = Config::parse(url).unwrap();
        assert_eq!(
            config.connect,
            "tcps://db.local:2484/ORCLPDB1?wallet_location=/w"
        );
    }

    #[test]
    fn rejects_url_without_credentials() {
        assert!(Config::parse("oracle://db.local:1521/X").is_err());
        assert!(Config::parse("oracle://app:pw@db/X%4").is_ok()); // only credentials are decoded
        assert!(Config::parse("oracle://app:%zz@db/X").is_err());
    }
}
