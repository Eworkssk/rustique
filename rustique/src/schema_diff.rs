//! `make:migration <name> --diff`: a migration whose `up` turns the live
//! schema into the one in the schema snapshot file, and whose `down` turns it
//! back, written as [`Schema`](crate::Schema) / [`Blueprint`](crate::Blueprint) calls.

use std::collections::{BTreeMap, BTreeSet};

use itertools::Itertools as _;

use crate::grammar::parse_column_type;
use crate::schema::ColumnType;
use crate::{Dialect, Error, Result};

// Desired schema = the snapshot file, not the models: `#[rustique::model]` knows column names only (`Model::COLUMNS`), no SQL types, lengths, defaults or indexes.

/// A schema snapshot as `schema::snapshot` writes it (format documented there).
#[derive(Debug)]
struct Snapshot {
    dialect: Dialect,
    version: u32,
    tables: BTreeMap<String, Table>,
}

#[derive(Debug, Clone, Default, PartialEq)]
struct Table {
    columns: Vec<Col>,
    keys: Vec<Key>,
}

#[derive(Debug, Clone, PartialEq)]
struct Col {
    name: String,
    nullable: bool,
    /// Lower-cased, as the catalog reports it: `varchar(50)`, `number(19,0)`.
    ty: String,
    /// SQL, as the catalog reports it.
    default: Option<String>,
    /// `auto_increment`, `use_current_on_update` or empty.
    extra: String,
}

/// A primary key (named `primary`), unique or plain index, or foreign key.
#[derive(Debug, Clone, PartialEq)]
struct Key {
    name: String,
    kind: String,
    columns: Vec<String>,
    on: String,
    references: Vec<String>,
    on_delete: String,
}

impl Snapshot {
    fn parse(text: &str, file: &str) -> Result<Self> {
        let mut dialect = None;
        let mut version = 1;
        let mut tables: BTreeMap<String, Table> = BTreeMap::new();
        let bad = |number: usize| Error::Config(format!("{file}: line {number} is malformed"));
        for (number, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            // Oracle reports upper-case names; Blueprint calls take lower-case ones.
            let oracle = dialect == Some(Dialect::Oracle);
            let name = |s: &str| {
                if oracle {
                    s.to_lowercase()
                } else {
                    s.to_owned()
                }
            };
            let names = |s: &str| s.split(',').map(name).collect::<Vec<_>>();
            let fields: Vec<&str> = line.split('\t').collect();
            match *fields.as_slice() {
                ["dialect", "mysql"] => dialect = Some(Dialect::MySql),
                ["dialect", "oracle"] => dialect = Some(Dialect::Oracle),
                ["version", v] => version = v.parse().map_err(|_| bad(number + 1))?,
                [
                    table,
                    key,
                    kind @ ("primary key" | "unique" | "index" | "foreign key"),
                    columns,
                    ref rest @ ..,
                ] => {
                    let (on, references, on_delete) = match *rest {
                        [on, references, on_delete] => {
                            (name(on), names(references), on_delete.to_owned())
                        }
                        [] => Default::default(),
                        _ => return Err(bad(number + 1)),
                    };
                    tables.entry(name(table)).or_default().keys.push(Key {
                        name: name(key),
                        kind: kind.to_owned(),
                        columns: names(columns),
                        on,
                        references,
                        on_delete,
                    });
                }
                [
                    table,
                    column,
                    nullable @ ("null" | "not null"),
                    ty,
                    ref rest @ ..,
                ] => {
                    let field = |i: usize| rest.get(i).copied().unwrap_or_default().to_owned();
                    tables.entry(name(table)).or_default().columns.push(Col {
                        name: name(column),
                        nullable: nullable == "null",
                        ty: ty.to_lowercase(),
                        default: Some(field(0)).filter(|d| !d.is_empty()),
                        extra: field(1),
                    });
                }
                _ => return Err(bad(number + 1)),
            }
        }
        let dialect = dialect.ok_or_else(|| Error::Config(format!("{file}: no dialect line")))?;
        Ok(Self {
            dialect,
            version,
            tables,
        })
    }
}

