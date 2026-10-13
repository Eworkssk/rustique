//! The `schema-check` feature: compares a model's fields with the schema
//! snapshot that the CLI's `schema:snapshot` writes to `rustique-schema.txt`
//! next to the crate's `Cargo.toml` (format in `rustique/src/schema.rs`).
//!
//! Without the file nothing is checked. Types the check does not know,
//! such as `#[derive(rustique::Cast)]` types, are checked for existence only.
//! Relation keys are checked when their table is in the snapshot.
//!
//! `col!`, `#[rustique::scopes]` and relation `scope`s check column strings
//! (`column_literals`, `check_reference`).

use std::collections::HashMap;
use std::path::Path;

use proc_macro2::{Delimiter, Span, TokenStream as TokenStream2, TokenTree};
use quote::{ToTokens, quote};
use syn::{GenericArgument, Ident, LitStr, PathArguments, Type};

use crate::{Column, MorphToField, RelationField, RelationKind, snake, type_name};

const FILE: &str = "rustique-schema.txt";

/// The snapshot next to the crate's `Cargo.toml`, with tokens that make cargo
/// rebuild the caller when the file changes; `None` without the file.
pub(crate) fn load() -> syn::Result<Option<(Snapshot, TokenStream2)>> {
    let Ok(dir) = std::env::var("CARGO_MANIFEST_DIR") else {
        return Ok(None);
    };
    let path = Path::new(&dir).join(FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let snapshot = Snapshot::parse(&text).map_err(|problem| {
        syn::Error::new(Span::call_site(), format!("{}: {problem}", path.display()))
    })?;
    let path = path.display().to_string();
    Ok(Some((
        snapshot,
        quote!(
            const _: &[u8] = ::core::include_bytes!(#path);
        ),
    )))
}

/// Errors for every field, relation key or column string of a relation
/// `scope` that does not match the snapshot, or the rebuild tokens of [`load`].
pub(crate) fn check(
    table: &LitStr,
    model: &Ident,
    columns: &[Column],
    relations: &[RelationField],
    morphs: &[MorphToField],
) -> syn::Result<TokenStream2> {
    let Some((snapshot, rebuild)) = load()? else {
        return Ok(TokenStream2::new());
    };
    let keys = relation_keys(&table.value(), model, relations, morphs);
    let mut literals = Vec::new();
    for scope in relations.iter().filter_map(|r| r.scope.as_ref()) {
        column_literals(scope.to_token_stream(), &mut literals);
    }
    let errors = [
        check_columns(&snapshot, table, columns),
        check_keys(&snapshot, &keys),
    ]
    .into_iter()
    .filter_map(Result::err)
    .chain(
        literals
            .iter()
            .filter_map(|l| check_reference(&snapshot, l, false).err()),
    );
    combine(errors).map_or(Ok(rebuild), Err)
}

/// `col!("table.column")`: the literal, after [`check_reference`] with `strict`.
pub(crate) fn col(literal: &LitStr) -> syn::Result<TokenStream2> {
    let Some((snapshot, rebuild)) = load()? else {
        return Ok(quote!(#literal));
    };
    check_reference(&snapshot, literal, true)?;
    Ok(quote!({ #rebuild #literal }))
}

/// Errors for the `table.column` strings that `tokens` passes to query-builder
/// methods ([`column_literals`]), or the rebuild tokens of [`load`].
pub(crate) fn check_tokens(tokens: TokenStream2) -> syn::Result<TokenStream2> {
    let Some((snapshot, rebuild)) = load()? else {
        return Ok(TokenStream2::new());
    };
    let mut literals = Vec::new();
    column_literals(tokens, &mut literals);
    let errors = literals
        .iter()
        .filter_map(|l| check_reference(&snapshot, l, false).err());
    combine(errors).map_or(Ok(rebuild), Err)
}

/// `column` (in any table), `table.column` or `table.*`: an error when the
/// snapshot has no such column. With `strict`, a table missing from the
/// snapshot or a lone column in no table is an error; otherwise it is skipped,
/// as are strings of any other shape (`schema.table.column`, `count(*)`, `id as key`).
pub(crate) fn check_reference(
    snapshot: &Snapshot,
    literal: &LitStr,
    strict: bool,
) -> syn::Result<()> {
    let value = literal.value();
    let plain = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
    let fail = |message: String| Err(syn::Error::new_spanned(literal, message));
    match value.split_once('.') {
        Some((table, column)) if plain(table) && (plain(column) || column == "*") => {
            match snapshot.tables.get(&table.to_lowercase()) {
                None if strict => fail(format!("table `{table}` is not in {FILE}")),
                Some(columns) if column != "*" && !columns.contains_key(&column.to_lowercase()) => {
                    fail(format!(
                        "table `{table}` has no column `{column}` in {FILE}"
                    ))
                }
                _ => Ok(()),
            }
        }
        None if strict
            && plain(&value)
            && !snapshot
                .tables
                .values()
                .any(|t| t.contains_key(&value.to_lowercase())) =>
        {
            fail(format!("no table has a column `{value}` in {FILE}"))
        }
        _ => Ok(()),
    }
}

/// Query-builder methods whose first argument is a column.
const COLUMN_METHODS: &[&str] = &[
    "where_",
    "or_where",
    "where_in",
    "or_where_in",
    "where_not_in",
    "or_where_not_in",
    "where_null",
    "or_where_null",
    "where_not_null",
    "or_where_not_null",
    "where_between",
    "where_not_between",
    "where_date",
    "where_column",
    "or_where_column",
    "order_by",
    "order_by_desc",
    "group_by",
    "select",
    "add_select",
];

/// The string literals, or arrays of them, that `tokens` passes as the first
/// argument of a [`COLUMN_METHODS`] call: `.where_("users.active", ...)`,
/// `.select(["users.id"])`.
///
/// shortcut: matches method names on tokens, without types, so a non-`Query`
/// method of the same name is scanned too; harmless while only `table.column`
/// strings naming a snapshot table are checked. Upgrade if lone columns should
/// be checked against the model's table.
pub(crate) fn column_literals(tokens: TokenStream2, found: &mut Vec<LitStr>) {
    let tokens: Vec<TokenTree> = tokens.into_iter().collect();
    for (i, token) in tokens.iter().enumerate() {
        if let TokenTree::Group(group) = token {
            column_literals(group.stream(), found);
        }
        let Some(
            [
                TokenTree::Punct(dot),
                TokenTree::Ident(method),
                TokenTree::Group(args),
            ],
        ) = tokens.get(i..i + 3)
        else {
            continue;
        };
        if dot.as_char() != '.'
            || args.delimiter() != Delimiter::Parenthesis
            || !COLUMN_METHODS.contains(&method.to_string().as_str())
        {
            continue;
        }
        let first: Vec<TokenTree> = args
            .stream()
            .into_iter()
            .take_while(|t| !matches!(t, TokenTree::Punct(p) if p.as_char() == ','))
            .collect();
        let literals = match &first[..] {
            [TokenTree::Group(array)] if array.delimiter() == Delimiter::Bracket => {
                array.stream().into_iter().collect()
            }
            [TokenTree::Literal(_)] => first,
            _ => continue,
        };
        found.extend(
            literals
                .into_iter()
                .filter_map(|t| syn::parse2::<LitStr>(t.into()).ok()),
        );
    }
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

fn combine(errors: impl IntoIterator<Item = syn::Error>) -> Option<syn::Error> {
    errors.into_iter().reduce(|mut all, error| {
        all.combine(error);
        all
    })
}

/// The columns that relation attributes name, each with the table it belongs in.
///
/// shortcut: the related (and `through`) model's table is known only as a Rust
/// type, so it is guessed as the snake_case plural of the type name (`BlogPost`
/// → `blog_posts`) and skipped when the snapshot has no such table; a model with
/// another table while a table of the guessed name exists is checked against the
/// wrong one. Upgrade when the macro can see the related model's `TABLE`.
pub(crate) fn relation_keys(
    table: &str,
    model: &Ident,
    relations: &[RelationField],
    morphs: &[MorphToField],
) -> Vec<(String, LitStr)> {
    let guess = |path: &syn::Path| format!("{}s", snake(&type_name(path)));
    let mut keys = Vec::new();
    for relation in relations {
        let related = guess(&relation.related);
        let pivot = relation
            .kind
            .has_pivot()
            .then(|| relation.pivot_table(&snake(&model.to_string())));
        let through = relation.through.as_ref().map(guess);
        for (option, columns) in &relation.keys {
            let owner = match (option.as_str(), relation.kind) {
                ("local_key" | "parent_key", _) | ("foreign_key", RelationKind::BelongsTo) => {
                    Some(table)
                }
                ("foreign_key" | "owner_key" | "related_key" | "second_key", _) => {
                    Some(related.as_str())
                }
                ("foreign_pivot_key" | "related_pivot_key" | "pivot", _) => pivot.as_deref(),
                ("first_key" | "second_local_key", _) => through.as_deref(),
                _ => None,
            };
            if let Some(owner) = owner {
                keys.extend(columns.iter().map(|c| (owner.to_owned(), c.clone())));
            }
        }
        if let (Some(pivot), Some(timestamps)) = (&pivot, &relation.pivot_timestamps) {
            keys.extend(timestamps.iter().map(|c| (pivot.clone(), c.clone())));
        }
        if relation.kind.is_morph() {
            let name = &relation.keys["name"][0];
            let column = |suffix| LitStr::new(&format!("{}{suffix}", name.value()), name.span());
            // `{name}_id` is the default of this key, checked above when it is given.
            let (owner, id_key) = match relation.kind {
                RelationKind::MorphOne | RelationKind::MorphMany => (Some(&related), ""),
                RelationKind::MorphToMany => (pivot.as_ref(), "foreign_pivot_key"),
                _ => (pivot.as_ref(), "related_pivot_key"),
            };
            if let Some(owner) = owner {
                keys.push((owner.clone(), column("_type")));
                if !relation.keys.contains_key(id_key) {
                    keys.push((owner.clone(), column("_id")));
                }
            }
        }
    }
    for morph in morphs {
        for suffix in ["_type", "_id"] {
            let column = LitStr::new(&format!("{}{suffix}", morph.columns), morph.ident.span());
            keys.push((table.to_owned(), column));
        }
    }
    keys
}

/// Errors for `(table, column)` pairs whose table is in the snapshot without the column.
pub(crate) fn check_keys(snapshot: &Snapshot, keys: &[(String, LitStr)]) -> syn::Result<()> {
    let errors = keys.iter().filter_map(|(table, column)| {
        let columns = snapshot.tables.get(&table.to_lowercase())?;
        (!columns.contains_key(&column.value().to_lowercase())).then(|| {
            syn::Error::new_spanned(
                column,
                format!(
                    "table `{table}` has no column `{}` in {FILE}",
                    column.value()
                ),
            )
        })
    });
    combine(errors).map_or(Ok(()), Err)
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
    fn checks_relation_keys() {
        let snapshot = Snapshot::parse(
            "users\tid\tnot null\tbigint\n\
             users\tuuid\tnot null\tvarchar(36)\n\
             users\tcommentable_type\tnull\tvarchar(50)\n\
             users\tcommentable_id\tnull\tbigint\n\
             posts\tuser_id\tnot null\tbigint\n\
             role_user\tuser_id\tnot null\tbigint\n\
             role_user\trole_id\tnot null\tbigint\n\
             taggables\ttaggable_type\tnot null\tvarchar(50)\n",
        )
        .unwrap();
        let mut item: syn::ItemStruct = parse_quote! {
            struct User {
                #[rustique(has_many(Post, foreign_key = "user_id", local_key = "uuid"))]
                posts: Rel<Vec<Post>>,
                #[rustique(has_many(Post, foreign_key = "author_id", local_key = "nope"))]
                bad_posts: Rel<Vec<Post>>,
                // No `comments` table in the snapshot: skipped.
                #[rustique(has_many(Comment, foreign_key = "anything"))]
                comments: Rel<Vec<Comment>>,
                #[rustique(belongs_to_many(Role, foreign_pivot_key = "user_id", related_pivot_key = "rol_id", pivot = ["level"]))]
                roles: Rel<Vec<Role>>,
                // `taggables` has `taggable_type` but no `taggable_id`.
                #[rustique(morph_to_many(Tag, name = "taggable"))]
                tags: Rel<Vec<Tag>>,
                #[rustique(morph_to)]
                commentable: Rel<Option<Commentable>>,
                #[rustique(morph_to)]
                owner: Rel<Option<Owner>>,
            }
        };
        let syn::Fields::Named(named) = &mut item.fields else {
            unreachable!()
        };
        let fields = crate::ModelFields::parse(&mut named.named, None).unwrap();
        let keys = relation_keys("users", &item.ident, &fields.relations, &fields.morphs);
        let mut errors: Vec<String> = check_keys(&snapshot, &keys)
            .err()
            .into_iter()
            .flatten()
            .map(|e| e.to_string())
            .collect();
        errors.sort();
        assert_eq!(
            errors,
            [
                "table `posts` has no column `author_id` in rustique-schema.txt",
                "table `role_user` has no column `level` in rustique-schema.txt",
                "table `role_user` has no column `rol_id` in rustique-schema.txt",
                "table `taggables` has no column `taggable_id` in rustique-schema.txt",
                "table `users` has no column `nope` in rustique-schema.txt",
                "table `users` has no column `owner_id` in rustique-schema.txt",
                "table `users` has no column `owner_type` in rustique-schema.txt",
            ]
        );
    }

    #[test]
    fn checks_column_strings() {
        let snapshot = Snapshot::parse(MYSQL).unwrap();
        let check = |value: &str, strict| {
            check_reference(&snapshot, &LitStr::new(value, Span::call_site()), strict)
                .err()
                .map(|e| e.to_string())
        };
        for ok in [
            "users.email",
            "USERS.EMAIL",
            "users.*",
            "email",
            "a.b.c",
            "count(*)",
        ] {
            assert_eq!(check(ok, true), None, "{ok}");
        }
        assert_eq!(
            check("users.mail", false).unwrap(),
            "table `users` has no column `mail` in rustique-schema.txt"
        );
        assert_eq!(
            check("posts.id", true).unwrap(),
            "table `posts` is not in rustique-schema.txt"
        );
        assert_eq!(check("posts.id", false), None);
        assert_eq!(
            check("mail", true).unwrap(),
            "no table has a column `mail` in rustique-schema.txt"
        );
        assert_eq!(check("mail", false), None);

        let mut found = Vec::new();
        column_literals(
            quote! {
                fn scope_x(q: Query<User>) -> Query<User> {
                    q.where_("users.a", "=", 1)
                        .select(["users.b", "users.c"])
                        .where_has("posts.comments", |p| p.order_by("posts.d"))
                        .where_(name, "=", 1)
                        .where_raw("users.e = 1")
                }
            },
            &mut found,
        );
        let found: Vec<String> = found.iter().map(LitStr::value).collect();
        assert_eq!(found, ["users.a", "users.b", "users.c", "posts.d"]);
    }

    #[test]
    fn rejects_malformed_snapshots() {
        assert!(Snapshot::parse("users id bigint").is_err());
        assert!(Snapshot::parse("users\tid\tmaybe\tbigint").is_err());
    }
}
