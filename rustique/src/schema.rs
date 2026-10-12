//! Schema builder: [`Schema`] runs DDL described by a [`Blueprint`].
//!
//! The SQL itself is written by `grammar/schema.rs`; this module holds the
//! builder types and runs the statements.

use crate::grammar::{self, Compiled};
use crate::{CREATED_AT, DELETED_AT, Db, Dialect, Executor, Result, UPDATED_AT, Value};

/// Creates, changes and drops tables, like Laravel's `Schema` facade.
///
/// Neither database runs DDL inside a transaction: each statement commits on
/// its own, so a failing [`create`](Self::create) can leave a table without
/// some of its indexes or foreign keys.
///
/// # Examples
///
/// ```no_run
/// # async fn run(db: &rustique::Db) -> rustique::Result<()> {
/// use rustique::Schema;
///
/// Schema::create(db, "users", |t| {
///     t.id();
///     t.string("email", 255).unique();
///     t.boolean("active").default(true);
///     t.timestamps();
/// })
/// .await?;
/// # Ok(()) }
/// ```
#[derive(Debug, Clone, Copy)]
pub struct Schema;

impl Schema {
    /// `CREATE TABLE`, then the indexes and foreign keys declared in `build`.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) for a malformed
    /// blueprint, database errors.
    pub async fn create(
        db: &impl Executor,
        table: &str,
        build: impl FnOnce(&mut Blueprint),
    ) -> Result<()> {
        let mut blueprint = Blueprint::create(table);
        build(&mut blueprint);
        run(db, blueprint.to_sql(db.dialect())?).await
    }

    /// Changes an existing table: adds the columns declared in `build`, then
    /// runs its other commands (drops, renames, indexes) in order.
    ///
    /// # Errors
    ///
    /// As [`create`](Self::create).
    pub async fn table(
        db: &impl Executor,
        table: &str,
        build: impl FnOnce(&mut Blueprint),
    ) -> Result<()> {
        let mut blueprint = Blueprint::alter(table);
        build(&mut blueprint);
        run(db, blueprint.to_sql(db.dialect())?).await
    }

    /// `DROP TABLE`.
    ///
    /// # Errors
    ///
    /// Database errors, including a missing table.
    pub async fn drop(db: &impl Executor, table: &str) -> Result<()> {
        run(db, vec![grammar::drop_table(db.dialect(), table, false)?]).await
    }

    /// `DROP TABLE` that ignores a missing table. On Oracle this is a PL/SQL
    /// block that swallows ORA-00942, since 19c has no `IF EXISTS`.
    ///
    /// # Errors
    ///
    /// Database errors other than a missing table.
    pub async fn drop_if_exists(db: &impl Executor, table: &str) -> Result<()> {
        run(db, vec![grammar::drop_table(db.dialect(), table, true)?]).await
    }

    /// Drops every table of the current database (MySQL) or the current
    /// user's schema (Oracle), like Laravel's `Schema::dropAllTables`, for
    /// `migrate:fresh`. Views and Oracle sequences stay.
    ///
    /// MySQL drops them in one statement with foreign key checks off, on one
    /// connection; Oracle drops them one by one with `CASCADE CONSTRAINTS PURGE`.
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn drop_all_tables(db: &Db) -> Result<()> {
        drop_all_tables(db, &[]).await
    }

    /// Renames a table.
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn rename(db: &impl Executor, from: &str, to: &str) -> Result<()> {
        run(db, vec![grammar::rename_table(db.dialect(), from, to)?]).await
    }

    /// Whether `table` (or `schema.table`) exists; without a schema, in the
    /// current database (MySQL) or the current user's schema (Oracle).
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn has_table(db: &impl Executor, table: &str) -> Result<bool> {
        let query = grammar::has_table(db.dialect(), table, None)?;
        // Never a lagging replica: migrations act on the answer.
        Ok(!db.erase().on_writer().fetch_all(query).await?.is_empty())
    }

