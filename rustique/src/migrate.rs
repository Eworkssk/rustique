//! Migrations: [`Migration`], [`Migrator`] and the [`migrations!`](crate::migrations) macro.

use std::time::Duration;

use crate::grammar;
use crate::{BoxFuture, Db, Dialect, Error, Executor, QueryBuilder, Result, Schema, Transaction};

/// Default table that records which migrations have run, as in Laravel.
pub const MIGRATIONS_TABLE: &str = "migrations";

/// Default MySQL lock name, or Oracle table, that serializes migration runs.
const LOCK: &str = "migrations_lock";

/// `up` or `down` of a [`Migration`].
pub type MigrationFn = for<'a> fn(&'a Db) -> BoxFuture<'a, Result<()>>;

/// One schema change. Build them with [`migrations!`](crate::migrations).
#[derive(Debug, Clone, Copy)]
pub struct Migration {
    name: &'static str,
    up: MigrationFn,
    down: MigrationFn,
}

impl Migration {
    /// A migration called `name` (its key in the migrations table).
    #[must_use]
    pub const fn new(name: &'static str, up: MigrationFn, down: MigrationFn) -> Self {
        Self { name, up, down }
    }

    /// The name recorded in the migrations table.
    #[must_use]
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

/// Builds a `Vec<Migration>` from modules that each define
/// `pub async fn up(db: &Db) -> rustique::Result<()>` and a matching `down`.
/// A migration's name is its module name; they run in the listed order.
///
/// ```ignore
/// mod m2026_10_10_000001_create_users {
///     use rustique::{Db, Schema};
///
///     pub async fn up(db: &Db) -> rustique::Result<()> {
///         Schema::create(db, "users", |t| {
///             t.id();
///             t.string("email", 255).unique();
///         })
///         .await
///     }
///
///     pub async fn down(db: &Db) -> rustique::Result<()> {
///         Schema::drop(db, "users").await
///     }
/// }
///
/// let migrator = rustique::Migrator::new(rustique::migrations![m2026_10_10_000001_create_users]);
/// ```
#[macro_export]
macro_rules! migrations {
    ($($module:ident),* $(,)?) => {
        ::std::vec![$({
            fn up(db: &$crate::Db) -> $crate::BoxFuture<'_, $crate::Result<()>> {
                ::std::boxed::Box::pin($module::up(db))
            }
            fn down(db: &$crate::Db) -> $crate::BoxFuture<'_, $crate::Result<()>> {
                ::std::boxed::Box::pin($module::down(db))
            }
            $crate::Migration::new(::std::stringify!($module), up, down)
        }),*]
    };
}

/// Whether a migration has run, from [`Migrator::status`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MigrationStatus {
    /// Migration name.
    pub name: &'static str,
    /// Batch it ran in, `None` when pending.
    pub batch: Option<i64>,
}

/// Runs and rolls back [`Migration`]s, tracked in the [`MIGRATIONS_TABLE`]
/// unless [`table`](Self::table) says otherwise.
///
/// DDL commits on its own on both databases, so a migration cannot be undone
/// by a transaction: each one is recorded right after it succeeds, and a
/// failure stops the run with the earlier migrations recorded. A migration
/// that fails halfway leaves its partial changes behind.
///
/// `migrate` and `rollback` take a database lock first, so processes started
/// together (several deploys, say) run them one after another: MySQL's
/// `GET_LOCK('migrations_lock')`, or on Oracle a lock on a `migrations_lock`
/// table that the first run creates. They wait for it without limit unless
/// [`lock_timeout`](Self::lock_timeout) is set.
#[derive(Debug, Clone)]
pub struct Migrator {
    migrations: Vec<Migration>,
    table: &'static str,
    lock: &'static str,
    lock_timeout: Option<Duration>,
    seeder: Option<MigrationFn>,
}

/// Which migrations [`Migrator::rollback_locked`] undoes.
#[derive(Debug, Clone, Copy)]
enum Rollback {
    LastBatch,
    Steps(usize),
    All,
}

impl Migrator {
    /// A migrator for `migrations`, which run in this order.
    ///
    /// # Panics
    ///
    /// When two migrations share a name.
    #[must_use]
    pub fn new(migrations: Vec<Migration>) -> Self {
        for (i, migration) in migrations.iter().enumerate() {
            assert!(
                migrations[..i].iter().all(|m| m.name != migration.name),
                "duplicate migration `{}`",
                migration.name
            );
        }
        Self {
            migrations,
            table: MIGRATIONS_TABLE,
            lock: LOCK,
            lock_timeout: None,
            seeder: None,
        }
    }

