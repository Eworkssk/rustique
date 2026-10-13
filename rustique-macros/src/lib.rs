//! Proc-macros of the `rustique` crate. Use them through `rustique`: `#[rustique::model]`,
//! `#[rustique::scopes]`, `#[derive(rustique::Cast)]`.

mod cast;
mod morph;
#[cfg_attr(not(feature = "schema-check"), allow(dead_code))]
mod schema_check;

use std::collections::HashMap;

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::meta::ParseNestedMeta;
use syn::parse::ParseStream;
use syn::parse::Parser;
use syn::punctuated::Punctuated;
use syn::{
    Attribute, Field, Fields, FnArg, Ident, ImplItem, ItemImpl, ItemStruct, LitBool, LitStr, Meta,
    Path, Token, Type, Visibility, parse_macro_input,
};

/// Turns a struct into an `rustique::Model`. See the `Model` trait for the options.
///
/// Adds a hidden `__meta` field (build models with `..Default::default()`),
/// one `&str` constant per column (`User::EMAIL == "email"`) and the `Model`
/// implementation.
///
/// With the `schema-check` feature, the fields are checked against the
/// `rustique-schema.txt` snapshot next to the crate's `Cargo.toml`, when there is
/// one: see the `schema:snapshot` command in `rustique::cli`.
#[proc_macro_attribute]
pub fn model(args: TokenStream, item: TokenStream) -> TokenStream {
    let mut options = Options::default();
    let parser = syn::meta::parser(|meta| options.parse(&meta));
    parse_macro_input!(args with parser);
    let item = parse_macro_input!(item as ItemStruct);
    expand(options, item)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Stores a type in a column: implements `rustique::FromValue` and
/// `From<T> for rustique::Value`, so the type can be a model field and a binding.
///
/// - Unit-only enum: stored as text, the variant name in snake_case
///   (`PendingReview` is `"pending_review"`); `#[cast(rename = "x")]` on a
///   variant overrides it.
/// - Unit-only enum with `#[cast(int)]`: stored as its discriminant.
/// - One-field tuple struct (`struct Email(String)`): stored as its field.
/// - Any `Serialize + Deserialize` type with `#[cast(json)]`: stored as JSON.
///
/// `Option<T>` maps `NULL` to `None` as for every `FromValue` type.
#[proc_macro_derive(Cast, attributes(cast))]
pub fn derive_cast(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    cast::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Makes an enum of models the target of `#[rustique(morph_to)]` fields: one
/// tuple variant per model, `Post(Post)`. A row's `*_type` column picks the
/// variant whose model has that `rustique::Model::MORPH_ALIAS`.
#[proc_macro_derive(MorphTo)]
pub fn derive_morph_to(item: TokenStream) -> TokenStream {
    let input = parse_macro_input!(item as syn::DeriveInput);
    morph::expand(input)
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns `scope_*` functions of an `impl Model` block into query methods.
///
/// `fn scope_active(q: Query<User>) -> Query<User>` becomes
/// `User::query().active()` and `User::active()`; further arguments carry
/// over (`scope_of_type(q, ty: &str)` becomes `.of_type("admin")`). The query
/// methods live in a generated trait `UserScopes`, implemented for
/// `rustique::Query<User>`; import it where the scopes are used outside the
/// defining module. Other items of the block are left as they are.
///
/// As in Eloquent, the WHERE clauses before a scope and those it adds are
/// each wrapped in parentheses when they contain an `OR`, so
/// `.where_(tenant).visible()` with an `OR` inside `visible` stays within the tenant.
///
/// A scope named like a Rust keyword (`scope_type`) becomes `r#type`. A
/// scope cannot share its name with another method of the model, such as a
/// relation, nor with an associated function of `rustique::Model` such as `find`.
#[proc_macro_attribute]
pub fn scopes(args: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemImpl);
    let result = if args.is_empty() {
        expand_scopes(item)
    } else {
        Err(syn::Error::new(
            Span::call_site(),
            "#[rustique::scopes] takes no options",
        ))
    };
    result.unwrap_or_else(syn::Error::into_compile_error).into()
}

fn expand_scopes(item: ItemImpl) -> syn::Result<TokenStream2> {
    let self_ty = &item.self_ty;
    let model = match &**self_ty {
        Type::Path(path) if item.generics.params.is_empty() && item.trait_.is_none() => {
            type_name(&path.path)
        }
        _ => {
            return Err(syn::Error::new_spanned(
                self_ty,
                "#[rustique::scopes] needs an `impl Model { ... }` block without generics",
            ));
        }
    };
    let scopes_trait = format_ident!("{model}Scopes");
    let mut declarations = Vec::new();
    let mut definitions = Vec::new();
    let mut statics = Vec::new();
    for impl_item in &item.items {
        let ImplItem::Fn(function) = impl_item else {
            continue;
        };
        let sig = &function.sig;
        let Some(name) = sig
            .ident
            .unraw()
            .to_string()
            .strip_prefix("scope_")
            .map(str::to_owned)
        else {
            continue;
        };
        let mut inputs = sig.inputs.iter();
        if !matches!(inputs.next(), Some(FnArg::Typed(_))) {
            return Err(syn::Error::new_spanned(
                sig,
                "a scope takes the query as its first argument: `fn scope_x(q: Query<Self>) -> Query<Self>`",
            ));
        }
        let mut types = Vec::new();
        for input in inputs {
            match input {
                FnArg::Typed(arg) => types.push(&arg.ty),
                FnArg::Receiver(_) => unreachable!("`self` is always first"),
            }
        }
        let args: Vec<Ident> = (0..types.len()).map(|i| format_ident!("arg{i}")).collect();
        let method = scope_ident(&name, sig)?;
        let scope = &sig.ident;
        let (generics, where_clause) = (&sig.generics, &sig.generics.where_clause);
        let mut docs: Vec<&Attribute> = function
            .attrs
            .iter()
            .filter(|a| a.path().is_ident("doc"))
            .collect();
        let default_doc: Attribute = {
            let doc = format!("Scope `{name}`.");
            syn::parse_quote!(#[doc = #doc])
        };
        if docs.is_empty() {
            docs.push(&default_doc);
        }
        let signature = quote! {
            fn #method #generics (self, #(#args: #types),*) -> Self #where_clause
        };
        declarations.push(quote! {
            #(#docs)*
            #signature;
        });
        definitions.push(quote! {
            #signature {
                ::rustique::__private::scope(self, move |q| <#self_ty>::#scope(q, #(#args),*))
            }
        });
        statics.push(quote! {
            #(#docs)*
            pub fn #method #generics (#(#args: #types),*) -> ::rustique::Query<Self> #where_clause {
                ::rustique::__private::scope(
                    <Self as ::rustique::Model>::query(),
                    move |q| <#self_ty>::#scope(q, #(#args),*),
                )
            }
        });
    }
    let doc = format!("Query scopes of `{model}`, from its `#[rustique::scopes]` block.");
    Ok(quote! {
        #item

        #[doc = #doc]
        pub trait #scopes_trait: ::core::marker::Sized {
            #(#declarations)*
        }

        impl #scopes_trait for ::rustique::Query<#self_ty> {
            #(#definitions)*
        }

        impl #self_ty {
            #(#statics)*
        }
    })
}

