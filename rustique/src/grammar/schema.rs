//! DDL: compiles a [`Blueprint`] into `CREATE TABLE`, `ALTER TABLE` and
//! index statements. DDL takes no bindings, so default values are literals.

use super::writer::Writer;
use super::{Dialect, mysql, oracle, raw};
use crate::grammar::Compiled;
use crate::schema::{
    Blueprint, Column, ColumnType, Command, DefaultExpr, ForeignKey, Index, IndexKind, OnDelete,
};
use crate::{Error, Result, Value};

pub(crate) fn blueprint(dialect: Dialect, t: &Blueprint) -> Result<Vec<String>> {
    let mut statements = Vec::new();
    if t.create {
        statements.push(statement(dialect, |w| create_table(w, t))?);
    } else {
        for column in &t.columns {
            let sql = if column.change {
                change_column(dialect, &t.table, column)?
            } else {
                statement(dialect, |w| {
                    alter(w, &t.table, " ADD ");
                    column_definition(w, column);
                })?
            };
            statements.push(sql);
        }
    }

    let derived: Vec<Command> = t.columns.iter().flat_map(column_commands).collect();
    for command in derived.iter().chain(&t.commands) {
        if let Command::Index(index) = command
            && index.kind == IndexKind::Primary
            && t.create
        {
            continue; // part of CREATE TABLE
        }
        statements.push(statement(dialect, |w| self::command(w, &t.table, command))?);
    }
    if dialect == Dialect::Oracle {
        // MySQL writes column comments into the column definition.
        for column in &t.columns {
            if let Some(text) = &column.comment {
                statements.push(statement(dialect, |w| {
                    w.push("COMMENT ON COLUMN ");
                    w.column(&format!("{}.{}", t.table, column.name));
                    w.push(" IS ");
                    w.push(&string(dialect, text));
                })?);
            }
        }
    }

    if let Some(column) =
        (t.columns.iter()).find(|c| c.change && (t.create || c.primary || c.auto_increment))
    {
        return Err(Error::InvalidQuery(format!(
            "column `{}`: `change` works in `Schema::table` and cannot change primary keys or auto-increment",
            column.name
        )));
    }
    if let Some(column) = t
        .columns
        .iter()
        .find(|c| c.on_delete != OnDelete::Restrict && c.references.is_none())
    {
        return Err(Error::InvalidQuery(format!(
            "column `{}` has an ON DELETE action but no foreign key; call `constrained` first",
            column.name
        )));
    }
    Ok(statements)
}

/// `DROP TABLE t`, or one that ignores a missing table.
pub(crate) fn drop_table(dialect: Dialect, table: &str, if_exists: bool) -> Result<String> {
    let drop = statement(dialect, |w| {
        w.push("DROP TABLE ");
        if if_exists && dialect == Dialect::MySql {
            w.push("IF EXISTS ");
        }
        w.table(table);
    })?;
    Ok(match (if_exists, dialect) {
        (true, Dialect::Oracle) => oracle::ignore_missing_table(&drop),
        _ => drop,
    })
}

/// `ALTER TABLE a RENAME TO b`, understood by both databases.
pub(crate) fn rename_table(dialect: Dialect, from: &str, to: &str) -> Result<String> {
    statement(dialect, |w| {
        alter(w, from, " RENAME TO ");
        w.table(to);
    })
}

/// A query returning one row when the table (and column, if given) exists.
pub(crate) fn has_table(dialect: Dialect, table: &str, column: Option<&str>) -> Result<Compiled> {
    let (schema, table) = match table.rsplit_once('.') {
        Some((schema, table)) => (Some(schema), table),
        None => (None, table),
    };
    let sql = match (dialect, column) {
        (Dialect::MySql, None) => mysql::HAS_TABLE,
        (Dialect::MySql, Some(_)) => mysql::HAS_COLUMN,
        (Dialect::Oracle, None) => oracle::HAS_TABLE,
        (Dialect::Oracle, Some(_)) => oracle::HAS_COLUMN,
    };
    // Oracle stores unquoted names upper-cased, and the grammar upper-cases every name.
    let name = |name: &str| match dialect {
        Dialect::MySql => Value::from(name),
        Dialect::Oracle => Value::from(name.to_uppercase()),
    };
    let binds = [
        Some(schema.map_or(Value::Null, name)),
        Some(name(table)),
        column.map(name),
    ];
    raw(dialect, sql, binds.into_iter().flatten())
}

