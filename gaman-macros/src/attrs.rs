use darling::FromMeta;
use syn::LitStr;

#[derive(Clone, FromMeta)]
pub(crate) struct PrimaryKeyArgs {
    pub(crate) name: String,
    pub(crate) columns: Vec<LitStr>,
}

#[derive(Clone, FromMeta)]
pub(crate) struct ForeignKeyArgs {
    pub(crate) name: String,
    pub(crate) columns: Vec<LitStr>,
    pub(crate) references: ForeignKeyReferenceArgs,
}

#[derive(Clone, FromMeta)]
pub(crate) struct ForeignKeyReferenceArgs {
    pub(crate) table: String,
    pub(crate) columns: Vec<LitStr>,
}

pub(crate) fn to_snake_case(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 4);
    for (i, ch) in s.chars().enumerate() {
        if ch.is_uppercase() && i != 0 {
            out.push('_');
        }
        out.push(ch.to_lowercase().next().unwrap_or(ch));
    }
    out
}