    /// Whether `table` has `column`.
    ///
    /// # Errors
    ///
    /// Database errors.
    pub async fn has_column(db: &impl Executor, table: &str, column: &str) -> Result<bool> {
        let query = grammar::has_table(db.dialect(), table, Some(column))?;
        Ok(!db.erase().on_writer().fetch_all(query).await?.is_empty())
    }
}

/// [`Schema::drop_all_tables`], keeping the tables in `keep` (case-insensitive).
pub(crate) async fn drop_all_tables(db: &Db, keep: &[&str]) -> Result<()> {
    let db = &db.write();
    let sql = match db.dialect() {
        Dialect::MySql => {
            "SELECT table_name AS name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE'"
        }
        // Not the recycle bin, nested tables or index-organized overflow segments.
        Dialect::Oracle => {
            "SELECT table_name AS name FROM user_tables WHERE dropped = 'NO' \
             AND nested = 'NO' AND secondary = 'N' \
             AND (iot_type IS NULL OR iot_type <> 'IOT_OVERFLOW')"
        }
    };
    let mut names: Vec<String> = Vec::new();
    for row in db.select(sql, []).await?.iter() {
        let name: String = row.get("name")?;
        if !keep.iter().any(|k| k.eq_ignore_ascii_case(&name)) {
            names.push(name);
        }
    }
    if names.is_empty() {
        return Ok(());
    }
    // Names as the database reports them, quoted exactly (the grammar
    // would upper-case them on Oracle).
    let quote = |name: &str, q: char| format!("{q}{}{q}", name.replace(q, &format!("{q}{q}")));
    match db.dialect() {
        Dialect::MySql => {
            let list: Vec<String> = names.iter().map(|n| quote(n, '`')).collect();
            // A transaction pins the connection the session variable is set on.
            let tx = db.begin().await?;
            tx.statement("SET FOREIGN_KEY_CHECKS = 0", []).await?;
            let dropped = tx
                .statement(&format!("DROP TABLE {}", list.join(", ")), [])
                .await;
            tx.statement("SET FOREIGN_KEY_CHECKS = 1", []).await?;
            tx.commit().await?;
            dropped?;
        }
        Dialect::Oracle => {
            for name in &names {
                let sql = format!("DROP TABLE {} CASCADE CONSTRAINTS PURGE", quote(name, '"'));
                db.statement(&sql, []).await?;
            }
        }
    }
    Ok(())
}

/// Name of the schema snapshot file that `#[rustique::model]` reads with the
/// `schema-check` feature, next to the crate's `Cargo.toml`.
pub(crate) const SNAPSHOT_FILE: &str = "rustique-schema.txt";

/// The schema snapshot of the current database (MySQL) or user (Oracle): a
/// `dialect` line, then one tab-separated `table, column, null / not null,
/// type` line per column. `rustique-macros/src/schema_check.rs` reads it.
pub(crate) async fn snapshot(db: &Db) -> Result<String> {
    let db = &db.write();
    let sql = match db.dialect() {
        Dialect::MySql => {
            "SELECT table_name AS tbl, column_name AS col, is_nullable AS nullable, \
             column_type AS ty FROM information_schema.columns \
             WHERE table_schema = DATABASE() ORDER BY table_name, ordinal_position"
        }
        // `NUMBER(p,s)` and `VARCHAR2(n)` as written in DDL; `NUMBER(*,0)` for `INTEGER`.
        Dialect::Oracle => {
            "SELECT table_name AS tbl, column_name AS col, nullable, data_type || CASE \
             WHEN data_type = 'NUMBER' AND data_scale IS NOT NULL \
             THEN '(' || NVL(TO_CHAR(data_precision), '*') || ',' || data_scale || ')' \
             WHEN char_used IS NOT NULL THEN '(' || char_length || ')' END AS ty \
             FROM user_tab_columns WHERE table_name NOT LIKE 'BIN$%' \
             ORDER BY table_name, column_id"
        }
    };
    let mut columns = Vec::new();
    for row in db.select(sql, []).await?.iter() {
        let nullable: String = row.get("nullable")?;
        let nullable = if nullable.starts_with('Y') {
            "null"
        } else {
            "not null"
        };
        columns.push([
            row.get("tbl")?,
            row.get("col")?,
            nullable.to_owned(),
            row.get("ty")?,
        ]);
    }
    Ok(snapshot_text(db.dialect(), &columns))
}

