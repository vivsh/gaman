use std::collections::BTreeMap;

use serde::{Deserialize, Deserializer, Serialize};
use sqlparser::ast::Statement;
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntityKind {
    Extension,
    Sequence,
    Enum,
    Function,
    Table,
    Row,
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
    #[doc(hidden)]
    #[serde(default)]
    pub(crate) identity_only: bool,
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
            identity_only: false,
        }
    }

    /// Creates trusted opaque identity when a catalog cannot return executable source.
    #[doc(hidden)]
    pub fn trusted_identity() -> Self {
        Self {
            raw: None,
            trusted: true,
            fingerprint: None,
            identity_only: true,
        }
    }

    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.raw.is_some() || self.identity_only
    }

    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.raw.is_none() && !self.trusted && self.fingerprint.is_none() && !self.identity_only
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
    /// Modeled PostgreSQL partition role retained in migration state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) postgres_partition: Option<PostgresPartitionMeta>,
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
            postgres_partition: None,
        }
    }

    #[doc(hidden)]
    pub fn is_empty(&self) -> bool {
        self.header_raw.is_empty()
            && self.tail_raw.is_empty()
            && !self.trusted
            && self.fingerprint.is_none()
            && self.postgres_partition.is_none()
    }
}

/// One PostgreSQL range-partition child definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostgresRangePartition {
    pub(crate) name: String,
    pub(crate) start: String,
    pub(crate) end: String,
}

impl PostgresRangePartition {
    /// Creates a named partition with an inclusive start and exclusive end value.
    pub fn new(name: impl Into<String>, start: impl Into<String>, end: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            start: start.into(),
            end: end.into(),
        }
    }

    /// Returns the stable child-table name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the inclusive lower bound rendered as a PostgreSQL literal.
    pub fn start(&self) -> &str {
        &self.start
    }

    /// Returns the exclusive upper bound rendered as a PostgreSQL literal.
    pub fn end(&self) -> &str {
        &self.end
    }
}

/// PostgreSQL range-partition metadata registered against a modeled parent table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PostgresRangePartitioning {
    pub(crate) column: String,
    #[serde(default)]
    pub(crate) partitions: Vec<PostgresRangePartition>,
}

impl PostgresRangePartitioning {
    /// Starts a range-partition definition for one modeled parent column.
    pub fn new(column: impl Into<String>) -> Self {
        Self {
            column: column.into(),
            partitions: Vec::new(),
        }
    }

    /// Registers one child partition and returns the updated definition.
    pub fn partition(
        mut self,
        name: impl Into<String>,
        start: impl Into<String>,
        end: impl Into<String>,
    ) -> Self {
        self.partitions
            .push(PostgresRangePartition::new(name, start, end));
        self
    }

    /// Returns the modeled range-key column.
    pub fn column(&self) -> &str {
        &self.column
    }

    /// Returns child partitions in registration order.
    pub fn partitions(&self) -> &[PostgresRangePartition] {
        &self.partitions
    }
}

/// Internal parent/child role used by ordinary table operations and replay.
#[doc(hidden)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PostgresPartitionMeta {
    /// A range-partitioned parent table.
    Parent { column: String },
    /// A child table attached to a range-partitioned parent.
    Child {
        parent: String,
        start: String,
        end: String,
    },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub tables: BTreeMap<String, Table>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub managed_rows: BTreeMap<String, crate::managed_rows::ManagedRows>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, ViewDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: BTreeMap<String, FunctionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, ExtensionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub sequences: BTreeMap<String, SequenceDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enums: BTreeMap<String, EnumDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(try_from = "FunctionDefInput")]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// Typed parameters used by newly authored functions and migrations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parameters: Vec<FunctionParameter>,
    /// Legacy raw signature accepted for backwards-compatible input.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arguments: String,
    pub returns: String,
    pub language: String,
    pub body: String,
    /// Explicit root dependencies installed before this function.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub depends_on: Vec<crate::EntityDependency>,
    #[serde(default, skip_serializing_if = "crate::states::is_volatile")]
    pub volatility: Volatility,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub security_definer: bool,
    #[serde(default, skip_serializing_if = "OpaqueMeta::is_empty")]
    #[doc(hidden)]
    pub opaque: OpaqueMeta,
}

