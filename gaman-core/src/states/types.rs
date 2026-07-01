use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
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
}

impl FunctionDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
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
/// `body` and `language` are inline sugar: `normalize()` converts them into a
/// synthetic `FunctionDef` and sets `function_name`, then clears both fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub definition: String,
}

impl ViewDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
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
}

impl ExtensionDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }
}

/// A named enum type with an ordered set of label values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EnumDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub values: Vec<String>,
}

impl EnumDef {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
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
}

impl Table {
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    pub fn pk_constraint_name(&self) -> String {
        format!("{}_pkey", self.name)
    }

    pub fn pk_constraint_name_for(table_name: &str) -> String {
        format!("{}_pkey", table_name)
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
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrimaryKey {
    pub name: String,
    pub columns: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    #[serde(default)]
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    Unique { name: String, columns: Vec<String> },
    Check { name: String, expression: String },
}

impl Constraint {
    pub fn name(&self) -> &str {
        match self {
            Constraint::Unique { name, .. } => name,
            Constraint::Check { name, .. } => name,
        }
    }
}

/// Inline foreign-key reference declared on a column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
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
        }
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

#[derive(Deserialize)]
struct ForeignKeyWire {
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
