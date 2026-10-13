//! The `schema-check` feature: compares a model's fields with the schema
//! snapshot that the CLI's `schema:snapshot` writes to `rustique-schema.txt`
//! next to the crate's `Cargo.toml` (format in `rustique/src/schema.rs`).
//!
//! Without the file nothing is checked. Types the check does not know,
//! such as `#[derive(rustique::Cast)]` types, are checked for existence only.

use std::collections::HashMap;
use std::path::Path;

use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::quote;
use syn::{GenericArgument, LitStr, PathArguments, Type};

use crate::Column;

const FILE: &str = "rustique-schema.txt";

/// Errors for every field that does not match the snapshot, or tokens that
/// make cargo rebuild the model when the snapshot changes.
pub(crate) fn check(table: &LitStr, columns: &[Column]) -> syn::Result<TokenStream2> {
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return Ok(TokenStream2::new());
    };
    let path = Path::new(&dir).join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(TokenStream2::new());
    };
    let snapshot = Snapshot::parse(&text).map_err(|problem| {
        syn::Error::new(Span::call_site(), format!("{}: {problem}", path.display()))
    })?;
    check_columns(&snapshot, table, columns)?;
    let path = path.display().to_string();
    Ok(quote!(
        const _: &[u8] = ::core::include_bytes!(#path);
    ))
}

#[derive(Debug, Default)]
pub(crate) struct Snapshot {
    oracle: bool,
    /// Lower-case table name to its lower-case column names.
    tables: HashMap<String, HashMap<String, SnapshotColumn>>,
}

#[derive(Debug)]
struct SnapshotColumn {
    nullable: bool,
    ty: String,
}

impl Snapshot {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let mut snapshot = Self::default();
        for (number, line) in text.lines().enumerate() {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            match line.split('\t').collect::<Vec<_>>()[..] {
                ["dialect", dialect] => snapshot.oracle = dialect == "oracle",
                // Keys and the format version are for `make:migration --diff`.
                ["version", _] | [_, _, "primary key" | "unique" | "index" | "foreign key", ..] => {
                }
                [table, column, nullable @ ("null" | "not null"), ty, ..] => {
                    snapshot
                        .tables
                        .entry(table.to_lowercase())
                        .or_default()
                        .insert(
                            column.to_lowercase(),
                            SnapshotColumn {
                                nullable: nullable == "null",
                                ty: ty.to_owned(),
                            },
                        );
                }
                _ => {
                    return Err(format!(
                        "line {} is not `table<TAB>column<TAB>null|not null<TAB>type`; \
                         write the file again with `schema:snapshot`",
                        number + 1
                    ));
                }
            }
        }
        Ok(snapshot)
    }
}

pub(crate) fn check_columns(
    snapshot: &Snapshot,
    table: &LitStr,
    columns: &[Column],
) -> syn::Result<()> {
    let name = table.value();
    // shortcut: `schema.table` names a schema the snapshot does not cover; skipped.
    if name.contains('.') {
        return Ok(());
    }
    let Some(snapshot_columns) = snapshot.tables.get(&name.to_lowercase()) else {
        return Err(syn::Error::new_spanned(
            table,
            format!("table `{name}` is not in {FILE}; migrate, then run `schema:snapshot` again"),
        ));
    };
    let mut errors: Option<syn::Error> = None;
    let mut fail = |error: syn::Error| match &mut errors {
        Some(errors) => errors.combine(error),
        None => errors = Some(error),
    };
    for column in columns {
        let Some(found) = snapshot_columns.get(&column.name.to_lowercase()) else {
            fail(syn::Error::new_spanned(
                &column.ident,
                format!("table `{name}` has no column `{}` in {FILE}", column.name),
            ));
            continue;
        };
        let (optional, inner) = match generic_arg(&column.ty, "Option") {
            Some(inner) => (true, inner),
            None => (false, &column.ty),
        };
        let Some(rust) = RustKind::of(inner) else {
            continue;
        };
        // Oracle stores `''` as `NULL` and reads it back into a `String` as `""`.
        let empty_string = snapshot.oracle && rust == RustKind::String;
        if found.nullable && !optional && !empty_string && !column.not_null {
            fail(syn::Error::new_spanned(
                &column.ty,
                format!(
                    "column `{name}.{}` is nullable: make the field an `Option`, \
                     or mark it `#[rustique(not_null)]` if it never holds NULL",
                    column.name
                ),
            ));
        }
        if let Some(kind) = DbKind::of(&found.ty)
            && !rust.reads(kind)
        {
            fail(syn::Error::new_spanned(
                inner,
                format!(
                    "column `{name}.{}` is `{}`, which this type cannot read",
                    column.name, found.ty
                ),
            ));
        }
    }
    errors.map_or(Ok(()), Err)
}

