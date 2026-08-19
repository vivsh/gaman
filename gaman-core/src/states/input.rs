use std::collections::BTreeMap;

use serde::Deserialize;

use super::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, OpaqueMeta,
    PrimaryKey, Schema, Table, TableOptionsMeta, TriggerDef, ViewDef, Volatility,
};

/// Authored YAML/JSON schema shape without internal lifecycle metadata.
///
/// SQL parsing, inspection, and migration replay use the runtime [`Schema`]
/// type directly because they need opaque/raw metadata. Authored structured
/// input uses this type first so internal fields cannot be injected through
/// normal YAML or JSON schema files.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InputSchema {
    /// Authored table definitions keyed by table name.
    #[serde(default)]
    pub tables: BTreeMap<String, TableInput>,
    /// Managed rows keyed by their target table identity.
    #[serde(default)]
    pub managed_rows: BTreeMap<String, crate::managed_rows::ManagedRows>,
    /// Authored view definitions keyed by view name.
    #[serde(default)]
    pub views: BTreeMap<String, ViewInput>,
    /// Authored function definitions keyed by function name.
    #[serde(default)]
    pub functions: BTreeMap<String, FunctionInput>,
    /// Authored extension definitions keyed by extension name.
    #[serde(default)]
    pub extensions: BTreeMap<String, ExtensionInput>,
    /// Authored enum definitions keyed by enum name.
    #[serde(default)]
    pub enums: BTreeMap<String, EnumInput>,
}

impl InputSchema {
    /// Convert authored structured schema into runtime schema with empty
    /// lifecycle metadata.
    pub fn into_schema(self) -> Schema {
        Schema {
            tables: self
                .tables
                .into_iter()
                .map(|(key, table)| (key, table.into_table()))
                .collect(),
            managed_rows: self.managed_rows,
            views: self
                .views
                .into_iter()
                .map(|(key, view)| (key, view.into_view()))
                .collect(),
            functions: self
                .functions
                .into_iter()
                .map(|(key, function)| (key, function.into_function()))
                .collect(),
            extensions: self
                .extensions
                .into_iter()
                .map(|(key, extension)| (key, extension.into_extension()))
                .collect(),
            enums: self
                .enums
                .into_iter()
                .map(|(key, enum_def)| (key, enum_def.into_enum()))
                .collect(),
        }
    }
}

/// Authored table definition without unmanaged table-option metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TableInput {
    /// Table name. Empty names are filled from the map key during normalization.
    #[serde(default)]
    pub name: String,
    /// Optional database schema name.
    #[serde(default)]
    pub schema: Option<String>,
    /// Optional primary-key definition.
    #[serde(default)]
    pub primary_key: Option<PrimaryKey>,
    /// Authored columns.
    pub columns: Vec<Column>,
    /// Authored table-level foreign keys.
    #[serde(default)]
    pub foreign_keys: Vec<ForeignKey>,
    /// Authored modeled indexes.
    #[serde(default)]
    pub indexes: Vec<IndexInput>,
    /// Authored modeled constraints.
    #[serde(default)]
    pub constraints: Vec<ConstraintInput>,
    /// Authored modeled triggers.
    #[serde(default)]
    pub triggers: Vec<TriggerInput>,
}

impl TableInput {
    fn into_table(self) -> Table {
        Table {
            name: self.name,
            schema: self.schema,
            primary_key: self.primary_key,
            columns: self.columns,
            foreign_keys: self.foreign_keys,
            indexes: self
                .indexes
                .into_iter()
                .map(IndexInput::into_index)
                .collect(),
            constraints: self
                .constraints
                .into_iter()
                .map(ConstraintInput::into_constraint)
                .collect(),
            triggers: self
                .triggers
                .into_iter()
                .map(TriggerInput::into_trigger)
                .collect(),
            options: TableOptionsMeta::default(),
        }
    }
}

/// Authored index definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IndexInput {
    /// Index name.
    #[serde(default)]
    pub name: String,
    /// Ordered indexed columns.
    pub columns: Vec<String>,
    /// Whether the index is unique.
    #[serde(default)]
    pub unique: bool,
    /// Optional partial-index predicate.
    #[serde(default)]
    pub predicate: Option<String>,
}

impl IndexInput {
    fn into_index(self) -> Index {
        Index {
            name: self.name,
            columns: self.columns,
            unique: self.unique,
            predicate: self.predicate,
            opaque: OpaqueMeta::default(),
        }
    }
}