    /// Registers the seeder that [`seed`](Self::seed), `db:seed` and
    /// `migrate:fresh --seed` run, like Laravel's `DatabaseSeeder`. It is a
    /// function like a migration's `up`; call other seeders from it:
    ///
    /// ```
    /// use rustique::{BoxFuture, Db, Migrator};
    ///
    /// fn seed(db: &Db) -> BoxFuture<'_, rustique::Result<()>> {
    ///     Box::pin(async move {
    ///         // UserFactory::new().count(10).create(db).await?; ...
    ///         let _ = db;
    ///         Ok(())
    ///     })
    /// }
    ///
    /// let migrator = Migrator::new(Vec::new()).seeder(seed);
    /// ```
    #[must_use]
    pub fn seeder(mut self, seeder: MigrationFn) -> Self {
        self.seeder = Some(seeder);
        self
    }

    /// Runs the [`seeder`](Self::seeder).
    ///
    /// # Errors
    ///
    /// [`Error::Config`] without a seeder, otherwise the seeder's errors.
    pub async fn seed(&self, db: &Db) -> Result<()> {
        let seeder = self.seeder.ok_or_else(|| {
            Error::Config("no seeder registered; see `Migrator::seeder`".to_owned())
        })?;
        seeder(db).await
    }

    /// Records migrations in `table` instead of `migrations`. It needs the
    /// columns `id`, `migration` and `batch`; `migrate` creates it if missing.
    #[must_use]
    pub fn table(mut self, table: &'static str) -> Self {
        self.table = table;
        self
    }

    /// Locks on `name` instead of `migrations_lock`: the MySQL `GET_LOCK`
    /// name, or the Oracle lock table.
    #[must_use]
    pub fn lock_name(mut self, name: &'static str) -> Self {
        self.lock = name;
        self
    }

    /// Fails `migrate` and `rollback` when another run holds the lock for
    /// longer than `timeout` (whole seconds), instead of waiting for it.
    #[must_use]
    pub fn lock_timeout(mut self, timeout: Duration) -> Self {
        self.lock_timeout = Some(timeout);
        self
    }

    /// Runs every pending migration as one new batch; returns their names.
    ///
    /// # Errors
    ///
    /// The first failing migration's error; later ones do not run.
    pub async fn migrate(&self, db: &Db) -> Result<Vec<&'static str>> {
        let db = &db.write(); // never a lagging read replica
        let lock = self.lock(db).await?;
        let done = self.migrate_locked(db).await;
        let unlocked = self.unlock(lock).await;
        let done = done?;
        unlocked?;
        Ok(done)
    }

    async fn migrate_locked(&self, db: &Db) -> Result<Vec<&'static str>> {
        if !Schema::has_table(db, self.table).await? {
            Schema::create(db, self.table, |t| {
                t.id();
                t.string("migration", 255);
                t.integer("batch");
            })
            .await?;
        }
        let ran = self.ran(db).await?;
        let batch = ran.iter().map(|r| r.batch).max().unwrap_or(0) + 1;
        let mut done = Vec::new();
        for migration in &self.migrations {
            if ran.iter().any(|r| r.name == migration.name) {
                continue;
            }
            (migration.up)(db).await?;
            QueryBuilder::table(self.table)
                .insert(
                    db,
                    &["migration", "batch"],
                    vec![vec![migration.name.into(), batch.into()]],
                )
                .await?;
            done.push(migration.name);
        }
        Ok(done)
    }

    /// Runs `down` of every migration in the last batch, newest first; returns their names.
    ///
    /// # Errors
    ///
    /// [`Error::MigrationNotFound`] when the batch holds a migration that is
    /// not registered; otherwise the first failing migration's error.
    pub async fn rollback(&self, db: &Db) -> Result<Vec<&'static str>> {
        self.rollback_with(db, Rollback::LastBatch).await
    }

    /// Runs `down` of the last `steps` migrations, newest first, whatever
    /// their batch, like `migrate:rollback --step=N`; returns their names.
    ///
    /// # Errors
    ///
    /// As [`rollback`](Self::rollback).
    pub async fn rollback_steps(&self, db: &Db, steps: usize) -> Result<Vec<&'static str>> {
        self.rollback_with(db, Rollback::Steps(steps)).await
    }

    /// Runs `down` of every migration that has run, newest first, like
    /// `migrate:reset`; returns their names.
    ///
    /// # Errors
    ///
    /// As [`rollback`](Self::rollback).
    pub async fn reset(&self, db: &Db) -> Result<Vec<&'static str>> {
        self.rollback_with(db, Rollback::All).await
    }

    /// Drops every table (see [`Schema::drop_all_tables`]), then runs every
    /// migration, like `migrate:fresh`; `down` functions do not run. Returns
    /// the names of the migrations run. Meant for development and tests: all
    /// data is lost.
    ///
    /// # Errors
    ///
    /// As [`Schema::drop_all_tables`] and [`migrate`](Self::migrate).
    pub async fn fresh(&self, db: &Db) -> Result<Vec<&'static str>> {
        let db = &db.write(); // never a lagging read replica
        let lock = self.lock(db).await?;
        // The Oracle lock table is held by `lock`; dropping it would wait on it.
        let done = match crate::schema::drop_all_tables(db, &[self.lock]).await {
            Ok(()) => self.migrate_locked(db).await,
            Err(error) => Err(error),
        };
        let unlocked = self.unlock(lock).await;
        let done = done?;
        unlocked?;
        Ok(done)
    }

    async fn rollback_with(&self, db: &Db, which: Rollback) -> Result<Vec<&'static str>> {
        let db = &db.write(); // never a lagging read replica
        let lock = self.lock(db).await?;
        let done = self.rollback_locked(db, which).await;
        let unlocked = self.unlock(lock).await;
        let done = done?;
        unlocked?;
        Ok(done)
    }

    async fn rollback_locked(&self, db: &Db, which: Rollback) -> Result<Vec<&'static str>> {
        let ran = self.ran(db).await?;
        let Some(batch) = ran.iter().map(|r| r.batch).max() else {
            return Ok(Vec::new());
        };
        let records: Vec<&Record> = match which {
            Rollback::LastBatch => ran.iter().rev().filter(|r| r.batch == batch).collect(),
            Rollback::Steps(steps) => ran.iter().rev().take(steps).collect(),
            Rollback::All => ran.iter().rev().collect(),
        };
        let mut done = Vec::new();
        for record in records {
            let migration = self
                .migrations
                .iter()
                .find(|m| m.name == record.name)
                .ok_or_else(|| Error::MigrationNotFound(record.name.clone()))?;
            (migration.down)(db).await?;
            QueryBuilder::table(self.table)
                .where_("id", "=", record.id)
                .delete(db)
                .await?;
            done.push(migration.name);
        }
        Ok(done)
    }

    /// Every registered migration with the batch it ran in.
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn status(&self, db: &Db) -> Result<Vec<MigrationStatus>> {
        let db = &db.write(); // never a lagging read replica
        let ran = self.ran(db).await?;
        Ok(self
            .migrations
            .iter()
            .map(|m| MigrationStatus {
                name: m.name,
                batch: ran.iter().find(|r| r.name == m.name).map(|r| r.batch),
            })
            .collect())
    }
}

