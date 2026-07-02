use darling::FromDeriveInput;
use proc_macro::TokenStream;
use quote::quote;
use syn::{LitStr, parse_macro_input};

use crate::attrs::to_snake_case;
use crate::table::IntoTableInput;

pub(crate) fn derive_into_table(input: TokenStream) -> TokenStream {
    let args = match IntoTableInput::from_derive_input(&parse_macro_input!(input)) {
        Ok(v) => v,
        Err(e) => return e.write_errors().into(),
    };

    let struct_ident = &args.ident;
    let table_name = args
        .name
        .clone()
        .unwrap_or_else(|| to_snake_case(&args.ident.to_string()));

    let fields = args
        .data
        .as_ref()
        .take_struct()
        .expect("only named structs supported");

    let mut col_stmts: Vec<proc_macro2::TokenStream> = vec![];
    let mut table_stmts: Vec<proc_macro2::TokenStream> = vec![];
    let mut column_names: Vec<String> = vec![];
    let mut field_primary_key_columns: Vec<String> = vec![];

    for field in fields.iter() {
        if field.skip {
            continue;
        }

        let col_name = field.name.clone().unwrap_or_else(|| {
            field
                .ident
                .as_ref()
                .map(|id| id.to_string())
                .unwrap_or_default()
        });
        column_names.push(col_name.clone());

        let field_ty = &field.ty;
        let mut closure_stmts: Vec<proc_macro2::TokenStream> = vec![];
        let sql_type = match (&field.sql_type, &field.raw_sql_type) {
            (Some(_), Some(_)) => {
                return syn::Error::new_spanned(
                    &args.ident,
                    "column type specified twice; use #[column(r#type = \"...\")]",
                )
                .to_compile_error()
                .into();
            }
            (Some(sql_type), None) | (None, Some(sql_type)) => Some(sql_type),
            (None, None) => None,
        };

        if sql_type.is_some() {
            let nullable = field.nullable.unwrap_or(false);
            if nullable {
                closure_stmts.push(quote! { let c = c.nullable(); });
            } else {
                closure_stmts.push(quote! { let c = c.not_null(); });
            }
        } else {
            if let Some(nullable_override) = field.nullable {
                if nullable_override {
                    closure_stmts.push(quote! { let c = c.nullable(); });
                } else {
                    closure_stmts.push(quote! { let c = c.not_null(); });
                }
            }
        }

        if field.primary_key {
            field_primary_key_columns.push(col_name.clone());
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

        if let Some(ref index_name) = field.index_name {
            table_stmts.push(quote! { .index(#index_name, &[#col_name]) });
        } else if field.index {
            table_stmts.push(quote! { .index_columns(&[#col_name]) });
        }

        if let Some(ref unique_name) = field.unique_name {
            table_stmts.push(quote! { .unique(#unique_name, &[#col_name]) });
        } else if field.unique {
            table_stmts.push(quote! { .unique_columns(&[#col_name]) });
        }

        if let Some(sql_type) = sql_type {
            col_stmts.push(quote! {
                .column(#col_name, #sql_type, |c| {
                    #(#closure_stmts)*
                    c
                })
            });
        } else {
            col_stmts.push(quote! {
                .column_from_type::<#field_ty>(dialect, #col_name, |c| {
                    #(#closure_stmts)*
                    c
                })
            });
        }
    }

    if let Some(primary_key) = args.primary_key.clone() {
        let primary_key_columns: Vec<String> =
            primary_key.columns.iter().map(LitStr::value).collect();
        let mut explicit_columns = primary_key_columns.clone();
        let mut flagged_columns = field_primary_key_columns.clone();
        explicit_columns.sort();
        flagged_columns.sort();
        if !flagged_columns.is_empty() && explicit_columns != flagged_columns {
            return syn::Error::new_spanned(
                &args.ident,
                "table primary_key columns conflict with #[column(primary_key)] fields",
            )
            .to_compile_error()
            .into();
        }
        for column in &primary_key_columns {
            if !column_names.iter().any(|name| name == column) {
                return syn::Error::new_spanned(
                    &args.ident,
                    format!("table primary_key references unknown column '{column}'"),
                )
                .to_compile_error()
                .into();
            }
        }
        let name = primary_key.name;
        let columns = primary_key.columns;
        if let Some(name) = name {
            table_stmts.push(quote! { .primary_key(#name, &[#(#columns),*]) });
        } else {
            table_stmts.push(quote! { .primary_key_columns(&[#(#columns),*]) });
        }
    }

    for foreign_key in args.foreign_keys.clone() {
        let source_columns: Vec<String> = foreign_key.columns.iter().map(LitStr::value).collect();
        let target_columns: Vec<String> = foreign_key
            .references
            .columns
            .iter()
            .map(LitStr::value)
            .collect();
        if source_columns.is_empty() {
            let label = foreign_key.name.as_deref().unwrap_or("<unnamed>");
            return syn::Error::new_spanned(
                &args.ident,
                format!("foreign key '{label}' has no source columns"),
            )
            .to_compile_error()
            .into();
        }
        if target_columns.is_empty() {
            let label = foreign_key.name.as_deref().unwrap_or("<unnamed>");
            return syn::Error::new_spanned(
                &args.ident,
                format!("foreign key '{label}' has no target columns"),
            )
            .to_compile_error()
            .into();
        }
        if source_columns.len() != target_columns.len() {
            let label = foreign_key.name.as_deref().unwrap_or("<unnamed>");
            return syn::Error::new_spanned(
                &args.ident,
                format!(
                    "foreign key '{}' has {} source columns but {} target columns",
                    label,
                    source_columns.len(),
                    target_columns.len(),
                ),
            )
            .to_compile_error()
            .into();
        }
        for column in &source_columns {
            if !column_names.iter().any(|name| name == column) {
                let label = foreign_key.name.as_deref().unwrap_or("<unnamed>");
                return syn::Error::new_spanned(
                    &args.ident,
                    format!(
                        "foreign key '{}' references unknown source column '{column}'",
                        label
                    ),
                )
                .to_compile_error()
                .into();
            }
        }
        let name = foreign_key.name;
        let target_table = foreign_key.references.table;
        let columns = foreign_key.columns;
        let target_columns = foreign_key.references.columns;
        if let Some(name) = name {
            table_stmts.push(
                quote! { .foreign_key_named_columns(#name, &[#(#columns),*], #target_table, &[#(#target_columns),*]) },
            );
        } else {
            table_stmts.push(
                quote! { .foreign_key_columns(&[#(#columns),*], #target_table, &[#(#target_columns),*]) },
            );
        }
    }

    let schema_stmt = if let Some(ref schema) = args.schema {
        quote! { .schema(#schema) }
    } else {
        quote! {}
    };

    quote! {
        impl ::gaman::schema::IntoTable for #struct_ident {
            fn into_table(dialect: &::gaman::core::Dialect) -> ::gaman::schema::Table {
                ::gaman::schema::TableBuilder::new(#table_name)
                    #schema_stmt
                    #(#col_stmts)*
                    #(#table_stmts)*
                    .build()
            }
        }
    }
    .into()
}
