use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Extension,
    Enum,
    Function,
    Table,
    Column,
    ForeignKey,
    Index,
    Constraint,
    Trigger,
    View,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Dep {
    pub kind: EntityKind,
    pub name: Option<String>,
}

impl Dep {
    pub fn new(kind: EntityKind, name: &str) -> Self {
        Self {
            kind,
            name: Some(name.to_string()),
        }
    }

    pub fn all_of(kind: EntityKind) -> Self {
        Self { kind, name: None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Volatility {
    #[default]
    Volatile,
    Stable,
    Immutable,
}

pub fn is_volatile(v: &Volatility) -> bool {
    *v == Volatility::Volatile
}

pub fn schema_qualified_key(name: &str, schema: Option<&str>) -> String {
    match schema {
        None | Some("public") => name.to_string(),
        Some(s) => format!("{s}.{name}"),
    }
}

/// Internal lifecycle metadata for a known non-table entity that is managed as
/// raw SQL instead of a fully modeled structure.
#[doc(hidden)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpaqueMeta {
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) raw: Option<String>,
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) trusted: bool,
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) fingerprint: Option<String>,
}

impl OpaqueMeta {
    #[doc(hidden)]
    pub fn from_raw(raw: impl Into<String>) -> Self {
        let raw = raw.into();
        let fingerprint = Some(crate::opaque::fingerprint_opaque_source(&raw));
        Self {
            raw: Some(raw),
            trusted: false,
            fingerprint,
        }
    }

    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.raw.is_none() && !self.trusted && self.fingerprint.is_none()
    }
}

/// Internal lifecycle metadata for table-level syntax Gaman preserves but does
/// not model granularly yet.
#[doc(hidden)]
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TableOptionsMeta {
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) header_raw: Vec<String>,
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) tail_raw: Vec<String>,
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) trusted: bool,
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) fingerprint: Option<String>,
}

impl TableOptionsMeta {
    #[doc(hidden)]
    pub fn from_parts(header_raw: Vec<String>, tail_raw: Vec<String>) -> Self {
        let source = header_raw
            .iter()
            .chain(tail_raw.iter())
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(" ");
        let fingerprint =
            (!source.trim().is_empty()).then(|| crate::opaque::fingerprint_opaque_source(&source));
        Self {
            header_raw,
            tail_raw,
            trusted: false,
            fingerprint,
        }
    }

    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.header_raw.is_empty()
            && self.tail_raw.is_empty()
            && !self.trusted
            && self.fingerprint.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub tables: BTreeMap<String, Table>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, ViewDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: BTreeMap<String, FunctionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, ExtensionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enums: BTreeMap<String, EnumDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub arguments: String,
    pub returns: String,
    pub language: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "crate::states::is_volatile")]
    pub volatility: Volatility,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub security_definer: bool,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl FunctionDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            arguments: String::new(),
            returns: String::new(),
            language: String::new(),
            body: String::new(),
            volatility: Volatility::Volatile,
            security_definer: false,
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    /// Build an opaque function recovered from a trusted live database catalog.
    #[doc(hidden)]
    pub fn from_trusted_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        let mut index = Self::from_raw(name, raw);
        index.mark_trusted();
        index
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.opaque.raw.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.opaque.trusted = true;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTiming {
    Before,
    After,
    InsteadOf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEvent {
    Delete,
    Insert,
    Truncate,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerScope {
    Row,
    Statement,
}

/// A trigger attached to a table.
///
/// `query` stores authored trigger statements. PostgreSQL renders query
/// triggers through a generated trigger function, while SQLite renders the query
/// directly inside `CREATE TRIGGER`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TriggerDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub timing: TriggerTiming,
    pub events: Vec<TriggerEvent>,
    pub scope: TriggerScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl TriggerDef {
    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: Some(name.into()),
            timing: TriggerTiming::After,
            events: Vec::new(),
            scope: TriggerScope::Statement,
            function_name: None,
            when: None,
            query: None,
            language: None,
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.opaque.raw.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.opaque.trusted = true;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub definition: String,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl ViewDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            definition: String::new(),
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.opaque.raw.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.opaque.trusted = true;
    }
}

/// A PostgreSQL extension (e.g. pgcrypto, postgis).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExtensionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl ExtensionDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            version: None,
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.opaque.raw.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.opaque.trusted = true;
    }
}

