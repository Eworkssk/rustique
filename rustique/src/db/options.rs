//! [`DbBuilder`]: pool size, timeouts and TLS set in code instead of the URL.

use std::fmt;
#[cfg(feature = "mysql-rustls")]
use std::path::PathBuf;
use std::time::Duration;

use super::{Db, Executor as _};
use crate::{Dialect, Error, Result};

/// How long a query waits for a free pooled connection unless
/// [`DbBuilder::acquire_timeout`] says otherwise.
pub(super) const ACQUIRE_TIMEOUT: Duration = Duration::from_secs(30);

/// Error of a query that found no free pooled connection in time.
pub(super) fn acquire_timed_out(dialect: Dialect, timeout: Duration) -> Error {
    Error::Database {
        dialect,
        code: None,
        message: format!("no pooled connection available within {timeout:?}"),
    }
}

/// Pool settings; `None` keeps the URL's value or the driver's default.
#[derive(Debug, Clone, Default)]
pub(super) struct PoolConfig {
    pub min_connections: Option<u32>,
    pub max_connections: Option<u32>,
    pub acquire_timeout: Option<Duration>,
    pub idle_timeout: Option<Duration>,
    pub max_lifetime: Option<Duration>,
    /// See [`DbBuilder::keep_sessions_after_raw_sql`].
    pub keep_sessions: bool,
    /// See [`DbBuilder::session_setup`].
    pub session_setup: Vec<String>,
    /// See [`DbBuilder::timezone`].
    pub timezone: Option<String>,
    #[cfg(feature = "mysql-rustls")]
    pub tls: Option<Tls>,
}

/// Connection settings for [`Db`], from [`Db::builder`].
///
/// Settings made here override the same option in the URL.
///
/// ```no_run
/// # use std::time::Duration;
/// # use rustique::Db;
/// # async fn run() -> rustique::Result<()> {
/// let db = Db::builder("mysql://user:password@localhost/app")
///     .max_connections(20)
///     .acquire_timeout(Duration::from_secs(5))
///     .connect()
///     .await?;
/// # Ok(()) }
/// ```
#[must_use = "call `connect` to open the pool"]
pub struct DbBuilder {
    url: String,
    read_urls: Vec<String>,
    sticky: bool,
    config: PoolConfig,
}

impl DbBuilder {
    pub(super) fn new(url: &str) -> Self {
        Self {
            url: url.to_owned(),
            read_urls: Vec::new(),
            sticky: false,
            config: PoolConfig::default(),
        }
    }

    /// Most connections the pool opens. Defaults: MySQL 100 (`?pool_max=`), Oracle 10.
    pub fn max_connections(mut self, max: u32) -> Self {
        self.config.max_connections = Some(max);
        self
    }

    /// Idle connections the pool keeps open. Defaults: MySQL 10 (`?pool_min=`),
    /// Oracle 1. Oracle opens them at connect (with [`connect_lazy`](Self::connect_lazy),
    /// at the first query); MySQL keeps them once opened.
    pub fn min_connections(mut self, min: u32) -> Self {
        self.config.min_connections = Some(min);
        self
    }

    /// How long a query waits for a free connection (including opening a new
    /// one) before failing with [`Error::Database`]. Default 30 seconds.
    pub fn acquire_timeout(mut self, timeout: Duration) -> Self {
        self.config.acquire_timeout = Some(timeout);
        self
    }

    /// Closes connections idle this long, beyond [`min_connections`](Self::min_connections).
    /// Defaults: MySQL closes them at its next 30-second check
    /// (`?inactive_connection_ttl=`), Oracle never.
    pub fn idle_timeout(mut self, timeout: Duration) -> Self {
        self.config.idle_timeout = Some(timeout);
        self
    }

    /// Closes connections this old once they return to the pool, for example
    /// to follow a load balancer or a password rotation. Default: never.
    pub fn max_lifetime(mut self, lifetime: Duration) -> Self {
        self.config.max_lifetime = Some(lifetime);
        self
    }