/// `rustique::Model` associated functions: an inherent `User::find()` would
/// silently hide `Model::find`.
const MODEL_FUNCTIONS: &[&str] = &[
    "query",
    "all",
    "find",
    "find_or_fail",
    "find_many",
    "create",
    "first_or_new",
    "first_or_create",
    "create_or_first",
    "update_or_create",
    "insert_many",
    "upsert",
    "from_row",
    "key_values",
    "relation",
    "load_relation",
];
/// `rustique::Query` methods: inherent methods win over trait methods, so
/// `User::query().latest()` would silently call `Query::latest`, not the scope.
fn query_methods() -> impl Iterator<Item = &'static str> {
    include_str!("query_methods.txt")
        .lines()
        .filter(|l| !l.is_empty())
}

/// Method name of scope `name`: raw for keywords (`r#type`). Names in
/// [`MODEL_FUNCTIONS`] and `query_methods.txt` are refused.
fn scope_ident(name: &str, sig: &syn::Signature) -> syn::Result<Ident> {
    if MODEL_FUNCTIONS.contains(&name) {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            format!("scope `{name}` would hide `rustique::Model::{name}`; rename it"),
        ));
    }
    if query_methods().any(|m| m == name) {
        return Err(syn::Error::new_spanned(
            &sig.ident,
            format!("scope `{name}` would be hidden by `rustique::Query::{name}`; rename it"),
        ));
    }
    syn::parse_str::<Ident>(name)
        .or_else(|_| syn::parse_str::<Ident>(&format!("r#{name}")))
        .map(|mut ident| {
            ident.set_span(sig.ident.span());
            ident
        })
        .map_err(|_| {
            syn::Error::new_spanned(&sig.ident, format!("`{name}` is not a valid method name"))
        })
}

#[derive(Default)]
struct Options {
    table: Option<LitStr>,
    timestamps: bool,
    soft_deletes: bool,
    incrementing: Option<bool>,
    hooks: bool,
    observers: Vec<Path>,
    global_scopes: Vec<Path>,
    morph_alias: Option<LitStr>,
    /// `uuid` or `ulid`: the key is generated on insert when empty.
    unique_id: Option<Ident>,
    /// Renamed `created_at` / `updated_at` / `deleted_at` columns.
    created_at: Option<LitStr>,
    updated_at: Option<LitStr>,
    deleted_at: Option<LitStr>,
}