fn snapshot_text(dialect: Dialect, columns: &[[String; 4]]) -> String {
    let dialect = match dialect {
        Dialect::MySql => "mysql",
        Dialect::Oracle => "oracle",
    };
    let mut text = format!(
        "# Schema snapshot for #[rustique::model]'s compile-time check (cargo feature `schema-check`).\n\
         # Written by the `schema:snapshot` command; run it again after migrating.\n\
         dialect\t{dialect}\n"
    );
    for column in columns {
        text.push_str(&column.join("\t"));
        text.push('\n');
    }
    text
}

async fn run(db: &impl Executor, statements: Vec<String>) -> Result<()> {
    for sql in statements {
        let query = Compiled {
            sql,
            binds: Vec::new(),
            batch: None,
            returning: 0,
        };
        db.execute(query).await?;
    }
    Ok(())
}

/// The columns and commands of one [`Schema::create`] or [`Schema::table`] call.
///
/// Columns are `NOT NULL` unless marked [`nullable`](Column::nullable).
#[derive(Debug, Clone)]
pub struct Blueprint {
    pub(crate) table: String,
    pub(crate) create: bool,
    pub(crate) columns: Vec<Column>,
    pub(crate) commands: Vec<Command>,
}

impl Blueprint {
    /// A blueprint for a new table.
    #[must_use]
    pub fn create(table: &str) -> Self {
        Self {
            table: table.to_owned(),
            create: true,
            columns: Vec::new(),
            commands: Vec::new(),
        }
    }

    /// A blueprint that changes an existing table.
    #[must_use]
    pub fn alter(table: &str) -> Self {
        Self {
            create: false,
            ..Self::create(table)
        }
    }

    /// The statements this blueprint runs, in order.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidQuery`](crate::Error::InvalidQuery) for a default value
    /// that cannot be written as SQL, `cascade_on_delete` without `constrained`,
    /// or an invalid identifier.
    pub fn to_sql(&self, dialect: Dialect) -> Result<Vec<String>> {
        grammar::blueprint(dialect, self)
    }

    fn add(&mut self, name: &str, ty: ColumnType) -> &mut Column {
        self.columns.push(Column::new(name, ty));
        self.columns.last_mut().expect("column was just pushed")
    }

    /// Auto-increment `id` primary key: `BIGINT UNSIGNED AUTO_INCREMENT` on
    /// MySQL, an identity `NUMBER(19)` on Oracle.
    pub fn id(&mut self) -> &mut Column {
        self.big_increments("id")
    }

    /// Auto-increment big integer primary key named `name`.
    pub fn big_increments(&mut self, name: &str) -> &mut Column {
        let column = self.big_integer(name).unsigned().primary();
        column.auto_increment = true;
        column
    }