    /// Returns connections that ran raw SQL ([`select`](crate::Executor::select) /
    /// [`statement`](crate::Executor::statement)) to the pool as they are. By
    /// default MySQL resets such a connection, and Oracle replaces a session
    /// that ran anything but a query or DML (PL/SQL, `ALTER SESSION`, ...;
    /// about 25 ms each), so session state cannot reach later queries; see
    /// `docs/database.md`, "Session state". Set this only when the
    /// application's raw SQL leaves no session state behind, such as PL/SQL
    /// procedures that only read and write tables. MySQL transactions are
    /// still reset.
    pub fn keep_sessions_after_raw_sql(mut self) -> Self {
        self.config.keep_sessions = true;
        self
    }

    /// SQL run on every new connection before its first query, such as
    /// `SET SESSION ...` (MySQL) or `ALTER SESSION SET ...` (Oracle), so
    /// session settings hold for every query of the pool. MySQL also runs it
    /// again after each connection reset (after a transaction or raw SQL), and
    /// Oracle on the new session that replaces a dropped one. A statement that
    /// fails fails the query that opened the connection.
    pub fn session_setup<I, S>(mut self, statements: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.config.session_setup = statements.into_iter().map(Into::into).collect();
        self
    }

    /// Session time zone of every connection, such as `"Europe/Bratislava"`
    /// or `"+02:00"`: runs `SET time_zone = '...'` (MySQL) or
    /// `ALTER SESSION SET TIME_ZONE = '...'` (Oracle) before the
    /// [`session_setup`](Self::session_setup) statements, and again whenever
    /// those run. It sets what `NOW()` / `CURRENT_TIMESTAMP` and MySQL
    /// `TIMESTAMP` columns use; the zone of the date-times the ORM writes is
    /// [`set_timezone`](crate::set_timezone), so set both to the same zone.
    /// MySQL needs its time zone tables loaded for named zones. A zone with
    /// a quote or backslash fails [`connect`](Self::connect) with [`Error::Config`].
    pub fn timezone(mut self, zone: &str) -> Self {
        self.config.timezone = Some(zone.to_owned());
        self
    }

    /// Connects to MySQL over TLS, verifying the server against `tls`.
    /// Oracle takes TLS in the URL instead (`tcps://`, see [`Db::connect`]),
    /// so this fails there with [`Error::Config`].
    #[cfg(feature = "mysql-rustls")]
    pub fn tls(mut self, tls: Tls) -> Self {
        self.config.tls = Some(tls);
        self
    }

    /// Adds a read replica, like the `read` hosts of a Laravel connection:
    /// plain `SELECT`s outside transactions go to the replicas in turn,
    /// everything else to the main URL. See [`Db::write`] for reads that
    /// must see the latest writes. Each replica gets a pool with the same
    /// settings, and must use the same driver as the main URL.
    pub fn read_url(mut self, url: &str) -> Self {
        self.read_urls.push(url.to_owned());
        self
    }

    /// Laravel's `sticky` option, default off: inside a
    /// [`Db::sticky_scope`], once this pool writes (a statement or a
    /// transaction), its plain reads go to the primary until the scope ends,
    /// so a request reads back what it just wrote. No effect without replicas.
    pub fn sticky(mut self, sticky: bool) -> Self {
        self.sticky = sticky;
        self
    }

    /// Opens the pool and checks that the database answers.
    ///
    /// # Errors
    ///
    /// As [`Db::connect`], plus [`Error::Config`] when `min_connections`
    /// exceeds `max_connections` or `max_connections` is 0.
    pub async fn connect(self) -> Result<Db> {
        let db = self.connect_lazy()?;
        for pool in std::iter::once(&db.pool).chain(db.read.iter()) {
            pool.open().await?;
        }
        Ok(db)
    }