/// `T` of `Name<T>`.
fn generic_arg<'a>(ty: &'a Type, name: &str) -> Option<&'a Type> {
    let Type::Path(path) = ty else { return None };
    let segment = path.path.segments.last()?;
    if segment.ident != name {
        return None;
    }
    let PathArguments::AngleBracketed(args) = &segment.arguments else {
        return None;
    };
    match args.args.first()? {
        GenericArgument::Type(inner) => Some(inner),
        _ => None,
    }
}

/// The Rust field types the check knows, after `Option`.
#[derive(Clone, Copy, PartialEq, Debug)]
enum RustKind {
    Bool,
    Integer,
    Float,
    Decimal,
    String,
    Bytes,
    DateTime,
    Time,
    Json,
}

impl RustKind {
    fn of(ty: &Type) -> Option<Self> {
        let Type::Path(path) = ty else { return None };
        let segment = path.path.segments.last()?;
        Some(match segment.ident.to_string().as_str() {
            "bool" => Self::Bool,
            "i8" | "i16" | "i32" | "i64" | "u8" | "u16" | "u32" | "u64" => Self::Integer,
            "f32" | "f64" => Self::Float,
            "Decimal" => Self::Decimal,
            "String" => Self::String,
            "Vec"
                if generic_arg(ty, "Vec").is_some_and(
                    |inner| matches!(inner, Type::Path(p) if p.path.is_ident("u8")),
                ) =>
            {
                Self::Bytes
            }
            "NaiveDate" | "NaiveDateTime" | "DateTime" => Self::DateTime,
            "NaiveTime" => Self::Time,
            // Not `rustique::Value`, which reads anything.
            "Value" if path.path.segments.iter().any(|s| s.ident == "serde_json") => Self::Json,
            _ => return None,
        })
    }

    /// Whether `FromValue` of this type accepts what a `kind` column holds.
    fn reads(self, kind: DbKind) -> bool {
        use DbKind as K;
        match self {
            Self::Bool | Self::Integer => matches!(kind, K::Integer | K::Number),
            Self::Float => matches!(kind, K::Float | K::Integer | K::Number | K::Decimal),
            Self::Decimal => matches!(
                kind,
                K::Decimal | K::Integer | K::Number | K::Float | K::Text
            ),
            Self::String => matches!(kind, K::Text | K::Json),
            Self::Bytes => matches!(kind, K::Binary | K::Text),
            Self::DateTime => kind == K::DateTime,
            Self::Time => matches!(kind, K::Time | K::Text),
            Self::Json => matches!(kind, K::Json | K::Text | K::Binary),
        }
    }
}

/// Column types as `FromValue` sees them. Dates and datetimes read into
/// each other's types, so they are one kind.
#[derive(Clone, Copy, PartialEq, Debug)]
enum DbKind {
    Integer,
    /// A fractional `DECIMAL(p,s)` / `NUMBER(p,s)`.
    Decimal,
    /// Oracle `NUMBER` without a scale: any number.
    Number,
    Float,
    Text,
    Json,
    Binary,
    DateTime,
    Time,
}