/// Deserialization-only compatibility shape for function payloads.
#[derive(Deserialize)]
struct FunctionDefInput {
    name: String,
    #[serde(default)]
    schema: Option<String>,
    #[serde(default)]
    parameters: Vec<FunctionParameter>,
    #[serde(default)]
    arguments: String,
    returns: String,
    language: String,
    body: String,
    #[serde(default)]
    depends_on: Vec<crate::EntityDependency>,
    #[serde(default)]
    volatility: Volatility,
    #[serde(default)]
    security_definer: bool,
    #[serde(default)]
    opaque: OpaqueMeta,
}

impl TryFrom<FunctionDefInput> for FunctionDef {
    type Error = String;

    fn try_from(value: FunctionDefInput) -> Result<Self, Self::Error> {
        if !value.arguments.trim().is_empty() && !value.parameters.is_empty() {
            return Err(
                "function cannot specify both legacy arguments and typed parameters".to_string(),
            );
        }
        let parameters = if value.parameters.is_empty() {
            legacy_function_parameters(&value.arguments).unwrap_or_default()
        } else {
            value.parameters
        };
        let arguments = parameters
            .is_empty()
            .then_some(value.arguments)
            .unwrap_or_default();
        Ok(Self {
            name: value.name,
            schema: value.schema,
            parameters,
            arguments,
            returns: value.returns,
            language: value.language,
            body: value.body,
            depends_on: value.depends_on,
            volatility: value.volatility,
            security_definer: value.security_definer,
            opaque: value.opaque,
        })
    }
}

/// Converts compatible legacy PostgreSQL function arguments to typed parameters.
///
/// Unrecognized source remains raw so older migration payloads stay readable without a
/// lossy guess. PostgreSQL is the only dialect that currently models stored functions.
pub(crate) fn legacy_function_parameters(arguments: &str) -> Option<Vec<FunctionParameter>> {
    if arguments.trim().is_empty() {
        return Some(Vec::new());
    }
    let source = format!(
        "CREATE FUNCTION gaman_legacy({arguments}) RETURNS integer LANGUAGE sql AS $$ SELECT 1 $$"
    );
    let statement = Parser::parse_sql(&PostgreSqlDialect {}, &source)
        .ok()?
        .pop()?;
    let Statement::CreateFunction(function) = statement else {
        return None;
    };
    function
        .args?
        .into_iter()
        .map(|parameter| {
            parameter.mode.is_none().then(|| FunctionParameter {
                name: parameter.name.map(|name| name.value).unwrap_or_default(),
                type_name: parameter.data_type.to_string(),
                default: parameter
                    .default_expr
                    .map(|expression| expression.to_string()),
            })
        })
        .collect()
}

/// One stored-function parameter and optional SQL default expression.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FunctionParameter {
    /// Parameter name.
    #[serde(default)]
    pub name: String,
    /// SQL argument type.
    #[serde(rename = "type", alias = "type_name")]
    pub type_name: String,
    /// Optional SQL `DEFAULT` expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
}

impl PartialEq for FunctionDef {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
            && self.schema == other.schema
            && self.parameters_sql() == other.parameters_sql()
            && self.returns == other.returns
            && self.language == other.language
            && self.body == other.body
            && self.depends_on == other.depends_on
            && self.volatility == other.volatility
            && self.security_definer == other.security_definer
            && self.opaque == other.opaque
    }
}

/// Stable overload identity excluding parameter names and defaults.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct FunctionIdentity {
    /// Qualified function name.
    pub name: String,
    /// Ordered argument types.
    pub argument_types: Vec<String>,
}

impl FunctionDef {
    /// Builds trusted opaque identity when executable source is unavailable.
    #[doc(hidden)]
    pub fn from_trusted_identity(name: impl Into<String>) -> Self {
        let mut value = Self::from_raw(name, "");
        value.opaque = OpaqueMeta::trusted_identity();
        value
    }
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    /// Returns the stable overload identity.
    pub fn identity(&self) -> FunctionIdentity {
        FunctionIdentity {
            name: self.qualified_name(),
            argument_types: self
                .parameters
                .iter()
                .map(|value| value.type_name.clone())
                .collect(),
        }
    }

    /// Returns the schema map key for this overload.
    pub fn identity_key(&self) -> String {
        if self.is_opaque() {
            return self.qualified_name();
        }
        if self.parameters.is_empty() {
            if self.arguments.trim().is_empty() {
                self.qualified_name()
            } else {
                format!("{}({})", self.qualified_name(), self.arguments)
            }
        } else {
            let identity = self.identity();
            format!("{}({})", identity.name, identity.argument_types.join(", "))
        }
    }

