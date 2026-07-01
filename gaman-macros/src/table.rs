use darling::{FromDeriveInput, ast};

use crate::attrs::{ForeignKeyArgs, PrimaryKeyArgs};
use crate::column::IntoTableField;

#[derive(FromDeriveInput)]
#[darling(attributes(table), supports(struct_named))]
pub(crate) struct IntoTableInput {
    pub(crate) ident: syn::Ident,
    pub(crate) data: ast::Data<darling::util::Ignored, IntoTableField>,
    #[darling(default)]
    pub(crate) name: Option<String>,
    #[darling(default)]
    pub(crate) schema: Option<String>,
    #[darling(default)]
    pub(crate) primary_key: Option<PrimaryKeyArgs>,
    #[darling(default, multiple, rename = "foreign_key")]
    pub(crate) foreign_keys: Vec<ForeignKeyArgs>,
}
