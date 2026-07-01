mod attrs;
mod codegen;
mod column;
mod embedded;
mod table;

use proc_macro::TokenStream;

/// Embed `.yaml` migrations from a directory at compile time.
/// Returns an `EmbeddedMigrations` value carrying both the compiled-in files and the absolute
/// source directory path, so the runtime can validate against `config.migrations_dir`.
#[proc_macro]
pub fn embedded_migrations(input: TokenStream) -> TokenStream {
    embedded::embedded_migrations(input)
}

/// Derive `IntoTable` for a named struct.
/// Supports `#[table(name = "...", schema = "...")]` plus column attributes for
/// naming, explicit SQL types, nullability, defaults, keys, indexes, and checks.
#[proc_macro_derive(IntoTable, attributes(table, column))]
pub fn derive_into_table(input: TokenStream) -> TokenStream {
    codegen::derive_into_table(input)
}