    /// Converts compatible legacy PostgreSQL argument text into typed parameters.
    ///
    /// Unrecognized text remains intact so callers can retain backward-compatible opaque
    /// declarations without inventing a lossy signature.
    #[doc(hidden)]
    pub fn normalize_legacy_parameters(&mut self) {
        if self.parameters.is_empty()
            && let Some(parameters) = legacy_function_parameters(&self.arguments)
        {
            self.parameters = parameters;
            self.arguments.clear();
        }
    }

    /// Renders complete declarations for CREATE FUNCTION.
    pub fn parameters_sql(&self) -> String {
        if self.parameters.is_empty() {
            return self.arguments.clone();
        }
        self.parameters
            .iter()
            .map(|parameter| match (&parameter.name, &parameter.default) {
                (name, Some(default)) if name.is_empty() => {
                    format!("{} DEFAULT {}", parameter.type_name, default)
                }
                (name, None) if name.is_empty() => parameter.type_name.clone(),
                (name, Some(default)) => {
                    format!("{} {} DEFAULT {}", name, parameter.type_name, default)
                }
                (name, None) => format!("{} {}", name, parameter.type_name),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Renders only overload types for DROP FUNCTION.
    pub fn argument_types_sql(&self) -> String {
        if self.parameters.is_empty() {
            return self.arguments.clone();
        }
        self.parameters
            .iter()
            .map(|parameter| parameter.type_name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            parameters: Vec::new(),
            arguments: String::new(),
            returns: String::new(),
            language: String::new(),
            body: String::new(),
            depends_on: Vec::new(),
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
        self.opaque.is_opaque()
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

/// Storage behavior for a generated column.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GeneratedStorage {
    /// Compute the value when it is read.
    Virtual,
    /// Persist the computed value in the table.
    Stored,
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
    /// Builds trusted opaque identity when executable source is unavailable.
    #[doc(hidden)]
    pub fn from_trusted_identity(name: impl Into<String>) -> Self {
        let mut value = Self::from_raw(name, "");
        value.opaque = OpaqueMeta::trusted_identity();
        value
    }

    /// Builds an opaque trigger from canonical trusted catalog source.
    #[doc(hidden)]
    pub fn from_trusted_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        let mut value = Self::from_raw(name, raw);
        value.mark_trusted();
        value
    }
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
        self.opaque.is_opaque()
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
    /// Builds trusted opaque identity when executable source is unavailable.
    #[doc(hidden)]
    pub fn from_trusted_identity(name: impl Into<String>) -> Self {
        let mut value = Self::from_raw(name, "");
        value.opaque = OpaqueMeta::trusted_identity();
        value
    }

    /// Builds an opaque view from canonical trusted catalog source.
    #[doc(hidden)]
    pub fn from_trusted_raw(name: impl Into<String>, raw: impl Into<String>) -> Self {
        let mut value = Self::from_raw(name, raw);
        value.mark_trusted();
        value
    }
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
        self.opaque.is_opaque()
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
        self.opaque.is_opaque()
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

/// A PostgreSQL sequence managed as one opaque root definition.
///
/// Gaman owns the definition and presence only. Runtime counter state is never
/// serialized, inspected, compared, or repaired.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SequenceDef {
    /// Unqualified sequence name.
    pub name: String,
    /// Optional PostgreSQL schema.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    /// One plain `CREATE SEQUENCE` statement. Inspected identity-only values omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sql: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) trusted: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl SequenceDef {
    /// Returns the canonical schema-qualified identity.
    pub fn qualified_name(&self) -> String {
        schema_qualified_key(&self.name, self.schema.as_deref())
    }

    /// Creates an authored opaque sequence definition.
    #[doc(hidden)]
    pub fn from_raw(name: impl Into<String>, sql: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            schema: None,
            sql: Some(sql.into()),
            trusted: false,
        }
    }

    /// Creates a catalog-derived identity without claiming executable source.
    #[doc(hidden)]
    pub fn trusted_identity(name: impl Into<String>, schema: Option<String>) -> Self {
        Self {
            name: name.into(),
            schema,
            sql: None,
            trusted: true,
        }
    }

    /// Returns stored authored SQL when available.
    #[doc(hidden)]
    pub fn raw_sql(&self) -> Option<&str> {
        self.sql.as_deref()
    }

    /// Reports whether this definition uses the opaque lifecycle.
    #[doc(hidden)]
    pub fn is_opaque(&self) -> bool {
        self.sql.is_some() || self.trusted
    }

    /// Marks an authored definition as accepted by structured clarification.
    #[doc(hidden)]
    pub fn mark_trusted(&mut self) {
        self.trusted = true;
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
        self.opaque.is_opaque()
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

    /// Returns the PostgreSQL range key when this table is a partitioned parent.
    pub fn postgres_range_partition_column(&self) -> Option<&str> {
        match &self.options.postgres_partition {
            Some(PostgresPartitionMeta::Parent { column }) => Some(column),
            _ => None,
        }
    }

    /// Returns parent and bounds when this table is a PostgreSQL range partition.
    #[doc(hidden)]
    pub fn postgres_range_partition_child(&self) -> Option<(&str, &str, &str)> {
        match &self.options.postgres_partition {
            Some(PostgresPartitionMeta::Child { parent, start, end }) => Some((parent, start, end)),
            _ => None,
        }
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
    /// Creates a modeled index over the supplied table columns.
    pub fn columns<I, S>(columns: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            columns: columns.into_iter().map(Into::into).collect(),
            ..Self::default()
        }
    }

    /// Overrides the deterministic name derived during schema normalization.
    pub fn named(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Marks this modeled index as unique.
    pub fn unique(mut self) -> Self {
        self.unique = true;
        self
    }

    /// Attach a partial-index predicate.
    pub fn predicate(mut self, expression: impl Into<String>) -> Self {
        self.predicate = Some(expression.into());
        self
    }

    /// Builds a trusted opaque index whose source is unavailable from the catalog.
    #[doc(hidden)]
    pub fn from_trusted_identity(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            columns: Vec::new(),
            unique: false,
            predicate: None,
            opaque: OpaqueMeta::trusted_identity(),
        }
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
        self.opaque.is_opaque()
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
        matches!(self, Self::Opaque { opaque, .. } if opaque.is_opaque())
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

/// MySQL-specific column properties.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MysqlColumnOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_increment: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update_expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invisible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl MysqlColumnOptions {
    /// Enables automatic sequence generation for this column.
    pub fn auto_increment(mut self) -> Self {
        self.auto_increment = true;
        self
    }
    /// Sets the automatic row-update expression.
    pub fn on_update(mut self, value: impl Into<String>) -> Self {
        self.on_update_expression = Some(value.into());
        self
    }
    /// Pins the column character set.
    pub fn character_set(mut self, value: impl Into<String>) -> Self {
        self.character_set = Some(value.into());
        self
    }
    /// Pins the column collation.
    pub fn collation(mut self, value: impl Into<String>) -> Self {
        self.collation = Some(value.into());
        self
    }
    /// Hides the column from implicit projections.
    pub fn invisible(mut self) -> Self {
        self.invisible = true;
        self
    }
    /// Sets the column comment.
    pub fn comment(mut self, value: impl Into<String>) -> Self {
        self.comment = Some(value.into());
        self
    }
}

/// MariaDB-specific column properties.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct MariadbColumnOptions {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_increment: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on_update_expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub character_set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub collation: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub invisible: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

impl MariadbColumnOptions {
    /// Enables automatic sequence generation for this column.
    pub fn auto_increment(mut self) -> Self {
        self.auto_increment = true;
        self
    }
    /// Sets the automatic row-update expression.
    pub fn on_update(mut self, value: impl Into<String>) -> Self {
        self.on_update_expression = Some(value.into());
        self
    }
    /// Pins the column character set.
    pub fn character_set(mut self, value: impl Into<String>) -> Self {
        self.character_set = Some(value.into());
        self
    }
    /// Pins the column collation.
    pub fn collation(mut self, value: impl Into<String>) -> Self {
        self.collation = Some(value.into());
        self
    }
    /// Hides the column from implicit projections.
    pub fn invisible(mut self) -> Self {
        self.invisible = true;
        self
    }
    /// Sets the column comment.
    pub fn comment(mut self, value: impl Into<String>) -> Self {
        self.comment = Some(value.into());
        self
    }
}

/// Product-specific column metadata with mutually exclusive product blocks.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ColumnDialectOptions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mysql: Option<MysqlColumnOptions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mariadb: Option<MariadbColumnOptions>,
}

impl ColumnDialectOptions {
    /// Returns the MySQL properties when selected.
    pub fn mysql(&self) -> Option<&MysqlColumnOptions> {
        self.mysql.as_ref()
    }

    /// Returns the MariaDB properties when selected.
    pub fn mariadb(&self) -> Option<&MariadbColumnOptions> {
        self.mariadb.as_ref()
    }
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
    /// Storage behavior for a generated expression.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_storage: Option<GeneratedStorage>,
    /// Dialect-owned metadata excluded from generic schema semantics.
    #[serde(flatten)]
    pub dialect_options: ColumnDialectOptions,
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