/// A named enum type with an ordered set of label values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub values: Vec<String>,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl EnumDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Table {
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_key: Option<PrimaryKey>,
    pub columns: Vec<Column>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub foreign_keys: Vec<ForeignKey>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub indexes: Vec<Index>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerDef>,
    #[serde(default, skip_serializing_if = "TableOptionsMeta::is_empty")]
    #[doc(hidden)]
    pub options: TableOptionsMeta,
}

impl Table {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    pub fn pk_constraint_name(&self) -> String {
        super::names::primary_key(&self.name)
    }

    pub fn pk_constraint_name_for(table_name: &str) -> String {
        super::names::primary_key(table_name)
    }

    pub fn primary_key_column_names(&self) -> Vec<&str> {
        match &self.primary_key {
            Some(pk) => pk.columns.iter().map(String::as_str).collect(),
            None => self
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| column.name.as_str())
                .collect(),
        }
    }

    pub fn primary_key_columns(&self) -> Vec<&Column> {
        self.primary_key_column_names()
            .into_iter()
            .filter_map(|name| self.columns.iter().find(|column| column.name == name))
            .collect()
    }

    pub fn is_primary_key_column(&self, name: &str) -> bool {
        self.primary_key_column_names()
            .into_iter()
            .any(|column| column == name)
    }

    #[doc(hidden)]
    pub fn has_unmanaged_options(&self) -> bool {
        !self.options.header_raw.is_empty() || !self.options.tail_raw.is_empty()
    }

    #[doc(hidden)]
    pub fn unmanaged_options_fingerprint(&self) -> Option<&str> {
        self.options.fingerprint.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_options_trusted(&mut self) {
        self.options.trusted = true;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PrimaryKey {
    #[serde(default)]
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Index {
    #[serde(default)]
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

impl Index {
    /// Attach a partial-index predicate.
    pub fn predicate(mut self, expression: impl Into<String>) -> Self {
        let expression = expression.into();
        if !expression.trim().is_empty() {
            self.predicate = Some(expression);
        }
        self
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            columns: Vec::new(),
            unique: false,
            predicate: None,
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    /// Build an opaque index recovered from a trusted live database catalog.
    #[doc(hidden)]
    pub fn from_trusted_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        let mut index = Self::from_raw(name, raw);
        index.mark_trusted();
        index
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.opaque.raw.is_some()
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.opaque.raw.as_deref()
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.opaque.trusted = true;
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    Unique {
        #[serde(default)]
        name: String,
        columns: Vec<String>,
    },
    Check {
        #[serde(default)]
        name: String,
        expression: String,
    },
    Opaque {
        name: String,
        #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
        #[doc(hidden)]
        opaque: OpaqueMeta,
    },
}

impl Constraint {
    pub fn name(&self) -> &str {
        match self {
            Constraint::Unique { name, .. } => name,
            Constraint::Check { name, .. } => name,
            Constraint::Opaque { name, .. } => name,
        }
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self::Opaque {
            name: name.into(),
            opaque: OpaqueMeta::from_raw(raw),
        }
    }

    #[doc(hidden)]
    pub fn from_trusted_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        let mut constraint = Self::from_raw(name, raw);
        constraint.mark_trusted();
        constraint
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        matches!(self, Self::Opaque { opaque, .. } if opaque.raw.is_some())
    }

    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        match self {
            Self::Opaque { opaque, .. } => opaque.raw.as_deref(),
            _ => None,
        }
    }

    #[doc(hidden)]
    pub fn opaque_meta(&self) -> Option<&OpaqueMeta> {
        match self {
            Self::Opaque { opaque, .. } => Some(opaque),
            _ => None,
        }
    }

    #[doc(hidden)]
    pub fn opaque_meta_mut(&mut self) -> Option<&mut OpaqueMeta> {
        match self {
            Self::Opaque { opaque, .. } => Some(opaque),
            _ => None,
        }
    }

    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        if let Some(opaque) = self.opaque_meta_mut() {
            opaque.trusted = true;
        }
    }
}

/// Inline foreign-key reference declared on a column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub col_type: String,
    #[serde(default)]
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<ColumnRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ForeignKey {
    pub name: String,
    pub columns: Vec<String>,
    pub to_table: String,
    pub to_columns: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_delete: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update: Option<String>,
}

impl ForeignKey {
    pub fn new(
        name: impl Into<String>,
        columns: impl IntoIterator<Item = impl Into<String>>,
        to_table: impl Into<String>,
        to_columns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            name: name.into(),
            columns: columns.into_iter().map(Into::into).collect(),
            to_table: to_table.into(),
            to_columns: to_columns.into_iter().map(Into::into).collect(),
            on_delete: None,
            on_update: None,
        }
    }

    pub fn on_delete(mut self, action: impl Into<String>) -> Self {
        let action = action.into();
        if !action.trim().is_empty() {
            self.on_delete = Some(action);
        }
        self
    }

    /// Set the foreign-key `ON UPDATE` action.
    pub fn on_update(mut self, action: impl Into<String>) -> Self {
        let action = action.into();
        if !action.trim().is_empty() {
            self.on_update = Some(action);
        }
        self
    }

    pub fn single(
        name: impl Into<String>,
        from_column: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
    ) -> Self {
        Self::new(name, [from_column], to_table, [to_column])
    }

    pub fn source_columns(&self) -> &[String] {
        &self.columns
    }

    pub fn target_columns(&self) -> &[String] {
        &self.to_columns
    }
}

pub fn canonical_foreign_key_action(action: &str) -> Option<&'static str> {
    match action
        .trim()
        .to_ascii_lowercase()
        .replace(' ', "_")
        .as_str()
    {
        "cascade" => Some("cascade"),
        "restrict" => Some("restrict"),
        "set_null" => Some("set_null"),
        "set_default" => Some("set_default"),
        "no_action" | "" => None,
        _ => None,
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ForeignKeyWire {
    #[serde(default)]
    name: String,
    #[serde(default)]
    columns: Vec<String>,
    #[serde(default)]
    from_column: Option<String>,
    to_table: String,
    #[serde(default)]
    to_columns: Vec<String>,
    #[serde(default)]
    to_column: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on_delete: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    on_update: Option<String>,
}

impl<'de> Deserialize<'de> for ForeignKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = ForeignKeyWire::deserialize(deserializer)?;
        let columns = merge_fk_columns(wire.columns, wire.from_column, "columns", "from_column")
            .map_err(serde::de::Error::custom)?;
        let to_columns =
            merge_fk_columns(wire.to_columns, wire.to_column, "to_columns", "to_column")
                .map_err(serde::de::Error::custom)?;
        Ok(Self {
            name: wire.name,
            columns,
            to_table: wire.to_table,
            to_columns,
            on_delete: wire.on_delete.filter(|action| !action.trim().is_empty()),
            on_update: wire.on_update.filter(|action| !action.trim().is_empty()),
        })
    }
}

fn merge_fk_columns(
    columns: Vec<String>,
    legacy_column: Option<String>,
    canonical_name: &str,
    legacy_name: &str,
) -> Result<Vec<String>, String> {
    match (columns.is_empty(), legacy_column) {
        (true, Some(column)) => Ok(vec![column]),
        (true, None) => Ok(columns),
        (false, None) => Ok(columns),
        (false, Some(column)) => {
            if columns.len() == 1 && columns[0] == column {
                Ok(columns)
            } else {
                Err(format!(
                    "foreign key specifies both '{canonical_name}' and conflicting legacy '{legacy_name}'"
                ))
            }
        }
    }
}
