//! [`Transaction`] and [`Savepoint`].

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(feature = "mysql")]
use super::mysql;
#[cfg(feature = "oracle")]
use super::oracle;
use super::{Cursor, Executed, Executor, log, sealed};
use crate::grammar::{self, Compiled};
use crate::{Dialect, Error, Result, Row, Value};

/// A transaction on one dedicated connection.
///
/// Finish it with [`commit`](Self::commit) or [`rollback`](Self::rollback).
/// Dropping it unfinished rolls back: MySQL connections are reset when they
/// return to the pool, Oracle connections roll back on a blocking thread.
#[must_use = "a transaction rolls back unless committed"]
pub struct Transaction {
    conn: Conn,
    /// The database, for statement spans.
    peer: Arc<log::Peer>,
    savepoints: AtomicU32,
    /// A [`Cursor`] on this transaction is still reading.
    cursor_open: AtomicBool,
    finished: bool,
}

pub(super) enum Conn {
    #[cfg(feature = "mysql")]
    MySql(mysql::Conn),
    #[cfg(feature = "oracle")]
    Oracle(oracle::Conn),
}

/// A point inside a [`Transaction`] to roll back to, from [`Transaction::savepoint`].
#[derive(Debug)]
#[must_use = "pass the savepoint to `rollback_to` or `release`"]
pub struct Savepoint {
    name: String,
}

impl Transaction {
    pub(super) fn new(conn: Conn, peer: Arc<log::Peer>) -> Self {
        Self {
            conn,
            peer,
            savepoints: AtomicU32::new(0),
            cursor_open: AtomicBool::new(false),
            finished: false,
        }
    }

    /// Commits the transaction.
    ///
    /// # Errors
    ///
    /// [`Error::Database`](crate::Error::Database) when `COMMIT` fails; the
    /// transaction is then rolled back.
    pub async fn commit(mut self) -> Result<()> {
        self.run("COMMIT").await?;
        self.finished = true;
        Ok(())
    }

    /// Rolls the transaction back.
    ///
    /// # Errors
    ///
    /// [`Error::Database`](crate::Error::Database) when `ROLLBACK` fails.
    pub async fn rollback(mut self) -> Result<()> {
        self.finished = true;
        self.run("ROLLBACK").await
    }

    /// Creates a savepoint, the equivalent of a nested Laravel transaction.
    ///
    /// # Errors
    ///
    /// [`Error::Database`](crate::Error::Database) when `SAVEPOINT` fails.
    pub async fn savepoint(&self) -> Result<Savepoint> {
        let number = self.savepoints.fetch_add(1, Ordering::Relaxed) + 1;
        let name = format!("orm_sp{number}");
        self.run(&format!("SAVEPOINT {name}")).await?;
        Ok(Savepoint { name })
    }

    /// Runs `f` inside this transaction behind a savepoint, the equivalent
    /// of a nested `DB::transaction` in Laravel: when `f` returns `Err`, only
    /// its work is undone and the outer transaction carries on.
    ///
    /// ```no_run
    /// # use rustique::prelude::*;
    /// # async fn run(db: Db) -> rustique::Result<()> {
    /// db.transaction(async |tx| {
    ///     tx.statement("INSERT INTO orders (total) VALUES (?)", [10.into()]).await?;
    ///     let optional = tx
    ///         .transaction(async |tx| {
    ///             tx.statement("INSERT INTO coupons (code) VALUES (?)", ["X".into()]).await
    ///         })
    ///         .await;
    ///     if optional.is_err() { /* the order stays, the coupon is gone */ }
    ///     Ok::<_, rustique::Error>(())
    /// }).await
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// The closure's error, or [`Error::Database`](crate::Error::Database)
    /// when the savepoint cannot be created or released.
    pub async fn transaction<T, E, F>(&self, f: F) -> Result<T, E>
    where
        F: AsyncFnOnce(&Transaction) -> Result<T, E>,
        E: From<Error>,
    {
        let savepoint = self.savepoint().await?;
        match f(self).await {
            Ok(value) => {
                self.release(savepoint).await?;
                Ok(value)
            }
            Err(e) => {
                if let Err(rollback) = self.rollback_to(&savepoint).await {
                    tracing::warn!(target: "rustique::query", error = %rollback, "rollback to savepoint failed");
                }
                Err(e)
            }
        }
    }

    /// Undoes everything done since `savepoint` was created. The savepoint
    /// stays valid and can be rolled back to again.
    ///
    /// # Errors
    ///
    /// [`Error::Database`](crate::Error::Database) when the rollback fails.
    pub async fn rollback_to(&self, savepoint: &Savepoint) -> Result<()> {
        self.run(&format!("ROLLBACK TO SAVEPOINT {}", savepoint.name))
            .await
    }

    /// Keeps the work done since `savepoint` and forgets the savepoint.
    ///
    /// # Errors
    ///
    /// [`Error::Database`](crate::Error::Database) when the release fails.
    pub async fn release(&self, savepoint: Savepoint) -> Result<()> {
        match self.dialect() {
            Dialect::MySql => {
                self.run(&format!("RELEASE SAVEPOINT {}", savepoint.name))
                    .await
            }
            // Oracle has no RELEASE SAVEPOINT; savepoints end with the transaction.
            Dialect::Oracle => Ok(()),
        }
    }

    /// Fails while a cursor holds the connection: waiting for it would
    /// deadlock when the caller reads the cursor only after this statement.
    fn idle(&self) -> Result<()> {
        if self.cursor_open.load(Ordering::Acquire) {
            return Err(Error::InvalidQuery(
                "the transaction has an open cursor; read it to the end or drop it first".into(),
            ));
        }
        Ok(())
    }