/// The source of a migration module turning the `live` snapshot into the
/// `desired` one (read from `file`); `None` when they match.
pub(crate) fn migration(desired: &str, live: &str, file: &str) -> Result<Option<String>> {
    let desired = Snapshot::parse(desired, file)?;
    let live = Snapshot::parse(live, "the database")?;
    if desired.version < 2 {
        return Err(Error::Config(format!(
            "{file} lacks defaults, indexes and foreign keys; write it again with `schema:snapshot`"
        )));
    }
    if desired.dialect != live.dialect {
        return Err(Error::Config(format!(
            "{file} describes a {:?} database, but the database is {:?}",
            desired.dialect, live.dialect
        )));
    }
    let up = changes(&desired, &live);
    if up.is_empty() {
        return Ok(None);
    }
    let down = changes(&live, &desired);
    Ok(Some(format!(
        "use rustique::{{Db, Schema}};

// Generated by `make:migration --diff` from {file}; review it before running it.

pub async fn up(db: &Db) -> rustique::Result<()> {{
{up}    Ok(())
}}

pub async fn down(db: &Db) -> rustique::Result<()> {{
{down}    Ok(())
}}
"
    )))
}

/// The statements of `up` (or, swapped, `down`): create tables without their
/// foreign keys, which may point at each other; change tables, adding those
/// foreign keys; drop tables, after removing their foreign keys.
fn changes(desired: &Snapshot, actual: &Snapshot) -> String {
    let dialect = desired.dialect;
    let without_foreign = |t: &Table| Table {
        keys: t
            .keys
            .iter()
            .filter(|k| k.kind != "foreign key")
            .cloned()
            .collect(),
        ..t.clone()
    };
    let mut out = String::new();
    let mut call = |head: String, lines: Vec<String>| {
        out.push_str(&format!("    {head}, |t| {{\n"));
        for line in lines {
            out.push_str(&format!("        {line}\n"));
        }
        out.push_str("    })\n    .await?;\n");
    };
    for (name, table) in &desired.tables {
        if !actual.tables.contains_key(name) {
            let lines = body(dialect, &without_foreign(table), &Table::default());
            call(format!("Schema::create(db, {name:?}"), lines);
        }
    }
    let names: BTreeSet<&String> = desired.tables.keys().chain(actual.tables.keys()).collect();
    for name in names {
        let (d, a) = match (desired.tables.get(name), actual.tables.get(name)) {
            (Some(d), Some(a)) => (d.clone(), a.clone()),
            (Some(d), None) => (d.clone(), without_foreign(d)),
            (None, Some(a)) => (without_foreign(a), a.clone()),
            (None, None) => continue,
        };
        let lines = body(dialect, &d, &a);
        if !lines.is_empty() {
            call(format!("Schema::table(db, {name:?}"), lines);
        }
    }
    for name in actual.tables.keys() {
        if !desired.tables.contains_key(name) {
            // shortcut: views are listed like tables; one only in the database gets dropped here.
            out.push_str(&format!("    Schema::drop(db, {name:?}).await?;\n"));
        }
    }
    out
}