/// The inverse of `mysql::column_type` / `oracle::column_type`: the
/// [`ColumnType`] of a type as the schema snapshot reports it (`varchar(50)`,
/// `bigint unsigned`, `NUMBER(19,0)`, `VARCHAR2(50)`), and whether it is
/// unsigned. `None` for a type no [`Blueprint`] method declares.
///
/// It renders each candidate through the forward mapping and compares, so the
/// two cannot drift apart; a new [`ColumnType`] only needs adding to the list.
pub(crate) fn parse_column_type(dialect: Dialect, reported: &str) -> Option<(ColumnType, bool)> {
    let reported = reported.trim().to_lowercase();
    let (reported, unsigned) = match reported.strip_suffix(" unsigned") {
        Some(rest) => (rest.to_owned(), true),
        None => (reported, false),
    };
    let args: Vec<u32> = reported
        .split_once('(')
        .map(|(_, args)| args)
        .unwrap_or_default()
        .split(|c: char| !c.is_ascii_digit())
        .filter_map(|n| n.parse().ok())
        .collect();
    let arg = |i: usize| args.get(i).copied().unwrap_or(0);
    let small = |i: usize| u8::try_from(arg(i)).unwrap_or(0);
    // Earlier candidates win where two types share SQL on Oracle: `text` over
    // `json` (CLOB), `string(n, 18)` over `time` (VARCHAR2(18 CHAR)).
    let candidates = [
        ColumnType::BigInteger,
        ColumnType::Integer,
        ColumnType::SmallInteger,
        ColumnType::Boolean,
        ColumnType::String(arg(0)),
        ColumnType::Char(arg(0)),
        ColumnType::Text,
        ColumnType::LongText,
        ColumnType::Double,
        ColumnType::Decimal(small(0), small(1)),
        ColumnType::Date,
        ColumnType::DateTime,
        ColumnType::Time,
        ColumnType::Json,
        ColumnType::Binary,
    ];
    // DDL `VARCHAR2(50 CHAR)`, `NUMBER(19)`, `DECIMAL(10, 2)` as the catalog reports them.
    let normalize = |sql: &str| {
        let sql = sql.to_lowercase().replace(" char)", ")").replace(' ', "");
        match (dialect, sql.strip_prefix("number(")) {
            (Dialect::Oracle, Some(rest)) if !rest.contains(',') => {
                format!("number({},0)", rest.trim_end_matches(')'))
            }
            _ => sql,
        }
    };
    let reported = normalize(&reported);
    let ty = candidates.into_iter().find(|&ty| {
        let sql = match dialect {
            Dialect::MySql => mysql::column_type(ty),
            Dialect::Oracle => oracle::column_type(ty),
        };
        normalize(&sql) == reported
    })?;
    Some((ty, unsigned))
}

fn statement(dialect: Dialect, write: impl FnOnce(&mut Writer)) -> Result<String> {
    let mut w = Writer::new(dialect);
    write(&mut w);
    Ok(w.finish()?.sql)
}

fn alter(w: &mut Writer, table: &str, action: &str) {
    w.push("ALTER TABLE ");
    w.table(table);
    w.push(action);
}

fn create_table(w: &mut Writer, t: &Blueprint) {
    w.push("CREATE TABLE ");
    w.table(&t.table);
    w.push(" (");
    w.list(&t.columns, column_definition);
    for command in &t.commands {
        if let Command::Index(index) = command
            && index.kind == IndexKind::Primary
        {
            w.push(", PRIMARY KEY ");
            columns(w, &index.columns);
        }
    }
    w.push(")");
}

/// MySQL: `ALTER TABLE t MODIFY <definition>`. Oracle: a PL/SQL block that
/// modifies type and default, then nullability only when it differs, since
/// Oracle rejects `NOT NULL` on a column that already is (ORA-01442).
fn change_column(dialect: Dialect, table: &str, c: &Column) -> Result<String> {
    if dialect == Dialect::MySql {
        return statement(dialect, |w| {
            alter(w, table, " MODIFY ");
            column_definition(w, c);
        });
    }
    let modify = statement(dialect, |w| {
        alter(w, table, " MODIFY (");
        w.column(&c.name);
        w.push(" ");
        w.push(&oracle::column_type(c.ty));
        w.push(" DEFAULT ");
        if !default(w, c) {
            w.push("NULL");
        }
        oracle::check(w, c);
        w.push(")");
    })?;
    let nullability = statement(dialect, |w| {
        alter(w, table, " MODIFY (");
        w.column(&c.name);
        w.push(if c.nullable { " NULL)" } else { " NOT NULL)" });
    })?;
    let quoted = |s: &str| s.replace('\'', "''");
    let (owner, name) = match table.rsplit_once('.') {
        Some((owner, name)) => (format!("'{}'", quoted(&owner.to_uppercase())), name),
        None => ("USER".to_owned(), table),
    };
    Ok(format!(
        "DECLARE n VARCHAR2(1); BEGIN EXECUTE IMMEDIATE '{}'; \
         SELECT nullable INTO n FROM all_tab_columns \
         WHERE owner = {owner} AND table_name = '{}' AND column_name = '{}'; \
         IF n = '{}' THEN EXECUTE IMMEDIATE '{}'; END IF; END;",
        quoted(&modify),
        quoted(&name.to_uppercase()),
        quoted(&c.name.to_uppercase()),
        if c.nullable { "N" } else { "Y" },
        quoted(&nullability),
    ))
}