    /// `BIGINT` / `NUMBER(19)`.
    pub fn big_integer(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::BigInteger)
    }

    /// `INT` / `NUMBER(10)`.
    pub fn integer(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Integer)
    }

    /// `SMALLINT` / `NUMBER(5)`.
    pub fn small_integer(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::SmallInteger)
    }

    /// Unsigned big integer for a key referencing an [`id`](Self::id) column;
    /// add the constraint with [`constrained`](Column::constrained).
    pub fn foreign_id(&mut self, name: &str) -> &mut Column {
        self.big_integer(name).unsigned()
    }

    /// `TINYINT(1)` / `NUMBER(1)` with `CHECK (name IN (0, 1))`.
    pub fn boolean(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Boolean)
    }

    /// `VARCHAR(length)` / `VARCHAR2(length CHAR)`. Oracle caps it at 4000
    /// bytes unless `MAX_STRING_SIZE=EXTENDED`.
    pub fn string(&mut self, name: &str, length: u32) -> &mut Column {
        self.add(name, ColumnType::String(length))
    }

    /// Fixed-length `CHAR(length)` / `CHAR(length CHAR)`.
    pub fn char(&mut self, name: &str, length: u32) -> &mut Column {
        self.add(name, ColumnType::Char(length))
    }

    /// `TEXT` / `CLOB`.
    pub fn text(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Text)
    }

    /// `LONGTEXT` / `CLOB`.
    pub fn long_text(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::LongText)
    }

    /// `DOUBLE` / `BINARY_DOUBLE`.
    pub fn double(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Double)
    }

    /// `DECIMAL(precision, scale)` / `NUMBER(precision, scale)`.
    pub fn decimal(&mut self, name: &str, precision: u8, scale: u8) -> &mut Column {
        self.add(name, ColumnType::Decimal(precision, scale))
    }

    /// `DATE` on both databases (Oracle's `DATE` also holds a time, unused here).
    pub fn date(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Date)
    }

    /// `DATETIME(6)` / `TIMESTAMP(6)`, microsecond precision.
    pub fn datetime(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::DateTime)
    }

    /// `JSON` / `CLOB` with `CHECK (name IS JSON)`.
    pub fn json(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Json)
    }

    /// `LONGBLOB` / `BLOB`.
    pub fn binary(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Binary)
    }

    /// `CHAR(36)` on both databases.
    pub fn uuid(&mut self, name: &str) -> &mut Column {
        self.add(name, ColumnType::Uuid)
    }

    /// `CHAR(26)` on both databases, for the keys of `#[rustique::model(ulid)]` models.
    pub fn ulid(&mut self, name: &str) -> &mut Column {
        self.char(name, 26)
    }

    /// Nullable `created_at` and `updated_at` datetime columns.
    pub fn timestamps(&mut self) {
        self.datetime(CREATED_AT).nullable();
        self.datetime(UPDATED_AT).nullable();
    }

    /// Nullable `deleted_at` datetime column.
    pub fn soft_deletes(&mut self) {
        self.datetime(DELETED_AT).nullable();
    }

    fn index_command(&mut self, kind: IndexKind, columns: &[&str]) -> &mut Index {
        self.commands.push(Command::Index(Index {
            kind,
            columns: columns.iter().map(|&c| c.to_owned()).collect(),
            name: None,
        }));
        let Some(Command::Index(index)) = self.commands.last_mut() else {
            unreachable!("an index was just pushed")
        };
        index
    }

    /// Primary key over several columns.
    pub fn primary(&mut self, columns: &[&str]) -> &mut Index {
        self.index_command(IndexKind::Primary, columns)
    }

    /// Unique index, named `{table}_{columns}_unique` unless [`named`](Index::name).
    pub fn unique(&mut self, columns: &[&str]) -> &mut Index {
        self.index_command(IndexKind::Unique, columns)
    }

    /// Plain index, named `{table}_{columns}_index` unless [`named`](Index::name).
    pub fn index(&mut self, columns: &[&str]) -> &mut Index {
        self.index_command(IndexKind::Index, columns)
    }

    /// Foreign key `columns` referencing `references` of table `on`, named
    /// `{table}_{columns}_foreign` unless [`named`](ForeignKey::name).
    pub fn foreign(&mut self, columns: &[&str], on: &str, references: &[&str]) -> &mut ForeignKey {
        let owned = |list: &[&str]| list.iter().map(|&c| c.to_owned()).collect();
        self.commands.push(Command::Foreign(ForeignKey {
            columns: owned(columns),
            on: on.to_owned(),
            references: owned(references),
            on_delete: OnDelete::Restrict,
            name: None,
        }));
        let Some(Command::Foreign(foreign)) = self.commands.last_mut() else {
            unreachable!("a foreign key was just pushed")
        };
        foreign
    }

    /// Sets the table's comment: MySQL `ALTER TABLE t COMMENT = '...'`,
    /// Oracle `COMMENT ON TABLE t IS '...'`.
    pub fn comment(&mut self, text: &str) {
        self.commands.push(Command::TableComment(text.to_owned()));
    }

    /// Drops a column.
    pub fn drop_column(&mut self, name: &str) {
        self.commands.push(Command::DropColumn(name.to_owned()));
    }

    /// Renames a column.
    pub fn rename_column(&mut self, from: &str, to: &str) {
        self.commands
            .push(Command::RenameColumn(from.to_owned(), to.to_owned()));
    }

    /// Drops an index or unique index by name.
    pub fn drop_index(&mut self, name: &str) {
        self.commands.push(Command::DropIndex(name.to_owned()));
    }

    /// Drops a foreign key by name.
    pub fn drop_foreign(&mut self, name: &str) {
        self.commands.push(Command::DropForeign(name.to_owned()));
    }
}