impl Options {
    fn parse(&mut self, meta: &ParseNestedMeta) -> syn::Result<()> {
        if meta.path.is_ident("table") {
            self.table = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("timestamps") {
            self.timestamps = true;
        } else if meta.path.is_ident("soft_deletes") {
            self.soft_deletes = true;
        } else if meta.path.is_ident("incrementing") {
            self.incrementing = Some(meta.value()?.parse::<LitBool>()?.value);
        } else if meta.path.is_ident("hooks") {
            self.hooks = true;
        } else if meta.path.is_ident("uuid") || meta.path.is_ident("ulid") {
            if self.unique_id.is_some() {
                return Err(meta.error("use one of `uuid` and `ulid`"));
            }
            self.unique_id = meta.path.get_ident().cloned();
        } else if meta.path.is_ident("morph_alias") {
            self.morph_alias = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("created_at") {
            self.created_at = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("updated_at") {
            self.updated_at = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("deleted_at") {
            self.deleted_at = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("observers") {
            meta.parse_nested_meta(|observer| {
                self.observers.push(observer.path);
                Ok(())
            })?;
        } else if meta.path.is_ident("global_scopes") {
            meta.parse_nested_meta(|scope| {
                self.global_scopes.push(scope.path);
                Ok(())
            })?;
        } else {
            return Err(meta.error(
                "unknown model option; expected `table`, `timestamps`, `soft_deletes`, \
                 `incrementing`, `hooks`, `observers`, `global_scopes`, `morph_alias`, `uuid`, `ulid`, \
                 `created_at`, `updated_at` or `deleted_at`",
            ));
        }
        Ok(())
    }
}

struct Column {
    ident: Ident,
    ty: Type,
    /// Column name: the field name unless `#[rustique(column = "...")]` renames it.
    name: String,
    primary_key: bool,
    sequence: Option<LitStr>,
    /// `#[rustique(fillable)]`: in the `{Model}Fill` struct. Columns are guarded otherwise.
    fillable: bool,
    /// `#[rustique(not_null)]`: the column is nullable but never holds `NULL`, so
    /// the `schema-check` feature accepts a field that is not an `Option`.
    not_null: bool,
}

/// A `Rel` field filled by a relation.
struct RelationField {
    ident: Ident,
    vis: Visibility,
    name: String,
    kind: RelationKind,
    related: Path,
    keys: HashMap<String, Vec<LitStr>>,
    table: Option<LitStr>,
    through: Option<Path>,
    /// `pivot_timestamps` on `belongs_to_many`: the created / updated columns.
    pivot_timestamps: Option<[LitStr; 2]>,
    /// `scope = expr`: a `fn(Query<R>) -> Query<R>` constraining the relation.
    scope: Option<syn::Expr>,
    /// `latest_of_many [= "column"]` / `oldest_of_many [= "column"]` /
    /// `of_many(column = "max" | "min", ...)` on `has_one` / `morph_one`:
    /// `(column, highest wins)` ranked before the key, and whether the
    /// highest key wins ties.
    of_many: Option<(Vec<(LitStr, bool)>, bool)>,
}

#[derive(Clone, Copy, PartialEq)]
enum RelationKind {
    HasOne,
    HasMany,
    BelongsTo,
    BelongsToMany,
    HasManyThrough,
    HasOneThrough,
    MorphOne,
    MorphMany,
    MorphToMany,
    MorphedByMany,
}

impl RelationKind {
    fn parse(path: &Path) -> Option<Self> {
        let ident = path.get_ident()?;
        Some(match ident.to_string().as_str() {
            "has_one" => Self::HasOne,
            "has_many" => Self::HasMany,
            "belongs_to" => Self::BelongsTo,
            "belongs_to_many" => Self::BelongsToMany,
            "has_many_through" => Self::HasManyThrough,
            "has_one_through" => Self::HasOneThrough,
            "morph_one" => Self::MorphOne,
            "morph_many" => Self::MorphMany,
            "morph_to_many" => Self::MorphToMany,
            "morphed_by_many" => Self::MorphedByMany,
            _ => return None,
        })
    }

    fn options(self) -> &'static [&'static str] {
        match self {
            Self::HasOne | Self::HasMany => &["foreign_key", "local_key"],
            Self::BelongsTo => &["foreign_key", "owner_key"],
            Self::BelongsToMany => &[
                "table",
                "foreign_pivot_key",
                "related_pivot_key",
                "parent_key",
                "related_key",
                "pivot",
                "pivot_timestamps",
            ],
            Self::HasManyThrough | Self::HasOneThrough => &[
                "through",
                "first_key",
                "second_key",
                "local_key",
                "second_local_key",
            ],
            Self::MorphOne | Self::MorphMany => &["name", "local_key"],
            Self::MorphToMany | Self::MorphedByMany => &[
                "name",
                "table",
                "foreign_pivot_key",
                "related_pivot_key",
                "parent_key",
                "related_key",
                "pivot",
                "pivot_timestamps",
            ],
        }
    }

    /// Relations through a pivot table, with `table`, `pivot` and `pivot_timestamps`.
    fn has_pivot(self) -> bool {
        matches!(
            self,
            Self::BelongsToMany | Self::MorphToMany | Self::MorphedByMany
        )
    }

    /// Relations that need `name = "..."`.
    fn is_morph(self) -> bool {
        matches!(
            self,
            Self::MorphOne | Self::MorphMany | Self::MorphToMany | Self::MorphedByMany
        )
    }
}

#[allow(clippy::large_enum_variant)] // a few per struct, at compile time
enum Parsed {
    Column(Column),
    Relation(RelationField),
    /// `#[rustique(count)]` / `#[rustique(aggregate)]`: a `Rel<T>` read from the
    /// `<field>` column when present.
    Count(Ident, String),
    MorphTo(MorphToField),
}

/// `#[rustique(morph_to)]` on a `Rel<Option<T>>` field: `T` derives `MorphTo`, and
/// the `{name}_type` / `{name}_id` columns point at the parent.
struct MorphToField {
    ident: Ident,
    vis: Visibility,
    name: String,
    /// Column prefix: the field name unless `name = "..."` is given.
    columns: String,
    target: Type,
}

/// Reads and strips the `#[rustique(...)]` attributes of `field`. Relation and
/// count fields get `serde_attr`, if any.
fn parse_field(field: &mut Field, serde_attr: Option<&Attribute>) -> syn::Result<Parsed> {
    let ident = field.ident.clone().expect("named field");
    let name = ident.unraw().to_string();
    let mut column = Column {
        ident: ident.clone(),
        ty: field.ty.clone(),
        name: name.clone(),
        primary_key: false,
        sequence: None,
        fillable: false,
        not_null: false,
    };
    let mut relation = None;
    let mut morph_to = None;
    let mut count = false;
    let mut result = Ok(());
    field.attrs.retain(|attr| {
        if !attr.path().is_ident("rustique") {
            return true;
        }
        let parsed = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("primary_key") {
                column.primary_key = true;
            } else if meta.path.is_ident("fillable") {
                column.fillable = true;
            } else if meta.path.is_ident("not_null") {
                column.not_null = true;
            } else if meta.path.is_ident("guarded") {
                return Err(meta.error(
                    "`guarded` was removed: columns are guarded unless marked `#[rustique(fillable)]`",
                ));
            } else if meta.path.is_ident("sequence") {
                column.sequence = Some(meta.value()?.parse()?);
            } else if meta.path.is_ident("column") {
                column.name = meta.value()?.parse::<LitStr>()?.value();
            } else if meta.path.is_ident("count") || meta.path.is_ident("aggregate") {
                count = true;
            } else if meta.path.is_ident("morph_to") {
                morph_to = Some(parse_morph_to(&meta, &field.ty, &ident, &field.vis, &name)?);
            } else if let Some(kind) = RelationKind::parse(&meta.path) {
                relation = Some(parse_relation(&meta, kind, &ident, &field.vis, &name)?);
            } else {
                return Err(meta.error(
                    "unknown field option; expected `primary_key`, `column`, `sequence`, `fillable`, `not_null`, `count`, `aggregate`, \
                     `has_one`, `has_many`, `belongs_to`, `belongs_to_many`, `has_many_through`, \
                     `has_one_through`, `morph_one`, `morph_many`, `morph_to`, `morph_to_many` or \
                     `morphed_by_many`",
                ));
            }
            Ok(())
        });
        if result.is_ok() {
            result = parsed;
        }
        false
    });
    result?;
    if relation.is_some() || morph_to.is_some() || count {
        field.attrs.extend(serde_attr.cloned());
    }
    if let Some(morph_to) = morph_to {
        return Ok(Parsed::MorphTo(morph_to));
    }
    Ok(match relation {
        Some(relation) => Parsed::Relation(relation),
        None if count => Parsed::Count(ident, name),
        None => Parsed::Column(column),
    })
}

/// `has_many(Post, foreign_key = "user_id", ...)`
fn parse_relation(
    meta: &ParseNestedMeta,
    kind: RelationKind,
    ident: &Ident,
    vis: &Visibility,
    name: &str,
) -> syn::Result<RelationField> {
    let mut related = None;
    let mut keys = HashMap::new();
    let mut table = None;
    let mut through = None;
    let mut pivot_timestamps = None;
    let mut scope = None;
    let mut of_many = None;
    meta.parse_nested_meta(|option| {
        let bare = option.input.is_empty() || option.input.peek(Token![,]);
        if bare && related.is_none() {
            related = Some(option.path.clone());
            return Ok(());
        }
        let key = option.path.get_ident().map(ToString::to_string);
        match key.as_deref() {
            Some("table") if kind.has_pivot() => {
                table = Some(option.value()?.parse()?);
            }
            Some("through")
                if matches!(
                    kind,
                    RelationKind::HasManyThrough | RelationKind::HasOneThrough
                ) =>
            {
                through = Some(option.value()?.parse()?);
            }
            Some("pivot_timestamps") if kind.has_pivot() => {
                let names = if bare {
                    vec![
                        LitStr::new("created_at", Span::call_site()),
                        LitStr::new("updated_at", Span::call_site()),
                    ]
                } else {
                    parse_keys(option.value()?)?
                };
                pivot_timestamps = Some(names.try_into().map_err(|_| {
                    option.error("expected `pivot_timestamps = [\"created\", \"updated\"]`")
                })?);
            }
            Some("scope") => scope = Some(option.value()?.parse()?),
            Some(key @ ("latest_of_many" | "oldest_of_many" | "of_many"))
                if matches!(kind, RelationKind::HasOne | RelationKind::MorphOne) =>
            {
                if of_many.is_some() {
                    return Err(
                        option.error("use one of `latest_of_many`, `oldest_of_many` and `of_many`")
                    );
                }
                if key == "of_many" {
                    // `of_many(published_at = "max", id = "min")`, like Eloquent's `ofMany([...])`.
                    let mut columns = Vec::new();
                    option.parse_nested_meta(|column| {
                        let name = column
                            .path
                            .get_ident()
                            .ok_or_else(|| column.error("expected a column name"))?;
                        let aggregate: LitStr = column.value()?.parse()?;
                        let descending = match aggregate.value().to_ascii_lowercase().as_str() {
                            "max" => true,
                            "min" => false,
                            _ => return Err(column.error("expected `\"max\"` or `\"min\"`")),
                        };
                        columns.push((LitStr::new(&name.to_string(), name.span()), descending));
                        Ok(())
                    })?;
                    if columns.is_empty() {
                        return Err(option.error("expected `of_many(column = \"max\", ...)`"));
                    }
                    // Eloquent adds the key as `MAX` when the columns leave it out.
                    of_many = Some((columns, true));
                } else {
                    let descending = key == "latest_of_many";
                    let columns = if bare {
                        Vec::new()
                    } else {
                        vec![(option.value()?.parse()?, descending)]
                    };
                    of_many = Some((columns, descending));
                }
            }
            Some(key) if kind.options().contains(&key) => {
                keys.insert(key.to_owned(), parse_keys(option.value()?)?);
            }
            _ => {
                let mut options = kind.options().to_vec();
                if matches!(kind, RelationKind::HasOne | RelationKind::MorphOne) {
                    options.extend(["latest_of_many", "oldest_of_many", "of_many"]);
                }
                options.push("scope");
                return Err(option.error(format!(
                    "expected the related model, then any of: {}",
                    options.join(", ")
                )));
            }
        }
        Ok(())
    })?;
    let related = related.ok_or_else(|| meta.error("name the related model first"))?;
    if kind.is_morph() && keys.get("name").is_none_or(|name| name.len() != 1) {
        return Err(meta.error(
            "morph_one / morph_many / morph_to_many / morphed_by_many need `name = \"...\"`",
        ));
    }
    let through_kind = matches!(
        kind,
        RelationKind::HasManyThrough | RelationKind::HasOneThrough
    );
    if through_kind && through.is_none() {
        return Err(meta.error("has_many_through / has_one_through need `through = Model`"));
    }
    Ok(RelationField {
        ident: ident.clone(),
        vis: vis.clone(),
        name: name.to_owned(),
        kind,
        related,
        keys,
        table,
        through,
        pivot_timestamps,
        scope,
        of_many,
    })
}

/// `morph_to` or `morph_to(name = "commentable")`.
fn parse_morph_to(
    meta: &ParseNestedMeta,
    ty: &Type,
    ident: &Ident,
    vis: &Visibility,
    name: &str,
) -> syn::Result<MorphToField> {
    let mut columns = name.to_owned();
    if !meta.input.is_empty() && !meta.input.peek(Token![,]) {
        meta.parse_nested_meta(|option| {
            if option.path.is_ident("name") {
                columns = option.value()?.parse::<LitStr>()?.value();
                Ok(())
            } else {
                Err(option.error("expected `name = \"...\"`"))
            }
        })?;
    }
    let target = rel_option_inner(ty).ok_or_else(|| {
        syn::Error::new_spanned(ty, "a `morph_to` field must be `Rel<Option<T>>`")
    })?;
    Ok(MorphToField {
        ident: ident.clone(),
        vis: vis.clone(),
        name: name.to_owned(),
        columns,
        target,
    })
}

/// `T` of `Rel<Option<T>>`.
fn rel_option_inner(ty: &Type) -> Option<Type> {
    let generic = |ty: &Type, name: &str| -> Option<Type> {
        let Type::Path(path) = ty else { return None };
        let segment = path.path.segments.last()?;
        if segment.ident != name {
            return None;
        }
        let syn::PathArguments::AngleBracketed(args) = &segment.arguments else {
            return None;
        };
        match args.args.first()? {
            syn::GenericArgument::Type(inner) => Some(inner.clone()),
            _ => None,
        }
    };
    generic(&generic(ty, "Rel")?, "Option")
}

/// `"a"` or `["a", "b"]`.
fn parse_keys(input: ParseStream) -> syn::Result<Vec<LitStr>> {
    if input.peek(syn::token::Bracket) {
        let content;
        syn::bracketed!(content in input);
        let keys = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
        Ok(keys.into_iter().collect())
    } else {
        Ok(vec![input.parse()?])
    }
}

/// `BlogPost` becomes `blog_post`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

fn type_name(path: &Path) -> String {
    path.segments
        .last()
        .map(|s| s.ident.to_string())
        .unwrap_or_default()
}

impl RelationField {
    /// The `rustique::__private::Relation` describing this field, with
    /// Eloquent's default keys.
    fn relation(&self, parent: &Ident) -> TokenStream2 {
        let related = &self.related;
        let keys = |option: &str, default: TokenStream2| match self.keys.get(option) {
            Some(keys) => quote!(&[#(#keys),*]),
            None => default,
        };
        let primary_key =
            |model: &dyn quote::ToTokens| quote!(<#model as ::rustique::Model>::PRIMARY_KEY);
        let id = |name: String| {
            let column = format!("{name}_id");
            quote!(&[#column])
        };
        let parent_snake = snake(&parent.to_string());
        let this = quote!(Self);
        let (local, remote) = match self.kind {
            RelationKind::HasOne | RelationKind::HasMany => (
                keys("local_key", primary_key(&this)),
                keys("foreign_key", id(parent_snake.clone())),
            ),
            RelationKind::BelongsTo => (
                keys("foreign_key", id(self.name.clone())),
                keys("owner_key", primary_key(related)),
            ),
            RelationKind::BelongsToMany => (
                keys("parent_key", primary_key(&this)),
                keys("foreign_pivot_key", id(parent_snake.clone())),
            ),
            RelationKind::HasManyThrough | RelationKind::HasOneThrough => (
                keys("local_key", primary_key(&this)),
                keys("first_key", id(parent_snake.clone())),
            ),
            RelationKind::MorphOne | RelationKind::MorphMany => {
                (keys("local_key", primary_key(&this)), id(self.morph_name()))
            }
            RelationKind::MorphToMany => (
                keys("parent_key", primary_key(&this)),
                keys("foreign_pivot_key", id(self.morph_name())),
            ),
            RelationKind::MorphedByMany => (
                keys("parent_key", primary_key(&this)),
                keys("foreign_pivot_key", id(parent_snake.clone())),
            ),
        };
        let morph = match self.kind {
            RelationKind::MorphOne | RelationKind::MorphMany => {
                let column = format!("{}_type", self.morph_name());
                quote!(::core::option::Option::Some((#column, <Self as ::rustique::Model>::MORPH_ALIAS)))
            }
            _ => quote!(::core::option::Option::None),
        };
        let through = match (self.kind, &self.through) {
            (kind, _) if kind.has_pivot() => {
                let related_snake = snake(&type_name(related));
                let table = self.pivot_table(&parent_snake);
                let related_pivot = match kind {
                    RelationKind::MorphedByMany => id(self.morph_name()),
                    _ => id(related_snake),
                };
                let through_key = keys("related_pivot_key", related_pivot);
                let related_key = keys("related_key", primary_key(related));
                let pivot = keys("pivot", quote!(&[]));
                let timestamps = match &self.pivot_timestamps {
                    Some([created, updated]) => {
                        quote!(::core::option::Option::Some([#created, #updated]))
                    }
                    None => quote!(::core::option::Option::None),
                };
                let pivot_morph = match kind {
                    RelationKind::MorphToMany | RelationKind::MorphedByMany => {
                        let column = format!("{}_type", self.morph_name());
                        let model = match kind {
                            RelationKind::MorphToMany => quote!(Self),
                            _ => quote!(#related),
                        };
                        quote!(::core::option::Option::Some((
                            #column,
                            <#model as ::rustique::Model>::MORPH_ALIAS,
                        )))
                    }
                    _ => quote!(::core::option::Option::None),
                };
                quote!(::core::option::Option::Some(::rustique::__private::Through {
                    table: #table,
                    through_key: #through_key,
                    related_key: #related_key,
                    pivot: #pivot,
                    timestamps: #timestamps,
                    morph: #pivot_morph,
                }))
            }
            (_, Some(through)) => {
                let through_key = keys("second_local_key", primary_key(through));
                let related_key = keys("second_key", id(snake(&type_name(through))));
                quote!(::core::option::Option::Some(::rustique::__private::Through {
                    table: <#through as ::rustique::Model>::TABLE,
                    through_key: #through_key,
                    related_key: #related_key,
                    pivot: &[],
                    timestamps: ::core::option::Option::None,
                    morph: ::core::option::Option::None,
                }))
            }
            _ => quote!(::core::option::Option::None),
        };
        let kind = match self.kind {
            RelationKind::HasOne => quote!(HasOne),
            RelationKind::HasMany => quote!(HasMany),
            RelationKind::BelongsTo => quote!(BelongsTo),
            // Pivot relations share the pivot code; the morph type sits in `Through`.
            RelationKind::BelongsToMany
            | RelationKind::MorphToMany
            | RelationKind::MorphedByMany => quote!(BelongsToMany),
            RelationKind::HasManyThrough => quote!(HasManyThrough),
            RelationKind::HasOneThrough => quote!(HasOneThrough),
            RelationKind::MorphOne => quote!(MorphOne),
            RelationKind::MorphMany => quote!(MorphMany),
        };
        let of_many = match &self.of_many {
            Some((columns, descending)) => {
                let columns = columns
                    .iter()
                    .map(|(column, desc)| quote!((#column, #desc)));
                quote!(::core::option::Option::Some(::rustique::__private::OfMany {
                    columns: &[#(#columns),*],
                    descending: #descending,
                    key: <#related as ::rustique::Model>::PRIMARY_KEY,
                }))
            }
            None => quote!(::core::option::Option::None),
        };
        let scope = self.scope_fn();
        quote!(::rustique::__private::Relation {
            kind: ::rustique::__private::RelationKind::#kind,
            related_table: <#related as ::rustique::Model>::TABLE,
            related_query: || ::rustique::Query::into_builder(::rustique::__private::scope(
                <#related as ::rustique::Model>::query(),
                #scope,
            )),
            local: #local,
            remote: #remote,
            through: #through,
            morph: #morph,
            of_many: #of_many,
        })
    }

    /// The relation's `scope` as a function pointer, or `|q| q`.
    fn scope_fn(&self) -> TokenStream2 {
        let related = &self.related;
        let scope = match &self.scope {
            Some(scope) => quote!(#scope),
            None => quote!(|query| query),
        };
        quote!({
            let scope: ::rustique::__private::RelationScope<#related> = #scope;
            scope
        })
    }

    /// The pivot table of a `has_pivot` relation: `table`, or Eloquent's default.
    fn pivot_table(&self, parent_snake: &str) -> String {
        match (&self.table, self.kind) {
            (Some(table), _) => table.value(),
            (None, RelationKind::BelongsToMany) => {
                let mut names = [parent_snake.to_owned(), snake(&type_name(&self.related))];
                names.sort();
                names.join("_")
            }
            // Eloquent's plural of the morph name: `taggable` → `taggables`.
            (None, _) => format!("{}s", self.morph_name()),
        }
    }

    /// `name` of a morph relation, checked at parse time.
    fn morph_name(&self) -> String {
        self.keys["name"][0].value()
    }

    /// `user.posts()` and the typed name `User::POSTS`, plus
    /// `user.roles_pivot()` for `belongs_to_many`.
    fn methods(&self) -> TokenStream2 {
        let Self {
            ident,
            vis,
            name,
            related,
            ..
        } = self;
        let doc = format!("Query of the `{name}` relation.");
        let scope = self.scope_fn();
        let constant = format_ident!("{}", ident.unraw().to_string().to_uppercase());
        let constant_doc = format!("The `{name}` relation, for `Query::with_typed` and friends.");
        let mut methods = quote! {
            #[doc = #constant_doc]
            #vis const #constant: ::rustique::RelName<Self, #related> = ::rustique::RelName::new(#name);
            #[doc = #doc]
            #vis fn #ident(&self) -> ::rustique::Query<#related> {
                ::rustique::__private::query::<Self, #related>(self, #name, #scope)
            }
        };
        if self.kind.has_pivot() {
            let pivot = format_ident!("{}_pivot", ident.unraw());
            let doc = format!("Pivot rows of the `{name}` relation.");
            methods.extend(quote! {
                #[doc = #doc]
                ///
                /// # Errors
                ///
                /// When the model has no key yet.
                #vis fn #pivot(&self) -> ::rustique::Result<::rustique::Pivot<#related>> {
                    ::rustique::Pivot::<#related>::new::<Self>(self, #name)
                }
            });
        }
        methods
    }
}

fn expand(options: Options, mut item: ItemStruct) -> syn::Result<TokenStream2> {
    let table = options
        .table
        .clone()
        .ok_or_else(|| syn::Error::new(Span::call_site(), "missing option `table = \"...\"`"))?;
    let serde = serde_conditions(&item.attrs, &["Serialize", "Deserialize"])?;
    let deserialize = serde_conditions(&item.attrs, &["Deserialize"])?;
    let serde_attr = |attr: TokenStream2| -> Option<Attribute> {
        (!serde.is_empty()).then(|| syn::parse_quote!(#[cfg_attr(any(#(#serde),*), serde(#attr))]))
    };
    let relation_serde = serde_attr(quote!(
        default,
        skip_serializing_if = "::rustique::Rel::is_not_loaded"
    ));
    let Fields::Named(named) = &mut item.fields else {
        return Err(syn::Error::new_spanned(
            &item.ident,
            "#[rustique::model] needs a struct with named fields",
        ));
    };
    let mut fields = ModelFields::parse(&mut named.named, relation_serde.as_ref())?;
    mark_default_key(&mut fields.columns, &item.ident)?;
    let key = Key::new(&options, &fields.columns, &item.ident)?;
    let managed = managed_columns(&options, &fields.columns, &item.ident)?;
    let serde_skip = serde_attr(quote!(skip));
    named.named.push(Field::parse_named.parse2(quote! {
        #[doc(hidden)]
        /// Bookkeeping for `rustique::Model`.
        #serde_skip
        pub __meta: ::rustique::Meta
    })?);

    let fill = expand_fill(&item, &fields.columns, &managed, &deserialize)?;
    #[cfg(feature = "schema-check")]
    let schema_check = schema_check::check(
        &table,
        &item.ident,
        &fields.columns,
        &fields.relations,
        &fields.morphs,
    )?;
    #[cfg(not(feature = "schema-check"))]
    let schema_check = TokenStream2::new();
    let ident = &item.ident;
    let (impl_generics, type_generics, where_clause) = item.generics.split_for_impl();
    let constants = fields.columns.iter().map(Column::constant);
    let methods = fields.relations.iter().map(RelationField::methods);
    let morph_methods = fields.morphs.iter().map(MorphToField::method);
    let model_constants = model_constants(&options, &table, &fields.columns, &key);
    let row_fns = row_fns(&fields, &key);
    let relation_fns = relation_fns(ident, &fields);
    let option_fns = option_fns(&options);
    let hooks_impl = (!options.hooks).then(
        || quote!(impl #impl_generics ::rustique::Hooks for #ident #type_generics #where_clause {}),
    );

    Ok(quote! {
        #item

        #fill

        #schema_check

        impl #impl_generics #ident #type_generics #where_clause {
            #(#constants)*
            #(#methods)*
            #(#morph_methods)*
        }

        impl #impl_generics ::rustique::Model for #ident #type_generics #where_clause {
            #model_constants
            #row_fns
            #relation_fns
            #option_fns
        }

        #hooks_impl
    })
}

/// The fields of a model struct, by what they map to.
struct ModelFields {
    columns: Vec<Column>,
    relations: Vec<RelationField>,
    /// `#[rustique(count)]` / `#[rustique(aggregate)]` fields and the column each reads.
    counts: Vec<(Ident, String)>,
    morphs: Vec<MorphToField>,
}

impl ModelFields {
    /// Sorts `fields`, stripping their `#[rustique(...)]` attributes; relation and
    /// count fields get `relation_serde`, if any.
    fn parse(
        fields: &mut Punctuated<Field, Token![,]>,
        relation_serde: Option<&Attribute>,
    ) -> syn::Result<Self> {
        let mut parsed = Self {
            columns: Vec::new(),
            relations: Vec::new(),
            counts: Vec::new(),
            morphs: Vec::new(),
        };
        for field in fields {
            match parse_field(field, relation_serde)? {
                // Deserializing the model reads every column, for trusted data
                // (caches, round trips); untrusted input goes through `{Model}Fill`.
                Parsed::Column(column) => parsed.columns.push(column),
                Parsed::Relation(relation) => parsed.relations.push(relation),
                Parsed::Count(ident, name) => parsed.counts.push((ident, name)),
                Parsed::MorphTo(morph) => parsed.morphs.push(morph),
            }
        }
        Ok(parsed)
    }
}

/// Makes the `id` field the primary key when no field is marked `primary_key`.
fn mark_default_key(columns: &mut [Column], model: &Ident) -> syn::Result<()> {
    if columns.iter().any(|c| c.primary_key) {
        return Ok(());
    }
    let id = columns
        .iter_mut()
        .find(|c| c.ident.unraw() == "id")
        .ok_or_else(|| {
            syn::Error::new_spanned(
                model,
                "no primary key: add an `id` field or mark fields `#[rustique(primary_key)]`",
            )
        })?;
    id.primary_key = true;
    Ok(())
}

/// The primary key of a model.
struct Key<'a> {
    columns: Vec<&'a Column>,
    /// The database generates the key: `Model::INCREMENTING`.
    incrementing: bool,
    /// The Oracle sequence that fills the key.
    sequence: Option<&'a LitStr>,
}

impl<'a> Key<'a> {
    /// The key of `columns`, checked against the model `options`.
    fn new(options: &Options, columns: &'a [Column], model: &Ident) -> syn::Result<Self> {
        let keys: Vec<&Column> = columns.iter().filter(|c| c.primary_key).collect();
        let incrementing = options
            .incrementing
            .unwrap_or(keys.len() == 1 && options.unique_id.is_none());
        if incrementing && keys.len() > 1 {
            return Err(syn::Error::new_spanned(
                model,
                "a composite primary key cannot be incrementing",
            ));
        }
        if let Some(unique_id) = &options.unique_id
            && (incrementing || keys.len() > 1)
        {
            return Err(syn::Error::new_spanned(
                unique_id,
                "`uuid` / `ulid` need a single-column key that is not incrementing",
            ));
        }
        let mut sequence = None;
        for column in columns {
            if let Some(name) = &column.sequence {
                if !column.primary_key || !incrementing {
                    return Err(syn::Error::new_spanned(
                        name,
                        "`sequence` belongs on the primary key of an incrementing model",
                    ));
                }
                sequence = Some(name);
            }
        }
        Ok(Self {
            columns: keys,
            incrementing,
            sequence,
        })
    }

    /// `type Key`: the key field's type, or a tuple for a composite key.
    fn ty(&self) -> TokenStream2 {
        let types = self.columns.iter().map(|c| &c.ty);
        match &self.columns[..] {
            [_] => quote!(#(#types)*),
            _ => quote!((#(#types),*)),
        }
    }

    /// `key_values`: one `Value` per key column.
    fn values_fn(&self) -> TokenStream2 {
        let vars: Vec<Ident> = (0..self.columns.len())
            .map(|i| format_ident!("key{i}"))
            .collect();
        let pattern = match &vars[..] {
            [_] => quote!(#(#vars)*),
            _ => quote!((#(#vars),*)),
        };
        quote! {
            fn key_values(key: Self::Key) -> ::std::vec::Vec<::rustique::Value> {
                let #pattern = key;
                ::std::vec![#(::rustique::Value::from(#vars)),*]
            }
        }
    }
}

/// The `timestamps` / `soft_deletes` columns, which the model must have.
fn managed_columns(
    options: &Options,
    columns: &[Column],
    model: &Ident,
) -> syn::Result<Vec<String>> {
    let name = |option: &Option<LitStr>, default: &str| {
        option
            .as_ref()
            .map_or_else(|| default.to_owned(), LitStr::value)
    };
    let mut managed = Vec::new();
    if options.timestamps {
        managed.push(name(&options.created_at, "created_at"));
        managed.push(name(&options.updated_at, "updated_at"));
    }
    if options.soft_deletes {
        managed.push(name(&options.deleted_at, "deleted_at"));
    }
    if let Some(missing) = managed
        .iter()
        .find(|name| !columns.iter().any(|c| &c.name == *name))
    {
        return Err(syn::Error::new_spanned(
            model,
            format!("the model needs a `{missing}` column"),
        ));
    }
    Ok(managed)
}

impl Column {
    /// `pub const EMAIL: &'static str = "email";`
    fn constant(&self) -> TokenStream2 {
        let constant = format_ident!("{}", self.ident.unraw().to_string().to_uppercase());
        let name = &self.name;
        let doc = format!("Column `{name}`.");
        quote! {
            #[doc = #doc]
            pub const #constant: &'static str = #name;
        }
    }
}

impl MorphToField {
    /// `["{name}_type", "{name}_id"]`
    fn column_pair(&self) -> TokenStream2 {
        let kind = format!("{}_type", self.columns);
        let id = format!("{}_id", self.columns);
        quote!([#kind, #id])
    }

    /// `comment.commentable(&db)`
    fn method(&self) -> TokenStream2 {
        let Self {
            ident,
            vis,
            name,
            target,
            ..
        } = self;
        let columns = self.column_pair();
        let doc = format!("The model the `{name}` relation points at.");
        quote! {
            #[doc = #doc]
            ///
            /// # Errors
            ///
            /// Database and decode errors, and `rustique::Error::UnknownMorphType`.
            #vis async fn #ident(
                &self,
                db: &impl ::rustique::Executor,
            ) -> ::rustique::Result<::core::option::Option<#target>> {
                ::rustique::__private::morph_to::<Self, #target>(self, db, #columns).await
            }
        }
    }
}

/// `type Key` and the `Model` constants.
fn model_constants(
    options: &Options,
    table: &LitStr,
    columns: &[Column],
    key: &Key,
) -> TokenStream2 {
    let key_type = key.ty();
    let names = columns.iter().map(|c| &c.name);
    let key_names = key.columns.iter().map(|c| &c.name);
    let incrementing = key.incrementing;
    let sequence = match key.sequence {
        Some(name) => quote!(::core::option::Option::Some(#name)),
        None => quote!(::core::option::Option::None),
    };
    let (timestamps, soft_deletes) = (options.timestamps, options.soft_deletes);
    let morph_alias = options
        .morph_alias
        .as_ref()
        .map(|alias| quote!(const MORPH_ALIAS: &'static str = #alias;));
    let timestamp_columns = [
        (format_ident!("CREATED_AT"), &options.created_at),
        (format_ident!("UPDATED_AT"), &options.updated_at),
        (format_ident!("DELETED_AT"), &options.deleted_at),
    ]
    .into_iter()
    .filter_map(|(constant, column)| {
        column
            .as_ref()
            .map(|column| quote!(const #constant: &'static str = #column;))
    });
    quote! {
        type Key = #key_type;

        const TABLE: &'static str = #table;
        const COLUMNS: &'static [&'static str] = &[#(#names),*];
        const PRIMARY_KEY: &'static [&'static str] = &[#(#key_names),*];
        const INCREMENTING: bool = #incrementing;
        const SEQUENCE: ::core::option::Option<&'static str> = #sequence;
        const TIMESTAMPS: bool = #timestamps;
        const SOFT_DELETES: bool = #soft_deletes;
        #morph_alias
        #(#timestamp_columns)*
    }
}

/// `from_row`, `to_values`, `set_value`, `key_values`, `meta` and `meta_mut`.
fn row_fns(fields: &ModelFields, key: &Key) -> TokenStream2 {
    let names: Vec<&String> = fields.columns.iter().map(|c| &c.name).collect();
    let idents: Vec<&Ident> = fields.columns.iter().map(|c| &c.ident).collect();
    let relation_idents = fields.relations.iter().map(|r| &r.ident);
    let morph_idents = fields.morphs.iter().map(|m| &m.ident);
    let count_idents = fields.counts.iter().map(|(ident, _)| ident);
    let count_names = fields.counts.iter().map(|(_, name)| name);
    let key_values = key.values_fn();
    quote! {
        fn from_row(row: &mut ::rustique::Row) -> ::rustique::Result<Self> {
            ::core::result::Result::Ok(Self {
                #(#idents: row.take_field(#names)?,)*
                #(#relation_idents: ::core::default::Default::default(),)*
                #(#morph_idents: ::core::default::Default::default(),)*
                #(#count_idents: ::rustique::Rel::from_row(row, #count_names)?,)*
                __meta: ::core::default::Default::default(),
            })
        }

        fn to_values(&self) -> ::std::vec::Vec<::rustique::Value> {
            ::std::vec![#(::rustique::Value::from(::core::clone::Clone::clone(&self.#idents))),*]
        }

        fn set_value(&mut self, column: &str, value: ::rustique::Value) -> ::rustique::Result<()> {
            match column {
                #(#names => {
                    self.#idents = ::rustique::FromValue::from_value(value).map_err(|source| {
                        ::rustique::__private::decode_error(Self::TABLE, column, source)
                    })?;
                })*
                _ => return ::core::result::Result::Err(::rustique::Error::ColumnNotFound(column.to_owned())),
            }
            ::core::result::Result::Ok(())
        }

        #key_values

        fn meta(&self) -> &::rustique::Meta {
            &self.__meta
        }

        fn meta_mut(&mut self) -> &mut ::rustique::Meta {
            &mut self.__meta
        }
    }
}

/// `relation`, `morph_columns`, `morph_targets` and `load_relation`, for
/// models with relations.
fn relation_fns(model: &Ident, fields: &ModelFields) -> TokenStream2 {
    let ModelFields {
        relations, morphs, ..
    } = fields;
    let relation_idents: Vec<&Ident> = relations.iter().map(|r| &r.ident).collect();
    let relation_names: Vec<&String> = relations.iter().map(|r| &r.name).collect();
    let relation_types = relations.iter().map(|r| &r.related);
    let relation_values = relations.iter().map(|r| r.relation(model));
    let relation_scopes = relations.iter().map(RelationField::scope_fn);
    let morph_idents = morphs.iter().map(|m| &m.ident);
    let morph_names: Vec<&String> = morphs.iter().map(|m| &m.name).collect();
    let morph_columns: Vec<TokenStream2> = morphs.iter().map(MorphToField::column_pair).collect();
    let morph_targets = morphs.iter().map(|m| &m.target);

    let relation_fn = (!relations.is_empty()).then(|| {
        quote! {
            fn relation(name: &str) -> ::core::option::Option<::rustique::__private::Relation> {
                match name {
                    #(#relation_names => ::core::option::Option::Some(#relation_values),)*
                    _ => ::core::option::Option::None,
                }
            }
        }
    });
    let morph_fns = (!morphs.is_empty()).then(|| {
        quote! {
            fn morph_columns(name: &str) -> ::core::option::Option<[&'static str; 2]> {
                match name {
                    #(#morph_names => ::core::option::Option::Some(#morph_columns),)*
                    _ => ::core::option::Option::None,
                }
            }

            fn morph_targets(name: &str) -> ::std::vec::Vec<::rustique::__private::MorphTarget> {
                match name {
                    #(#morph_names => <#morph_targets as ::rustique::MorphTo>::targets(),)*
                    _ => ::std::vec::Vec::new(),
                }
            }
        }
    });
    let load_fn = (!relations.is_empty() || !morphs.is_empty()).then(|| {
        quote! {
            fn load_relation<'a, E: ::rustique::Executor>(
                db: &'a E,
                models: &'a mut [Self],
                name: &'a str,
                nested: ::rustique::__private::Nested,
            ) -> ::rustique::BoxFuture<'a, ::rustique::Result<()>> {
                ::std::boxed::Box::pin(async move {
                    match name {
                        #(#relation_names => {
                            ::rustique::__private::load::<Self, #relation_types, _, _>(
                                db, models, name, nested, |model: &mut Self| &mut model.#relation_idents,
                                #relation_scopes,
                            ).await
                        })*
                        #(#morph_names => {
                            ::rustique::__private::load_morph(
                                db, models, nested, #morph_columns, |model: &mut Self| &mut model.#morph_idents,
                            ).await
                        })*
                        _ => ::core::result::Result::Err(::rustique::Error::UnknownRelation {
                            model: <Self as ::rustique::Model>::TABLE,
                            relation: name.to_owned(),
                        }),
                    }
                })
            }
        }
    });
    quote! {
        #relation_fn
        #morph_fns
        #load_fn
    }
}

/// `new_unique_id`, `fire_observers` and `apply_global_scopes`, for models
/// whose options need them.
fn option_fns(options: &Options) -> TokenStream2 {
    let unique_id = options.unique_id.as_ref().map(|kind| {
        let new = format_ident!("new_{kind}");
        quote! {
            fn new_unique_id() -> ::core::option::Option<::rustique::Value> {
                ::core::option::Option::Some(::rustique::__private::#new())
            }
        }
    });
    let observers = &options.observers;
    let observers_fn = (!observers.is_empty()).then(|| {
        quote! {
            async fn fire_observers(
                &mut self,
                event: ::rustique::__private::Event,
                db: &impl ::rustique::Executor,
            ) -> ::rustique::Result<()> {
                #(::rustique::__private::observe::<Self, #observers>(self, event, db).await?;)*
                ::core::result::Result::Ok(())
            }
        }
    });
    let global_scopes = &options.global_scopes;
    let global_scopes_fn = (!global_scopes.is_empty()).then(|| {
        quote! {
            fn apply_global_scopes(query: ::rustique::Query<Self>) -> ::rustique::Query<Self> {
                query #(.apply_global_scope::<#global_scopes>())*
            }
        }
    });
    quote! {
        #unique_id
        #observers_fn
        #global_scopes_fn
    }
}

/// The `{Model}Fill` struct: the `#[rustique(fillable)]` columns, each an
/// `Option`, and `Model::fill`, which sets the fields that are `Some`. The
/// primary key and the timestamp columns cannot be fillable: a request body
/// that could set the key would pick new rows' keys or re-key existing ones.
/// It derives `Deserialize` when the model does, rejecting unknown and
/// guarded keys.
fn expand_fill(
    item: &ItemStruct,
    columns: &[Column],
    managed: &[String],
    deserialize: &[Meta],
) -> syn::Result<TokenStream2> {
    let fillable: Vec<&Column> = columns.iter().filter(|c| c.fillable).collect();
    if let Some(column) = fillable
        .iter()
        .find(|c| c.primary_key || managed.contains(&c.name))
    {
        return Err(syn::Error::new_spanned(
            &column.ident,
            "the primary key and the timestamp columns cannot be `fillable`; set them on the field",
        ));
    }
    let idents: Vec<&Ident> = fillable.iter().map(|c| &c.ident).collect();
    let types: Vec<&Type> = fillable.iter().map(|c| &c.ty).collect();
    let names: Vec<&String> = fillable.iter().map(|c| &c.name).collect();
    let (serde_derive, serde_field) = if deserialize.is_empty() {
        (None, None)
    } else {
        (
            Some(quote! {
                #[cfg_attr(
                    any(#(#deserialize),*),
                    derive(::rustique::__private::serde::Deserialize),
                    serde(crate = "::rustique::__private::serde", deny_unknown_fields)
                )]
            }),
            // `present`: a key given as `null` sets a nullable column to `None`.
            Some(quote! {
                #[cfg_attr(
                    any(#(#deserialize),*),
                    serde(default, deserialize_with = "::rustique::__private::present")
                )]
            }),
        )
    };
    let ident = &item.ident;
    let vis = &item.vis;
    let fill_ident = format_ident!("{ident}Fill");
    let generics = &item.generics;
    let (impl_generics, type_generics, where_clause) = generics.split_for_impl();
    let doc = format!("The fillable fields of [`{ident}`], for [`{ident}::fill`].");
    Ok(quote! {
        #[doc = #doc]
        #[derive(Default)]
        #serde_derive
        #vis struct #fill_ident #generics #where_clause {
            #(#serde_field pub #idents: ::core::option::Option<#types>,)*
        }

        impl #impl_generics #ident #type_generics #where_clause {
            /// The columns of the fields in the `Fill` struct, to check
            /// column names that arrive at runtime.
            pub const FILLABLE: &'static [&'static str] = &[#(#names),*];

            /// Sets the fields that are `Some` in `fill`; columns not marked
            /// `#[rustique(fillable)]` have no counterpart there, so they cannot
            /// be mass-assigned.
            pub fn fill(&mut self, fill: #fill_ident #type_generics) -> &mut Self {
                #(if let ::core::option::Option::Some(value) = fill.#idents {
                    self.#idents = value;
                })*
                self
            }
        }
    })
}

/// Conditions under which the struct derives any of `derives` from serde
/// (`all()` when unconditional), so the macro can add serde
/// attributes to the fields it handles. Only derives listed below
/// `#[rustique::model]` are visible here.
fn serde_conditions(attrs: &[Attribute], derives: &[&str]) -> syn::Result<Vec<Meta>> {
    let derives_serde = |meta: &Meta| -> syn::Result<bool> {
        let Meta::List(list) = meta else {
            return Ok(false);
        };
        if !list.path.is_ident("derive") {
            return Ok(false);
        }
        let paths = list.parse_args_with(Punctuated::<Path, Token![,]>::parse_terminated)?;
        Ok(paths.iter().any(|path| {
            path.segments
                .last()
                .is_some_and(|s| derives.iter().any(|d| s.ident == d))
        }))
    };

    let mut conditions: Vec<Meta> = Vec::new();
    for attr in attrs {
        if derives_serde(&attr.meta)? {
            conditions.push(syn::parse_quote!(all()));
        } else if attr.path().is_ident("cfg_attr") {
            let args = attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)?;
            let mut args = args.into_iter();
            if let Some(condition) = args.next() {
                for meta in args {
                    if derives_serde(&meta)? {
                        conditions.push(condition.clone());
                    }
                }
            }
        }
    }
    Ok(conditions)
}