impl DbKind {
    /// The kind of a snapshot type such as `bigint unsigned`, `decimal(10,2)`
    /// or `NUMBER(19,0)`; `None` for types the check does not know.
    fn of(ty: &str) -> Option<Self> {
        let ty = ty.to_lowercase();
        let base = ty.split(['(', ' ']).next().unwrap_or_default();
        let scale = ty
            .split_once(',')
            .and_then(|(_, rest)| rest.trim_end_matches(')').trim().parse::<u32>().ok());
        Some(match base {
            "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint" | "year" => {
                Self::Integer
            }
            "decimal" | "numeric" | "number" => match scale {
                Some(0) => Self::Integer,
                Some(_) => Self::Decimal,
                None => Self::Number,
            },
            "float" | "double" | "real" | "binary_float" | "binary_double" => Self::Float,
            "char" | "varchar" | "tinytext" | "text" | "mediumtext" | "longtext" | "enum"
            | "set" | "varchar2" | "nvarchar2" | "nchar" | "clob" | "nclob" => Self::Text,
            "json" => Self::Json,
            "binary" | "varbinary" | "tinyblob" | "blob" | "mediumblob" | "longblob" | "raw" => {
                Self::Binary
            }
            "date" | "datetime" | "timestamp" => Self::DateTime,
            "time" => Self::Time,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use syn::parse_quote;

    use super::*;

    const MYSQL: &str = "# comment\n\
        dialect\tmysql\n\
        version\t2\n\
        users\tid\tnot null\tbigint unsigned\t\tauto_increment\n\
        users\temail\tnot null\tvarchar(255)\t'x'\n\
        users\tnickname\tnull\tvarchar(50)\n\
        users\tscore\tnot null\tdecimal(10,2)\n\
        users\tactive\tnot null\ttinyint(1)\n\
        users\tdata\tnull\tjson\n\
        users\tcreated_at\tnull\tdatetime(6)\n\
        users\tpoint\tnot null\tpoint\n\
        users\tprimary\tprimary key\tid\n\
        users\tusers_email_unique\tunique\temail\n\
        users\tusers_team_id_foreign\tforeign key\tteam_id\tteams\tid\tcascade\n";

    fn column(name: &str, ty: Type) -> Column {
        Column {
            ident: syn::Ident::new(name, Span::call_site()),
            ty,
            name: name.to_owned(),
            primary_key: false,
            sequence: None,
            fillable: false,
            not_null: false,
        }
    }

    fn errors(snapshot: &str, table: &str, columns: &[Column]) -> Vec<String> {
        let snapshot = Snapshot::parse(snapshot).unwrap();
        let table = LitStr::new(table, Span::call_site());
        match check_columns(&snapshot, &table, columns) {
            Ok(()) => Vec::new(),
            Err(errors) => errors.into_iter().map(|e| e.to_string()).collect(),
        }
    }

    #[test]
    fn accepts_matching_models() {
        let columns = [
            column("id", parse_quote!(u64)),
            column("email", parse_quote!(String)),
            column("nickname", parse_quote!(Option<String>)),
            column("score", parse_quote!(rust_decimal::Decimal)),
            column("active", parse_quote!(bool)),
            column("data", parse_quote!(Option<serde_json::Value>)),
            column("created_at", parse_quote!(Option<DateTime<Utc>>)),
            // Unknown types: existence only.
            column("point", parse_quote!(Point)),
        ];
        assert_eq!(errors(MYSQL, "USERS", &columns), Vec::<String>::new());
        assert_eq!(
            errors(MYSQL, "other_schema.users", &columns),
            Vec::<String>::new()
        );
    }

    #[test]
    fn reports_every_mismatch() {
        let columns = [
            column("email", parse_quote!(i64)),
            column("nickname", parse_quote!(String)),
            column("score", parse_quote!(i64)),
            column("missing", parse_quote!(String)),
        ];
        assert_eq!(
            errors(MYSQL, "users", &columns),
            [
                "column `users.email` is `varchar(255)`, which this type cannot read",
                "column `users.nickname` is nullable: make the field an `Option`, \
                 or mark it `#[rustique(not_null)]` if it never holds NULL",
                "column `users.score` is `decimal(10,2)`, which this type cannot read",
                "table `users` has no column `missing` in rustique-schema.txt",
            ]
        );
        let not_null = Column {
            not_null: true,
            ..column("nickname", parse_quote!(String))
        };
        assert_eq!(errors(MYSQL, "users", &[not_null]), Vec::<String>::new());
        assert_eq!(
            errors(MYSQL, "posts", &[]),
            [
                "table `posts` is not in rustique-schema.txt; migrate, then run `schema:snapshot` again"
            ]
        );
    }

    #[test]
    fn oracle_types() {
        let oracle = "dialect\toracle\n\
            USERS\tID\tnot null\tNUMBER(19,0)\n\
            USERS\tNAME\tnull\tVARCHAR2(50)\n\
            USERS\tPRICE\tnot null\tNUMBER(10,2)\n\
            USERS\tANY_NUMBER\tnot null\tNUMBER\n\
            USERS\tSEEN\tnull\tTIMESTAMP(6)\n";
        let columns = [
            column("id", parse_quote!(i64)),
            // `''` is `NULL` on Oracle; a `String` reads it as `""`.
            column("name", parse_quote!(String)),
            column("price", parse_quote!(f64)),
            column("any_number", parse_quote!(i32)),
            column("seen", parse_quote!(Option<NaiveDate>)),
        ];
        assert_eq!(errors(oracle, "users", &columns), Vec::<String>::new());
        let columns = [column("price", parse_quote!(i64))];
        assert_eq!(errors(oracle, "users", &columns).len(), 1);
    }

    #[test]
    fn rejects_malformed_snapshots() {
        assert!(Snapshot::parse("users id bigint").is_err());
        assert!(Snapshot::parse("users\tid\tmaybe\tbigint").is_err());
    }
}
