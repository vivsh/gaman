use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use std::path::{Path, PathBuf};
use syn::{LitStr, parse_macro_input};

pub(crate) fn embedded_migrations(input: TokenStream) -> TokenStream {
    let path_lit = parse_macro_input!(input as LitStr);
    let rel_path = path_lit.value();

    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR not set");
    let dir = Path::new(&manifest_dir).join(&rel_path);
    let embedded_dir = absolute_dir(&dir);

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

    let dir_lit = LitStr::new(&embedded_dir.to_string_lossy(), Span::call_site());

    quote! {
        gaman::EmbeddedMigrations {
            files: &[#(#pairs),*],
            dir: #dir_lit,
            children: &[],
        }
    }
    .into()
}

fn absolute_dir(dir: &Path) -> PathBuf {
    std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf())
}