struct Record {
    id: i64,
    name: String,
    batch: i64,
}

impl Migrator {
    /// Waits for the migration lock, held by a transaction on its own
    /// connection until [`unlock`](Self::unlock); dropping the transaction releases it too.
    /// Oracle grants `DBMS_LOCK` to no one by default, hence the lock table.
    async fn lock(&self, db: &Db) -> Result<Transaction> {
        let tx = db.begin().await?;
        let wait = self.lock_timeout.map(|t| t.as_secs());
        match db.dialect() {
            Dialect::MySql => {
                let rows = tx
                    .select(
                        "SELECT GET_LOCK(?, ?) AS locked",
                        [
                            self.lock.into(),
                            wait.map_or(-1, |s| i64::try_from(s).unwrap_or(i64::MAX))
                                .into(),
                        ],
                    )
                    .await?;
                let locked: Option<i64> = rows.first().map_or(Ok(None), |r| r.get("locked"))?;
                if locked != Some(1) {
                    return Err(Error::Database {
                        dialect: Dialect::MySql,
                        code: None,
                        message: format!("GET_LOCK('{}') failed or timed out", self.lock),
                    });
                }
            }
            Dialect::Oracle => {
                if !Schema::has_table(db, self.lock).await? {
                    let created = Schema::create(db, self.lock, |t| {
                        t.id();
                    })
                    .await;
                    // A run starting at the same time may have created it first.
                    if created.is_err() && !Schema::has_table(db, self.lock).await? {
                        created?;
                    }
                }
                tx.statement(&grammar::oracle_lock_table(self.lock, wait)?, [])
                    .await?;
            }
        }
        Ok(tx)
    }

    async fn unlock(&self, tx: Transaction) -> Result<()> {
        if tx.dialect() == Dialect::MySql {
            tx.select("SELECT RELEASE_LOCK(?) AS released", [self.lock.into()])
                .await?;
        }
        tx.rollback().await
    }

    /// Recorded migrations in the order they ran; none before the first `migrate`.
    async fn ran(&self, db: &Db) -> Result<Vec<Record>> {
        if !Schema::has_table(db, self.table).await? {
            return Ok(Vec::new());
        }
        let rows = QueryBuilder::table(self.table)
            .order_by("id")
            .get(db)
            .await?;
        rows.iter()
            .map(|row| {
                Ok(Record {
                    id: row.get("id")?,
                    name: row.get("migration")?,
                    batch: row.get("batch")?,
                })
            })
            .collect()
    }
}