/// One column of a [`Blueprint`]; its methods chain: `t.string("email", 255).nullable().unique()`.
#[derive(Debug, Clone)]
pub struct Column {
    pub(crate) name: String,
    pub(crate) ty: ColumnType,
    pub(crate) nullable: bool,
    pub(crate) default: Option<Value>,
    pub(crate) default_expr: Option<DefaultExpr>,
    pub(crate) comment: Option<String>,
    pub(crate) unsigned: bool,
    pub(crate) auto_increment: bool,
    pub(crate) primary: bool,
    pub(crate) unique: bool,
    pub(crate) index: bool,
    pub(crate) references: Option<String>,
    pub(crate) on_delete: OnDelete,
    pub(crate) change: bool,
}

impl Column {
    fn new(name: &str, ty: ColumnType) -> Self {
        Self {
            name: name.to_owned(),
            ty,
            nullable: false,
            default: None,
            default_expr: None,
            comment: None,
            unsigned: false,
            auto_increment: false,
            primary: false,
            unique: false,
            index: false,
            references: None,
            on_delete: OnDelete::Restrict,
            change: false,
        }
    }

    /// Changes this existing column to the declared definition instead of
    /// adding it, like Laravel's `->change()`, inside [`Schema::table`].
    /// State the whole definition: a modifier left out is removed, so a
    /// column that should stay nullable needs [`nullable`](Self::nullable)
    /// again, and one without [`default`](Self::default) loses its default.
    /// [`unique`](Self::unique), [`index`](Self::index) and
    /// [`constrained`](Self::constrained) add a new index or key.
    ///
    /// MySQL runs `ALTER TABLE t MODIFY ...`. Oracle runs `MODIFY` for the
    /// type and default, then changes `NULL` / `NOT NULL` only when it
    /// differs (Oracle rejects a no-op change), in one PL/SQL block.
    /// Primary keys and auto-increment cannot be changed this way.
    pub fn change(&mut self) -> &mut Self {
        self.change = true;
        self
    }

    /// Allows `NULL`.
    pub fn nullable(&mut self) -> &mut Self {
        self.nullable = true;
        self
    }

    /// `DEFAULT value`, written as a SQL literal.
    pub fn default(&mut self, value: impl Into<Value>) -> &mut Self {
        self.default = Some(value.into());
        self.default_expr = None;
        self
    }

    /// `DEFAULT sql`, with `sql` written as given, like Laravel's
    /// `default(DB::raw(...))`. MySQL needs expressions other than literals
    /// and `CURRENT_TIMESTAMP` in parentheses: `default_raw("(UUID())")`.
    pub fn default_raw(&mut self, sql: &str) -> &mut Self {
        self.default_expr = Some(DefaultExpr::Raw(sql.to_owned()));
        self.default = None;
        self
    }

