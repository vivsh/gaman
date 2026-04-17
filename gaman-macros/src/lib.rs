use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{format_ident, quote};
use std::path::Path;
use syn::{LitStr, Type, parse_macro_input};
use darling::{FromDeriveInput, FromField, ast};

/// Embeds all `.yaml` migration files from `path` (relative to your crate root) at compile time.
///
/// Returns `&'static [(&'static str, &'static str)]` where each tuple is `(id, yaml_content)`.
/// Files are sorted lexicographically, so use a numeric prefix (`0001_`, `0002_`) for stable ordering.
///
/// # Example
///
/// ```no_run
/// use gaman::embed::Runner;
/// use gaman_macros::include_migrations;
///
/// Runner::new("postgres://localhost/mydb", include_migrations!("migrations"))
///     .run()
///     .expect("migrations failed");
/// ```
#[proc_macro]
pub fn include_migrations(input: TokenStream) -> TokenStream {
    let path_lit = parse_macro_input!(input as LitStr);
    let rel_path = path_lit.value();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
        .expect("CARGO_MANIFEST_DIR not set");
    let dir = Path::new(&manifest_dir).join(&rel_path);

    let mut entries: Vec<_> = if dir.exists() {
        std::fs::read_dir(&dir)
            .unwrap_or_else(|e| panic!("failed to read migrations dir '{}': {e}", dir.display()))
            .filter_map(|e| e.ok())
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("yaml"))
            .map(|e| e.path())
            .collect()
    } else {
        vec![]
    };

    entries.sort();

    let pairs: Vec<_> = entries
        .iter()
        .map(|path| {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let abs = path.to_str().unwrap_or_default().to_string();
            let id_lit = LitStr::new(&id, Span::call_site());
            let path_lit = LitStr::new(&abs, Span::call_site());
            quote! { (#id_lit, ::core::include_str!(#path_lit)) }
        })
        .collect();

    quote! {
        &[#(#pairs),*] as &[(&'static str, &'static str)]
    }
    .into()
}

// ── IntoTable derive ──────────────────────────────────────────────────────────

#[derive(FromDeriveInput)]
#[darling(attributes(table), supports(struct_named))]
struct IntoTableInput {
    ident: syn::Ident,
    data: ast::Data<darling::util::Ignored, IntoTableField>,
    #[darling(default)]
    name: Option<String>,
    #[darling(default)]
    schema: Option<String>,
}

#[derive(FromField)]
#[darling(attributes(column))]
struct IntoTableField {
    ident: Option<syn::Ident>,
    ty: Type,
    #[darling(default)]
    skip: bool,
    #[darling(default)]
    name: Option<String>,
    /// Explicit SQL type string — highest priority, bypasses `ColumnType` lookup.
    /// Required for third-party types (e.g. `#[column(type = "uuid")]`).
    /// When used without `nullable`, nullability defaults to `false`.
    #[darling(default, rename = "type")]
    sql_type: Option<String>,
    /// Override nullability. When absent: inferred via `ColumnType` (handles `Option<T>`).
    /// Must be set explicitly when `type = "..."` is used on an `Option<ThirdParty>` field.
    #[darling(default)]
    nullable: Option<bool>,
    #[darling(default)]
    primary_key: bool,
    #[darling(default)]
    default: Option<String>,
    /// Inline FK: `"table.column"`, e.g. `#[column(references = "users.id")]`.
    #[darling(default)]
    references: Option<String>,
    #[darling(default)]
    references_name: Option<String>,
    #[darling(default)]
    check: Option<String>,
}

fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    out
}

/// Derive `IntoTable` for a named struct, mapping each field to a Postgres column.
///
/// Type resolution precedence per column:
/// 1. `#[column(type = "...")]` — explicit SQL type string, bypasses all trait lookup
/// 2. `<FieldType as gaman::column_type::ColumnType>::column_desc(dialect)` — default,
///    handles `Option<T>` → nullable via the blanket impl
///
/// # Container attributes (`#[table(...)]`)
/// - `name = "..."` — table name override (default: snake_case struct name)
/// - `schema = "..."` — Postgres schema (omit for public)
///
/// # Field attributes (`#[column(...)]`)
/// - `skip` — exclude this field
/// - `name = "..."` — column name override
/// - `type = "..."` — explicit SQL type; required for third-party types (uuid, chrono, etc.)
/// - `nullable` / `nullable = false` — override inferred nullability; required when using
///   `type = "..."` on an `Option<ThirdPartyType>` field
/// - `primary_key` — mark as primary key
/// - `default = "expr"` — SQL default expression
/// - `references = "table.column"` — inline foreign key
/// - `references_name = "fk_name"` — explicit FK constraint name
/// - `check = "expr"` — inline check constraint
#[proc_macro_derive(IntoTable, attributes(table, column))]
pub fn derive_into_table(input: TokenStream) -> TokenStream {
    let args = match IntoTableInput::from_derive_input(&parse_macro_input!(input)) {
        Ok(v) => v,
        Err(e) => return e.write_errors().into(),
    };

    let struct_ident = &args.ident;
    let table_name = args
        .name
        .unwrap_or_else(|| to_snake_case(&args.ident.to_string()));

    let fields = args.data.take_struct().expect("only named structs supported");

    let mut pre_stmts: Vec<proc_macro2::TokenStream> = vec![];
    let mut col_stmts: Vec<proc_macro2::TokenStream> = vec![];

    for (i, field) in fields.iter().enumerate() {
        if field.skip {
            continue;
        }

        let col_name = field
            .name
            .clone()
            .unwrap_or_else(|| field.ident.as_ref().map(|id| id.to_string()).unwrap_or_default());

        let field_ty = &field.ty;
        let desc_var = format_ident!("__gaman_col_{}", i);

        let type_expr: proc_macro2::TokenStream;
        let mut closure_stmts: Vec<proc_macro2::TokenStream> = vec![];

        if let Some(ref sql_type) = field.sql_type {
            type_expr = quote! { #sql_type };
            let nullable = field.nullable.unwrap_or(false);
            if nullable {
                closure_stmts.push(quote! { let c = c.nullable(); });
            } else {
                closure_stmts.push(quote! { let c = c.not_null(); });
            }
        } else {
            pre_stmts.push(quote! {
                let #desc_var = <#field_ty as ::gaman::schema::ColumnType>::column_desc(dialect);
            });
            type_expr = quote! { #desc_var.sql_type };
            if let Some(nullable_override) = field.nullable {
                if nullable_override {
                    closure_stmts.push(quote! { let c = c.nullable(); });
                } else {
                    closure_stmts.push(quote! { let c = c.not_null(); });
                }
            } else {
                closure_stmts.push(quote! {
                    let c = if #desc_var.nullable { c.nullable() } else { c.not_null() };
                });
            }
        }

        if field.primary_key {
            closure_stmts.push(quote! { let c = c.primary_key(); });
        }

        if let Some(ref expr) = field.default {
            closure_stmts.push(quote! { let c = c.default(#expr); });
        }

        if let Some(ref refs) = field.references {
            let parts: Vec<&str> = refs.splitn(2, '.').collect();
            if parts.len() == 2 {
                let ref_table = parts[0];
                let ref_col = parts[1];
                if let Some(ref fk_name) = field.references_name {
                    closure_stmts.push(quote! {
                        let c = c.references_named(#fk_name, #ref_table, #ref_col);
                    });
                } else {
                    closure_stmts.push(quote! {
                        let c = c.references(#ref_table, #ref_col);
                    });
                }
            }
        }

        if let Some(ref expr) = field.check {
            closure_stmts.push(quote! { let c = c.check(#expr); });
        }

        col_stmts.push(quote! {
            .column(#col_name, #type_expr, |c| {
                #(#closure_stmts)*
                c
            })
        });
    }

    let schema_stmt = if let Some(ref schema) = args.schema {
        quote! { .schema(#schema) }
    } else {
        quote! {}
    };

    quote! {
        impl ::gaman::schema::IntoTable for #struct_ident {
            fn into_table(dialect: &::gaman::core::Dialect) -> ::gaman::schema::Table {
                #(#pre_stmts)*
                ::gaman::schema::TableBuilder::new(#table_name)
                    #schema_stmt
                    #(#col_stmts)*
                    .build()
            }
        }
    }
    .into()
}