    /// Runs a transaction-control statement (no bindings).
    async fn run(&self, sql: &str) -> Result<()> {
        self.idle()?;
        log::traced(log::statement(&self.peer, sql), async {
            match &self.conn {
                #[cfg(feature = "mysql")]
                Conn::MySql(conn) => conn.control(sql).await,
                #[cfg(feature = "oracle")]
                Conn::Oracle(conn) => conn.control(sql).await,
            }
        })
        .await
    }

    /// Records raw SQL about to run. MySQL resets the connection after every
    /// transaction anyway; Oracle drops the session at the end when `sql` may
    /// have changed it (see `oracle::changes_session`).
    #[cfg_attr(not(feature = "oracle"), allow(unused_variables))]
    pub(super) fn note_raw(&self, sql: &str) {
        match &self.conn {
            #[cfg(feature = "mysql")]
            Conn::MySql(_) => {}
            #[cfg(feature = "oracle")]
            Conn::Oracle(conn) => conn.note_raw(sql),
        }
    }
}

impl Drop for Transaction {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        match &self.conn {
            // The connection was marked for reset at `begin`; the pool resets
            // it on return, which rolls back the open transaction.
            #[cfg(feature = "mysql")]
            Conn::MySql(_) => {}
            #[cfg(feature = "oracle")]
            Conn::Oracle(conn) => conn.rollback_in_background(),
        }
    }
}

impl std::fmt::Debug for Transaction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction")
            .field("dialect", &self.dialect())
            .field("finished", &self.finished)
            .finish_non_exhaustive()
    }
}

impl sealed::Sealed for Transaction {}

impl Executor for Transaction {
    fn dialect(&self) -> Dialect {
        match self.conn {
            #[cfg(feature = "mysql")]
            Conn::MySql(_) => Dialect::MySql,
            #[cfg(feature = "oracle")]
            Conn::Oracle(_) => Dialect::Oracle,
        }
    }

    fn select(
        &self,
        sql: &str,
        binds: impl IntoIterator<Item = Value>,
    ) -> impl Future<Output = Result<Vec<Row>>> + Send {
        self.note_raw(sql);
        let compiled = grammar::raw(self.dialect(), sql, binds);
        async move { self.fetch_all(compiled?).await }
    }

    fn statement(
        &self,
        sql: &str,
        binds: impl IntoIterator<Item = Value>,
    ) -> impl Future<Output = Result<Executed>> + Send {
        self.note_raw(sql);
        let compiled = grammar::raw(self.dialect(), sql, binds);
        async move { self.execute(compiled?).await }
    }

    async fn fetch_map<T: Send + 'static>(
        &self,
        query: Compiled,
        f: impl FnMut(Row) -> Result<T> + Send + 'static,
    ) -> Result<Vec<T>> {
        self.idle()?;
        log::logged(&self.peer, query, |query| async move {
            match &self.conn {
                #[cfg(feature = "mysql")]
                Conn::MySql(conn) => conn.fetch_all(query, f).await,
                #[cfg(feature = "oracle")]
                Conn::Oracle(conn) => conn.fetch_all(query, f).await,
            }
        })
        .await
    }

    async fn execute(&self, query: Compiled) -> Result<Executed> {
        self.idle()?;
        log::logged(&self.peer, query, |query| async move {
            match &self.conn {
                #[cfg(feature = "mysql")]
                Conn::MySql(conn) => conn.execute(query).await,
                #[cfg(feature = "oracle")]
                Conn::Oracle(conn) => conn.execute(query).await,
            }
        })
        .await
    }

    fn cursor_buffered(&self, query: Compiled, buffer: usize) -> Cursor<'_> {
        if self.cursor_open.swap(true, Ordering::AcqRel) {
            return Cursor::failed(self.idle().unwrap_err());
        }
        // Entered while the driver spawns its task, which `Cursor` instruments with it.
        let _span = log::statement(&self.peer, &query.sql).entered();
        let mut cursor = match &self.conn {
            #[cfg(feature = "mysql")]
            Conn::MySql(conn) => conn.cursor(query, buffer),
            #[cfg(feature = "oracle")]
            Conn::Oracle(conn) => conn.cursor(query, buffer),
        };
        cursor.hold(&self.cursor_open);
        cursor
    }

    async fn execute_all(&self, queries: Vec<Compiled>) -> Result<Vec<Executed>> {
        let savepoint = self.savepoint().await?;
        let mut results = Vec::with_capacity(queries.len());
        for query in queries {
            match self.execute(query).await {
                Ok(executed) => results.push(executed),
                Err(error) => {
                    self.rollback_to(&savepoint).await?;
                    return Err(error);
                }
            }
        }
        self.release(savepoint).await?;
        Ok(results)
    }

    async fn fetch_each(&self, queries: Vec<Compiled>) -> Result<Vec<Vec<Row>>> {
        let savepoint = self.savepoint().await?;
        let mut results = Vec::with_capacity(queries.len());
        for query in queries {
            match self.fetch_all(query).await {
                Ok(rows) => results.push(rows),
                Err(error) => {
                    self.rollback_to(&savepoint).await?;
                    return Err(error);
                }
            }
        }
        self.release(savepoint).await?;
        Ok(results)
    }

    fn erase(&self) -> super::AnyExecutor<'_> {
        self.into()
    }
}