/// `name TYPE [UNSIGNED] [AUTO_INCREMENT | identity] [DEFAULT x] [NOT NULL] [CHECK] [PRIMARY KEY]`
fn column_definition(w: &mut Writer, c: &Column) {
    w.column(&c.name);
    w.push(" ");
    let mysql = w.dialect == Dialect::MySql;
    if mysql {
        w.push(&mysql::column_type(c.ty));
        if c.unsigned {
            w.push(" UNSIGNED");
        }
    } else {
        w.push(&oracle::column_type(c.ty));
    }
    if c.auto_increment {
        w.push(if mysql {
            " AUTO_INCREMENT"
        } else {
            " GENERATED BY DEFAULT AS IDENTITY"
        });
    }
    if c.default.is_some() || c.default_expr.is_some() {
        w.push(" DEFAULT ");
        default(w, c);
    }
    if mysql && c.on_update_current {
        if c.ty == ColumnType::DateTime {
            w.push(" ON UPDATE CURRENT_TIMESTAMP(6)");
        } else {
            w.fail(Error::InvalidQuery(format!(
                "column `{}`: `use_current_on_update` needs a datetime column",
                c.name
            )));
        }
    }
    if !c.nullable {
        w.push(" NOT NULL");
    }
    if !mysql {
        oracle::check(w, c);
    }
    if c.primary {
        w.push(" PRIMARY KEY");
    }
    if let (true, Some(text)) = (mysql, &c.comment) {
        w.push(" COMMENT ");
        w.push(&string(w.dialect, text));
    }
}

/// The column's default after `DEFAULT`; `false` when it has none.
fn default(w: &mut Writer, c: &Column) -> bool {
    let mysql = w.dialect == Dialect::MySql;
    match (&c.default, &c.default_expr) {
        (Some(value), _) => {
            // MySQL accepts defaults for TEXT, JSON and BLOB only as expressions.
            let parens = mysql && mysql::default_needs_parens(c.ty);
            if parens {
                w.push("(");
            }
            literal(w, value);
            if parens {
                w.push(")");
            }
        }
        (None, Some(DefaultExpr::Raw(sql))) => w.push(sql),
        (None, Some(DefaultExpr::Current)) => {
            let sql = match (c.ty, mysql) {
                (ColumnType::DateTime, true) => "(UTC_TIMESTAMP(6))",
                (ColumnType::Date, true) => "(UTC_DATE())",
                (ColumnType::DateTime, false) => "SYS_EXTRACT_UTC(SYSTIMESTAMP)",
                (ColumnType::Date, false) => "TRUNC(SYS_EXTRACT_UTC(SYSTIMESTAMP))",
                _ => {
                    w.fail(Error::InvalidQuery(format!(
                        "column `{}`: `use_current` needs a date or datetime column",
                        c.name
                    )));
                    return true;
                }
            };
            w.push(sql);
        }
        (None, None) => return false,
    }
    true
}

/// Indexes and foreign keys declared through column modifiers.
fn column_commands(c: &Column) -> Vec<Command> {
    let index = |kind| {
        Command::Index(Index {
            kind,
            columns: vec![c.name.clone()],
            name: None,
        })
    };
    let mut commands = Vec::new();
    if c.unique {
        commands.push(index(IndexKind::Unique));
    }
    if c.index {
        commands.push(index(IndexKind::Index));
    }
    if let Some(on) = &c.references {
        commands.push(Command::Foreign(ForeignKey {
            columns: vec![c.name.clone()],
            on: on.clone(),
            references: vec!["id".to_owned()],
            on_delete: c.on_delete,
            name: None,
        }));
    }
    commands
}