    /// Creates the pool without connecting: the first query opens the first
    /// connection, and fails if the database is down. Lets an application
    /// start while its database is unreachable. Oracle loads Instant Client
    /// only then, too. Needs a tokio runtime only once queries run.
    ///
    /// # Errors
    ///
    /// [`Error::Config`] for a malformed URL, a driver that is not compiled
    /// in, or the pool sizes [`connect`](Self::connect) rejects.
    pub fn connect_lazy(self) -> Result<Db> {
        let PoolConfig {
            min_connections,
            max_connections,
            ..
        } = self.config;
        if max_connections == Some(0) {
            return Err(Error::Config("max_connections must be at least 1".into()));
        }
        if let (Some(min), Some(max)) = (min_connections, max_connections)
            && min > max
        {
            return Err(Error::Config(format!(
                "min_connections ({min}) exceeds max_connections ({max})"
            )));
        }
        let mut db = Db::open(&self.url, self.config.clone())?;
        let mut read = Vec::with_capacity(self.read_urls.len());
        for url in &self.read_urls {
            let replica = Db::open(url, self.config.clone())?;
            if replica.dialect() != db.dialect() {
                return Err(Error::Config(
                    "a read replica must use the same driver as the main URL".into(),
                ));
            }
            read.push(replica.pool);
        }
        db.read = read.into();
        db.sticky = self.sticky;
        Ok(db)
    }
}

impl fmt::Debug for DbBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The URL holds the password.
        f.debug_struct("DbBuilder")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

/// TLS settings for MySQL, for [`DbBuilder::tls`].
///
/// `Tls::new()` alone trusts the system's root certificates, like
/// `?require_ssl=true`.
///
/// ```no_run
/// # use rustique::{Db, Tls};
/// # async fn run() -> rustique::Result<()> {
/// let tls = Tls::new()
///     .root_cert("/etc/ssl/mysql/ca.pem")
///     .client_identity("/etc/ssl/mysql/client-cert.pem", "/etc/ssl/mysql/client-key.pem");
/// let db = Db::builder("mysql://app@db.internal/app").tls(tls).connect().await?;
/// # Ok(()) }
/// ```
#[cfg(feature = "mysql-rustls")]
#[derive(Debug, Clone, Default)]
pub struct Tls {
    pub(super) root_certs: Vec<PathBuf>,
    pub(super) client_identity: Option<(PathBuf, PathBuf)>,
    pub(super) accept_invalid_certs: bool,
}

#[cfg(feature = "mysql-rustls")]
impl Tls {
    /// TLS that trusts the system's root certificates.
    pub fn new() -> Self {
        Self::default()
    }

    /// Also trusts the CA certificates in `path` (PEM or DER), such as a
    /// cloud provider's bundle or a private CA. Callable more than once.
    #[must_use]
    pub fn root_cert(mut self, path: impl Into<PathBuf>) -> Self {
        self.root_certs.push(path.into());
        self
    }

    /// Client certificate chain and private key (both PEM), for servers that
    /// require `X509` / `REQUIRE SUBJECT` accounts.
    #[must_use]
    pub fn client_identity(mut self, cert: impl Into<PathBuf>, key: impl Into<PathBuf>) -> Self {
        self.client_identity = Some((cert.into(), key.into()));
        self
    }

    /// Encrypts without checking who the server is: any certificate and host
    /// name pass. For local servers with self-signed certificates only; on a
    /// network this allows a man-in-the-middle. Prefer [`root_cert`](Self::root_cert).
    #[must_use]
    pub fn danger_accept_invalid_certs(mut self) -> Self {
        self.accept_invalid_certs = true;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn error(builder: DbBuilder) -> String {
        builder.connect().await.unwrap_err().to_string()
    }

    // Port 1 refuses connections, so passing validation would fail differently.
    const URL: &str = "mysql://app:pw@127.0.0.1:1/app";

    #[tokio::test]
    async fn rejects_invalid_pool_sizes() {
        let e = error(Db::builder(URL).max_connections(0)).await;
        assert!(e.contains("at least 1"), "{e}");
        let e = error(Db::builder(URL).min_connections(5).max_connections(2)).await;
        assert!(e.contains("exceeds"), "{e}");
    }

    #[cfg(all(feature = "mysql-rustls", feature = "oracle"))]
    #[tokio::test]
    async fn rejects_tls_settings_for_oracle() {
        let e = error(Db::builder("oracle://app:pw@db:1521/X").tls(Tls::new())).await;
        assert!(e.contains("tcps://"), "{e}");
    }

    #[test]
    fn debug_hides_the_url() {
        let debug = format!("{:?}", Db::builder(URL));
        assert!(!debug.contains("pw"), "{debug}");
    }
}