    /// Defaults a [`datetime`](Blueprint::datetime) or
    /// [`date`](Blueprint::date) column to the current time, like Laravel's
    /// `useCurrent`. In UTC, even when [`set_timezone`](crate::set_timezone)
    /// makes the models write another zone: MySQL
    /// `(UTC_TIMESTAMP(6))` / `(UTC_DATE())`, Oracle
    /// `SYS_EXTRACT_UTC(SYSTIMESTAMP)` (truncated for dates).
    pub fn use_current(&mut self) -> &mut Self {
        self.default_expr = Some(DefaultExpr::Current);
        self.default = None;
        self
    }

    /// Column comment: MySQL `COMMENT '...'`, Oracle `COMMENT ON COLUMN`.
    pub fn comment(&mut self, text: &str) -> &mut Self {
        self.comment = Some(text.to_owned());
        self
    }

    /// `UNSIGNED` on MySQL; Oracle has no unsigned numbers and ignores it.
    pub fn unsigned(&mut self) -> &mut Self {
        self.unsigned = true;
        self
    }

    /// Makes this column the primary key.
    pub fn primary(&mut self) -> &mut Self {
        self.primary = true;
        self
    }

    /// Adds a unique index on this column.
    pub fn unique(&mut self) -> &mut Self {
        self.unique = true;
        self
    }

    /// Adds an index on this column.
    pub fn index(&mut self) -> &mut Self {
        self.index = true;
        self
    }

    /// Adds a foreign key to `table.id`.
    pub fn constrained(&mut self, table: &str) -> &mut Self {
        self.references = Some(table.to_owned());
        self
    }

    /// `ON DELETE CASCADE` for the [`constrained`](Self::constrained) foreign key.
    pub fn cascade_on_delete(&mut self) -> &mut Self {
        self.on_delete = OnDelete::Cascade;
        self
    }

    /// `ON DELETE SET NULL` for the [`constrained`](Self::constrained) foreign key.
    pub fn null_on_delete(&mut self) -> &mut Self {
        self.on_delete = OnDelete::SetNull;
        self
    }
}

/// An index from [`Blueprint::primary`], [`unique`](Blueprint::unique) or
/// [`index`](Blueprint::index).
#[derive(Debug, Clone)]
pub struct Index {
    pub(crate) kind: IndexKind,
    pub(crate) columns: Vec<String>,
    pub(crate) name: Option<String>,
}

impl Index {
    /// Overrides the generated name. Ignored for primary keys.
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.to_owned());
        self
    }
}

/// A foreign key from [`Blueprint::foreign`]. Oracle supports no `ON UPDATE`
/// actions, so there are none here.
#[derive(Debug, Clone)]
pub struct ForeignKey {
    pub(crate) columns: Vec<String>,
    pub(crate) on: String,
    pub(crate) references: Vec<String>,
    pub(crate) on_delete: OnDelete,
    pub(crate) name: Option<String>,
}

impl ForeignKey {
    /// Overrides the generated name.
    pub fn name(&mut self, name: &str) -> &mut Self {
        self.name = Some(name.to_owned());
        self
    }

    /// `ON DELETE CASCADE`.
    pub fn cascade_on_delete(&mut self) -> &mut Self {
        self.on_delete = OnDelete::Cascade;
        self
    }

    /// `ON DELETE SET NULL`.
    pub fn null_on_delete(&mut self) -> &mut Self {
        self.on_delete = OnDelete::SetNull;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ColumnType {
    BigInteger,
    Integer,
    SmallInteger,
    Boolean,
    String(u32),
    Char(u32),
    Text,
    LongText,
    Double,
    Decimal(u8, u8),
    Date,
    DateTime,
    Json,
    Binary,
    Uuid,
}

/// A column default that is SQL rather than a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DefaultExpr {
    Raw(String),
    /// The current UTC time or date.
    Current,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexKind {
    Primary,
    Unique,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OnDelete {
    Restrict,
    Cascade,
    SetNull,
}

#[derive(Debug, Clone)]
pub(crate) enum Command {
    Index(Index),
    Foreign(ForeignKey),
    DropColumn(String),
    RenameColumn(String, String),
    DropIndex(String),
    DropForeign(String),
    TableComment(String),
}
