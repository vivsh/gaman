use darling::FromField;
use syn::Type;

#[derive(FromField)]
#[darling(attributes(column))]
pub(crate) struct IntoTableField {
    pub(crate) ident: Option<syn::Ident>,
    pub(crate) ty: Type,
    #[darling(default)]
    pub(crate) skip: bool,
    #[darling(default)]
    pub(crate) name: Option<String>,
    /// Explicit SQL type string, mainly for third-party types.
    /// When this is set, nullability defaults to `false` unless overridden.
    #[darling(default, rename = "type")]
    pub(crate) sql_type: Option<String>,
    /// Override nullability instead of inferring it from `ColumnType`.
    #[darling(default)]
    pub(crate) nullable: Option<bool>,
    #[darling(default)]
    pub(crate) primary_key: bool,
    #[darling(default)]
    pub(crate) index: bool,
    #[darling(default)]
    pub(crate) index_name: Option<String>,
    #[darling(default)]
    pub(crate) unique: bool,
    #[darling(default)]
    pub(crate) unique_name: Option<String>,
    #[darling(default)]
    pub(crate) default: Option<String>,
    /// Inline FK as `table.column`.
    #[darling(default)]
    pub(crate) references: Option<String>,
    #[darling(default)]
    pub(crate) references_name: Option<String>,
    #[darling(default)]
    pub(crate) check: Option<String>,
}