/// The blueprint lines turning table `a` into `d`, in the order
/// `Schema::table` runs them: foreign key and index drops, column drops, then
/// index and foreign key additions (it adds and changes columns first anyway).
fn body(dialect: Dialect, d: &Table, a: &Table) -> Vec<String> {
    let mut lines = Vec::new();
    let foreign_last = |k: &&Key| k.kind == "foreign key";
    for k in (a.keys.iter().filter(|k| !d.keys.contains(k))).sorted_by_key(|k| !foreign_last(k)) {
        lines.push(match k.kind.as_str() {
            "foreign key" => format!("t.drop_foreign({:?});", k.name),
            "primary key" => format!(
                "// TODO: drop the primary key ({}) by hand; Blueprint cannot.",
                k.columns.join(", ")
            ),
            _ => format!("t.drop_index({:?});", k.name),
        });
    }
    let dropped: Vec<&Col> = (a.columns.iter())
        .filter(|c| !d.columns.iter().any(|n| n.name == c.name))
        .collect();
    for c in &dropped {
        lines.push(format!("t.drop_column({:?});", c.name));
    }
    let mut increments = None;
    for c in &d.columns {
        let old = a.columns.iter().find(|o| o.name == c.name);
        if old == Some(c) {
            continue;
        }
        if let Some(o) = old
            && (o.extra == "auto_increment" || c.extra == "auto_increment")
        {
            lines.push(format!(
                "// TODO: change `{}` by hand; Blueprint cannot change auto-increment columns.",
                c.name
            ));
            continue;
        }
        let Some(declare) = column(dialect, c) else {
            lines.push(format!(
                "// TODO: `{}` is `{} {}`, which no Blueprint method declares.",
                c.name, c.ty, c.extra
            ));
            continue;
        };
        if declare.starts_with("t.big_increments") {
            increments = Some(vec![c.name.clone()]);
        }
        if old.is_some() {
            lines.push(format!("{declare}.change();"));
            continue;
        }
        if let Some(was) = dropped.iter().find(|o| o.ty == c.ty) {
            lines.push(format!(
                "// TODO: rename? `t.rename_column({:?}, {:?});` would keep the data.",
                was.name, c.name
            ));
        }
        lines.push(format!("{declare};"));
    }
    for k in (d.keys.iter().filter(|k| !a.keys.contains(k))).sorted_by_key(foreign_last) {
        if k.kind == "primary key" && increments.as_ref() == Some(&k.columns) {
            continue; // `big_increments` makes it
        }
        lines.push(key(k));
    }
    lines
}

/// `t.string("name", 50).nullable()...`; `None` for a type no Blueprint method declares.
fn column(dialect: Dialect, c: &Col) -> Option<String> {
    let (ty, unsigned) = parse_column_type(dialect, &c.ty)?;
    let n = format!("{:?}", c.name);
    let increments = c.extra == "auto_increment";
    let mut s = match ty {
        // Oracle has no unsigned numbers; `big_increments` is unsigned on MySQL.
        ColumnType::BigInteger if increments && (unsigned || dialect == Dialect::Oracle) => {
            format!("t.big_increments({n})")
        }
        _ if increments => return None,
        ColumnType::BigInteger => format!("t.big_integer({n})"),
        ColumnType::Integer => format!("t.integer({n})"),
        ColumnType::SmallInteger => format!("t.small_integer({n})"),
        ColumnType::Boolean => format!("t.boolean({n})"),
        ColumnType::String(length) => format!("t.string({n}, {length})"),
        ColumnType::Char(length) => format!("t.char({n}, {length})"),
        ColumnType::Text => format!("t.text({n})"),
        ColumnType::LongText => format!("t.long_text({n})"),
        ColumnType::Double => format!("t.double({n})"),
        ColumnType::Decimal(p, s) => format!("t.decimal({n}, {p}, {s})"),
        ColumnType::Date => format!("t.date({n})"),
        ColumnType::DateTime => format!("t.datetime({n})"),
        ColumnType::Time => format!("t.time({n})"),
        ColumnType::Json => format!("t.json({n})"),
        ColumnType::Binary => format!("t.binary({n})"),
        ColumnType::Uuid => format!("t.uuid({n})"),
    };
    if unsigned && !increments {
        s.push_str(".unsigned()");
    }
    if c.nullable {
        s.push_str(".nullable()");
    }
    if let Some(default) = &c.default {
        s.push_str(&format!(".default_raw({default:?})"));
    }
    if c.extra == "use_current_on_update" {
        s.push_str(".use_current_on_update()");
    }
    Some(s)
}