/// Authored constraint definition limited to modeled constraint variants.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ConstraintInput {
    /// Authored unique constraint.
    Unique {
        /// Constraint name.
        #[serde(default)]
        name: String,
        /// Ordered constrained columns.
        columns: Vec<String>,
    },
    /// Authored check constraint.
    Check {
        /// Constraint name.
        #[serde(default)]
        name: String,
        /// Check expression.
        expression: String,
    },
}

impl ConstraintInput {
    fn into_constraint(self) -> Constraint {
        match self {
            Self::Unique { name, columns } => Constraint::Unique { name, columns },
            Self::Check { name, expression } => Constraint::Check { name, expression },
        }
    }
}

/// Authored trigger definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TriggerInput {
    /// Trigger name.
    #[serde(default)]
    pub name: Option<String>,
    /// Trigger timing.
    pub timing: super::TriggerTiming,
    /// Trigger events.
    pub events: Vec<super::TriggerEvent>,
    /// Trigger scope.
    pub scope: super::TriggerScope,
    /// Optional function target.
    #[serde(default)]
    pub function_name: Option<String>,
    /// Optional `WHEN` expression.
    #[serde(default)]
    pub when: Option<String>,
    /// Optional body query.
    #[serde(default)]
    pub query: Option<String>,
    /// Optional trigger language metadata.
    #[serde(default)]
    pub language: Option<String>,
}

impl TriggerInput {
    fn into_trigger(self) -> TriggerDef {
        TriggerDef {
            name: self.name,
            timing: self.timing,
            events: self.events,
            scope: self.scope,
            function_name: self.function_name,
            when: self.when,
            query: self.query,
            language: self.language,
            opaque: OpaqueMeta::default(),
        }
    }
}

/// Authored function definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct FunctionInput {
    /// Function name.
    pub name: String,
    /// Optional database schema name.
    #[serde(default)]
    pub schema: Option<String>,
    /// Legacy function argument signature.
    #[serde(default)]
    pub arguments: String,
    /// Typed function parameters.
    #[serde(default)]
    pub parameters: Vec<super::FunctionParameter>,
    /// Function return type.
    pub returns: String,
    /// Function language.
    pub language: String,
    /// Function body.
    pub body: String,
    /// Explicit root dependencies required before this function.
    #[serde(default)]
    pub depends_on: Vec<crate::EntityDependency>,
    /// Function volatility.
    #[serde(default)]
    pub volatility: Volatility,
    /// Whether the function is `SECURITY DEFINER`.
    #[serde(default)]
    pub security_definer: bool,
}

impl FunctionInput {
    fn into_function(self) -> FunctionDef {
        let parameters = if self.parameters.is_empty() {
            super::legacy_function_parameters(&self.arguments).unwrap_or_default()
        } else {
            self.parameters
        };
        let arguments = parameters.is_empty().then_some(self.arguments).unwrap_or_default();
        FunctionDef {
            name: self.name,
            schema: self.schema,
            parameters,
            arguments,
            returns: self.returns,
            language: self.language,
            body: self.body,
            depends_on: self.depends_on,
            volatility: self.volatility,
            security_definer: self.security_definer,
            opaque: OpaqueMeta::default(),
        }
    }
}


/// Authored view definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViewInput {
    /// View name.
    pub name: String,
    /// Optional database schema name.
    #[serde(default)]
    pub schema: Option<String>,
    /// Authored view definition.
    pub definition: String,
}

impl ViewInput {
    fn into_view(self) -> ViewDef {
        ViewDef {
            name: self.name,
            schema: self.schema,
            definition: self.definition,
            opaque: OpaqueMeta::default(),
        }
    }
}

/// Authored extension definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ExtensionInput {
    /// Extension name.
    pub name: String,
    /// Optional database schema name.
    #[serde(default)]
    pub schema: Option<String>,
    /// Optional pinned extension version.
    #[serde(default)]
    pub version: Option<String>,
}

impl ExtensionInput {
    fn into_extension(self) -> ExtensionDef {
        ExtensionDef {
            name: self.name,
            schema: self.schema,
            version: self.version,
            opaque: OpaqueMeta::default(),
        }
    }
}

/// Authored enum definition without opaque raw SQL metadata.
#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EnumInput {
    /// Enum name.
    pub name: String,
    /// Optional database schema name.
    #[serde(default)]
    pub schema: Option<String>,
    /// Ordered enum labels.
    pub values: Vec<String>,
}

impl EnumInput {
    fn into_enum(self) -> EnumDef {
        EnumDef {
            name: self.name,
            schema: self.schema,
            values: self.values,
            opaque: OpaqueMeta::default(),
        }
    }
}