fn command(w: &mut Writer, table: &str, command: &Command) {
    match command {
        Command::Index(index) if index.kind == IndexKind::Primary => {
            alter(w, table, " ADD PRIMARY KEY ");
            columns(w, &index.columns);
        }
        Command::Index(index) => {
            let unique = index.kind == IndexKind::Unique;
            w.push(if unique {
                "CREATE UNIQUE INDEX "
            } else {
                "CREATE INDEX "
            });
            let suffix = if unique { "unique" } else { "index" };
            w.column(&name(index.name.as_deref(), table, &index.columns, suffix));
            w.push(" ON ");
            w.table(table);
            w.push(" ");
            columns(w, &index.columns);
        }
        Command::Foreign(fk) => {
            alter(w, table, " ADD CONSTRAINT ");
            w.column(&name(fk.name.as_deref(), table, &fk.columns, "foreign"));
            w.push(" FOREIGN KEY ");
            columns(w, &fk.columns);
            w.push(" REFERENCES ");
            w.table(&fk.on);
            w.push(" ");
            columns(w, &fk.references);
            w.push(match fk.on_delete {
                OnDelete::Restrict => "",
                OnDelete::Cascade => " ON DELETE CASCADE",
                OnDelete::SetNull => " ON DELETE SET NULL",
            });
        }
        Command::DropColumn(column) => {
            alter(w, table, " DROP COLUMN ");
            w.column(column);
        }
        Command::RenameColumn(from, to) => {
            alter(w, table, " RENAME COLUMN ");
            w.column(from);
            w.push(" TO ");
            w.column(to);
        }
        Command::DropIndex(index) => match w.dialect {
            Dialect::MySql => {
                w.push("DROP INDEX ");
                w.column(index);
                w.push(" ON ");
                w.table(table);
            }
            Dialect::Oracle => {
                w.push("DROP INDEX ");
                w.column(index);
            }
        },
        Command::TableComment(text) => match w.dialect {
            Dialect::MySql => {
                alter(w, table, " COMMENT = ");
                w.push(&string(w.dialect, text));
            }
            Dialect::Oracle => {
                w.push("COMMENT ON TABLE ");
                w.table(table);
                w.push(" IS ");
                w.push(&string(w.dialect, text));
            }
        },
        Command::DropForeign(fk) => {
            alter(
                w,
                table,
                match w.dialect {
                    Dialect::MySql => " DROP FOREIGN KEY ",
                    Dialect::Oracle => " DROP CONSTRAINT ",
                },
            );
            w.column(fk);
        }
    }
}

fn columns(w: &mut Writer, names: &[String]) {
    w.push("(");
    w.list(names, |w, name| w.column(name));
    w.push(")");
}

/// Laravel's naming: `{table}_{col1}_{col2}_{suffix}`, schema prefix dropped.
fn name(name: Option<&str>, table: &str, columns: &[String], suffix: &str) -> String {
    if let Some(name) = name {
        return name.to_owned();
    }
    let table = table.rsplit('.').next().unwrap_or(table);
    format!("{table}_{}_{suffix}", columns.join("_"))
        .to_lowercase()
        .replace(['-', '.'], "_")
}

fn literal(w: &mut Writer, value: &Value) {
    let text = match value {
        Value::Null => "NULL".to_owned(),
        Value::Bool(b) => u8::from(*b).to_string(),
        Value::I64(n) => n.to_string(),
        Value::U64(n) => n.to_string(),
        Value::F64(n) if n.is_finite() => n.to_string(),
        Value::Decimal(d) => d.to_string(),
        Value::String(s) => string(w.dialect, s),
        Value::Json(j) => string(w.dialect, j),
        Value::Time(t) => string(w.dialect, &t.to_string()),
        Value::Date(d) => {
            let date = string(w.dialect, &d.to_string());
            match w.dialect {
                Dialect::MySql => date,
                Dialect::Oracle => format!("DATE {date}"),
            }
        }
        Value::DateTime(dt) => {
            let text = string(w.dialect, &dt.format("%Y-%m-%d %H:%M:%S%.6f").to_string());
            match w.dialect {
                Dialect::MySql => text,
                Dialect::Oracle => format!("TIMESTAMP {text}"),
            }
        }
        Value::F64(_) | Value::Bytes(_) => {
            w.fail(Error::InvalidQuery(format!(
                "a {} cannot be a column default",
                value.kind()
            )));
            return;
        }
    };
    w.push(&text);
}

/// Single-quoted string literal. MySQL also treats `\` as an escape character.
fn string(dialect: Dialect, s: &str) -> String {
    let s = s.replace('\'', "''");
    match dialect {
        Dialect::MySql => format!("'{}'", s.replace('\\', "\\\\")),
        Dialect::Oracle => format!("'{s}'"),
    }
}