fn key(k: &Key) -> String {
    let list =
        |names: &[String]| format!("&[{}]", names.iter().map(|c| format!("{c:?}")).join(", "));
    let columns = list(&k.columns);
    match k.kind.as_str() {
        "primary key" => format!("t.primary({columns});"),
        "foreign key" => format!(
            "t.foreign({columns}, {:?}, {}).name({:?}){};",
            k.on,
            list(&k.references),
            k.name,
            match k.on_delete.as_str() {
                "cascade" => ".cascade_on_delete()",
                "set null" => ".null_on_delete()",
                _ => "",
            }
        ),
        kind => format!("t.{kind}({columns}).name({:?});", k.name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEAD: &str = "dialect\tmysql\nversion\t2\n";

    fn diff(desired: &str, live: &str) -> Option<String> {
        migration(
            &format!("{HEAD}{desired}"),
            &format!("{HEAD}{live}"),
            "schema.txt",
        )
        .unwrap()
    }

    const USERS: &str = "users\tid\tnot null\tbigint unsigned\t\tauto_increment\n\
        users\temail\tnot null\tvarchar(255)\n\
        users\tvotes\tnot null\tint\t0\n\
        users\tprimary\tprimary key\tid\n\
        users\tusers_email_unique\tunique\temail\n";

    #[test]
    fn writes_nothing_without_changes() {
        assert_eq!(diff(USERS, USERS), None);
    }

    #[test]
    fn creates_and_drops_tables() {
        let posts = "posts\tid\tnot null\tbigint unsigned\t\tauto_increment\n\
            posts\tuser_id\tnot null\tbigint unsigned\n\
            posts\tbody\tnull\ttext\n\
            posts\tprimary\tprimary key\tid\n\
            posts\tposts_user_id_foreign\tforeign key\tuser_id\tusers\tid\tcascade\n";
        let source = diff(&format!("{USERS}{posts}"), USERS).unwrap();
        let (up, down) = source.split_once("pub async fn down").unwrap();
        let create = r#"    Schema::create(db, "posts", |t| {
        t.big_increments("id");
        t.big_integer("user_id").unsigned();
        t.text("body").nullable();
    })
    .await?;
    Schema::table(db, "posts", |t| {
        t.foreign(&["user_id"], "users", &["id"]).name("posts_user_id_foreign").cascade_on_delete();
    })
    .await?;
    Ok(())"#;
        assert!(up.contains(create), "{up}");
        let drop = r#"    Schema::table(db, "posts", |t| {
        t.drop_foreign("posts_user_id_foreign");
    })
    .await?;
    Schema::drop(db, "posts").await?;
    Ok(())"#;
        assert!(down.contains(drop), "{down}");
    }

    #[test]
    fn changes_columns_and_keys() {
        let desired = "users\tid\tnot null\tbigint unsigned\t\tauto_increment\n\
            users\temail\tnull\tvarchar(320)\n\
            users\tvotes\tnot null\tint\t0\n\
            users\thandle\tnot null\tvarchar(255)\n\
            users\tseen_at\tnull\tdatetime(6)\t(utc_timestamp(6))\tuse_current_on_update\n\
            users\tshape\tnull\tpoint\n\
            users\tprimary\tprimary key\tid\n\
            users\tusers_votes_index\tindex\tvotes,email\n";
        let live = "users\tid\tnot null\tbigint unsigned\t\tauto_increment\n\
            users\temail\tnot null\tvarchar(255)\n\
            users\tvotes\tnot null\tint\t0\n\
            users\tname\tnot null\tvarchar(255)\n\
            users\tprimary\tprimary key\tid\n\
            users\tusers_email_unique\tunique\temail\n";
        let source = diff(desired, live).unwrap();
        let (up, down) = source.split_once("pub async fn down").unwrap();
        let expected = r#"    Schema::table(db, "users", |t| {
        t.drop_index("users_email_unique");
        t.drop_column("name");
        t.string("email", 320).nullable().change();
        // TODO: rename? `t.rename_column("name", "handle");` would keep the data.
        t.string("handle", 255);
        t.datetime("seen_at").nullable().default_raw("(utc_timestamp(6))").use_current_on_update();
        // TODO: `shape` is `point `, which no Blueprint method declares.
        t.index(&["votes", "email"]).name("users_votes_index");
    })
    .await?;"#;
        assert!(up.contains(expected), "{up}");
        for line in [
            r#"t.drop_index("users_votes_index");"#,
            r#"t.string("email", 255).change();"#,
            r#"t.string("name", 255);"#,
            r#"t.unique(&["email"]).name("users_email_unique");"#,
        ] {
            assert!(down.contains(line), "{line}\n{down}");
        }
    }

    #[test]
    fn reads_oracle_names_and_types() {
        let head = "dialect\toracle\nversion\t2\n";
        let live = format!(
            "{head}USERS\tID\tnot null\tNUMBER(19,0)\t\tauto_increment\n\
             USERS\tprimary\tprimary key\tID\n"
        );
        let desired = format!(
            "{live}USERS\tACTIVE\tnot null\tNUMBER(1,0)\t1\nUSERS\tPRICE\tnull\tNUMBER(10,2)\n\
             USERS\tCODE\tnull\tVARCHAR2(18)\nUSERS\tAT\tnull\tTIMESTAMP(6)\n"
        );
        let source = migration(&desired, &live, "f").unwrap().unwrap();
        for line in [
            r#"Schema::table(db, "users""#,
            r#"t.boolean("active").default_raw("1");"#,
            r#"t.decimal("price", 10, 2).nullable();"#,
            r#"t.string("code", 18).nullable();"#,
            r#"t.datetime("at").nullable();"#,
            r#"t.drop_column("active");"#,
        ] {
            assert!(source.contains(line), "{line}\n{source}");
        }
        let created = migration(&desired, head, "f").unwrap().unwrap();
        assert!(
            created.contains("t.big_increments(\"id\");\n        t.boolean"),
            "{created}"
        );
        assert!(!created.contains("t.primary"), "{created}");
    }

    #[test]
    fn refuses_old_or_foreign_snapshots() {
        let old = "dialect\tmysql\nusers\tid\tnot null\tbigint\n";
        assert!(migration(old, HEAD, "f").is_err());
        assert!(migration(HEAD, "dialect\toracle\nversion\t2\n", "f").is_err());
        assert!(migration(&format!("{HEAD}users\tbad\n"), HEAD, "f").is_err());
    }

    /// The inverse of the grammar's type mapping, on types as the catalogs report them.
    #[test]
    fn parses_column_types_back() {
        use ColumnType as T;
        let cases = [
            ("bigint", T::BigInteger, "NUMBER(19,0)", T::BigInteger),
            ("int", T::Integer, "NUMBER(10,0)", T::Integer),
            ("smallint", T::SmallInteger, "NUMBER(5,0)", T::SmallInteger),
            ("tinyint(1)", T::Boolean, "NUMBER(1,0)", T::Boolean),
            ("varchar(50)", T::String(50), "VARCHAR2(50)", T::String(50)),
            ("char(26)", T::Char(26), "CHAR(26)", T::Char(26)),
            ("text", T::Text, "CLOB", T::Text),
            ("longtext", T::LongText, "CLOB", T::Text),
            ("double", T::Double, "BINARY_DOUBLE", T::Double),
            (
                "decimal(10,2)",
                T::Decimal(10, 2),
                "NUMBER(10,2)",
                T::Decimal(10, 2),
            ),
            ("date", T::Date, "DATE", T::Date),
            ("datetime(6)", T::DateTime, "TIMESTAMP(6)", T::DateTime),
            ("time(6)", T::Time, "VARCHAR2(18)", T::String(18)),
            ("json", T::Json, "CLOB", T::Text),
            ("longblob", T::Binary, "BLOB", T::Binary),
        ];
        for (mysql, mysql_ty, oracle, oracle_ty) in cases {
            let parsed = parse_column_type(Dialect::MySql, mysql);
            assert_eq!(parsed, Some((mysql_ty, false)), "{mysql}");
            let parsed = parse_column_type(Dialect::Oracle, oracle);
            assert_eq!(parsed, Some((oracle_ty, false)), "{oracle}");
        }
        let unsigned = parse_column_type(Dialect::MySql, "bigint unsigned");
        assert_eq!(unsigned, Some((T::BigInteger, true)));
        assert_eq!(parse_column_type(Dialect::MySql, "mediumint"), None);
        assert_eq!(parse_column_type(Dialect::Oracle, "NUMBER(*,0)"), None);
    }
}
