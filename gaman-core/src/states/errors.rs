use thiserror::Error;

/// One deterministic problem found while compiling fluent schema-builder input.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchemaBuilderIssue {
    /// An extension targeted a table that is not present in the schema.
    #[error("table '{table}' cannot be extended because it does not exist")]
    MissingTable { table: String },
    /// An extension closure attempted to change the identity of its table.
    #[error("table extension changed identity from '{expected}' to '{observed}'")]
    TableIdentityChanged { expected: String, observed: String },
    /// An opaque declaration collided with an entity already registered under that identity.
    #[error("duplicate {kind} identity '{entity}' in schema builder")]
    DuplicateEntity { kind: String, entity: String },
    /// A builder identity was not an unambiguous one- or two-part SQL name.
    #[error("invalid qualified identity '{name}': {reason}")]
    InvalidQualifiedName { name: String, reason: String },
    /// An opaque definition could not prove the identity required for safe lifecycle operations.
    #[error("invalid opaque {kind} '{entity}': {reason}")]
    InvalidOpaqueDefinition {
        kind: String,
        entity: String,
        reason: String,
    },
    /// An unmanaged table fragment could escape or corrupt its CREATE TABLE position.
    #[error("invalid unmanaged {placement} on table '{table}': {reason}")]
    InvalidUnmanagedClause {
        table: String,
        placement: String,
        reason: String,
    },
    /// A managed-row declaration could not be serialized or merged safely.
    #[error("invalid managed rows for '{table}': {reason}")]
    InvalidManagedRows { table: String, reason: String },
    /// A fluent function declaration was incomplete or contained an invalid selector.
    #[error("invalid function definition '{function}': {reason}")]
    InvalidFunctionDefinition { function: String, reason: String },
}

/// Ordered builder failures returned together from the terminal build operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaBuilderErrors {
    issues: Vec<SchemaBuilderIssue>,
}

impl SchemaBuilderErrors {
    /// Creates an ordered error collection from all builder validation failures.
    pub fn new(issues: Vec<SchemaBuilderIssue>) -> Self {
        Self { issues }
    }

    /// Returns the individual failures in deterministic schema order.
    pub fn issues(&self) -> &[SchemaBuilderIssue] {
        &self.issues
    }
}

impl std::fmt::Display for SchemaBuilderErrors {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{issue}")?;
        }
        Ok(())
    }
}

impl std::error::Error for SchemaBuilderErrors {}

#[derive(Debug, Error, PartialEq)]
pub enum SchemaValidationError {
    #[error("{0}")]
    Invalid(String),
    /// A schema uses modeled metadata that the selected dialect cannot represent.
    #[error("{dialect} does not support {feature} on table '{table}'")]
    UnsupportedDialectFeature {
        /// Selected database dialect.
        dialect: String,
        /// Unsupported modeled capability.
        feature: String,
        /// Table carrying the unsupported metadata.
        table: String,
    },
    /// Fluent builder declarations failed before schema preparation.
    #[error("schema builder validation failed: {0}")]
    Builder(#[from] SchemaBuilderErrors),
}

impl From<String> for SchemaValidationError {
    fn from(value: String) -> Self {
        Self::Invalid(value)
    }
}

impl From<&str> for SchemaValidationError {
    fn from(value: &str) -> Self {
        Self::Invalid(value.to_string())
    }
}

#[derive(Debug, Error, PartialEq)]
pub enum ReplayError {
    #[error("invalid migration structure: {0}")]
    InvalidMigration(String),
    /// An opaque operation contains caller-owned lifecycle modifiers or mismatched identity.
    #[error("invalid opaque CREATE for '{entity}': {reason}")]
    InvalidOpaqueCreate {
        /// Canonical operation entity identity.
        entity: String,
        /// Stable parser-owned rejection reason.
        reason: String,
    },
    #[error("migration '{0}' not found in graph")]
    MigrationNotFound(String),
    #[error("table '{0}' already exists")]
    TableAlreadyExists(String),
    #[error("table '{0}' not found")]
    TableNotFound(String),
    #[error("table '{new}' already exists, cannot rename '{old}' to it")]
    RenameTargetExists { old: String, new: String },
    #[error("column '{column}' already exists on table '{table}'")]
    ColumnAlreadyExists { table: String, column: String },
    #[error("column '{column}' not found on table '{table}'")]
    ColumnNotFound { table: String, column: String },
    #[error("foreign key '{fk}' already exists on table '{table}'")]
    ForeignKeyAlreadyExists { table: String, fk: String },
    #[error("foreign key '{fk}' not found on table '{table}'")]
    ForeignKeyNotFound { table: String, fk: String },
    #[error("index '{index}' already exists on table '{table}'")]
    IndexAlreadyExists { table: String, index: String },
    #[error("index '{index}' not found on table '{table}'")]
    IndexNotFound { table: String, index: String },
    #[error("constraint '{constraint}' already exists on table '{table}'")]
    ConstraintAlreadyExists { table: String, constraint: String },
    #[error("constraint '{constraint}' not found on table '{table}'")]
    ConstraintNotFound { table: String, constraint: String },
    #[error("table '{0}' has multiple primary key declarations")]
    MultiplePrimaryKeys(String),
    #[error(
        "primary key changes on existing table '{0}' are not generated automatically; use an explicit SQL statement migration"
    )]
    PrimaryKeyMutation(String),
    #[error("function '{0}' already exists")]
    FunctionAlreadyExists(String),
    #[error("function '{0}' not found")]
    FunctionNotFound(String),
    #[error("trigger '{trigger}' already exists on table '{table}'")]
    TriggerAlreadyExists { table: String, trigger: String },
    #[error("trigger '{trigger}' not found on table '{table}'")]
    TriggerNotFound { table: String, trigger: String },
    #[error("view '{0}' already exists")]
    ViewAlreadyExists(String),
    #[error("view '{0}' not found")]
    ViewNotFound(String),
    #[error("extension '{0}' already exists")]
    ExtensionAlreadyExists(String),
    #[error("extension '{0}' not found")]
    ExtensionNotFound(String),
    #[error("sequence '{0}' already exists")]
    SequenceAlreadyExists(String),
    #[error("sequence '{0}' not found")]
    SequenceNotFound(String),
    #[error("enum '{0}' already exists")]
    EnumAlreadyExists(String),
    #[error("enum '{0}' not found")]
    EnumNotFound(String),
    #[error("in migration '{migration}' (operation {op_num}: {operation})")]
    WithContext {
        migration: String,
        op_num: usize,
        /// Human-readable identity of the operation that failed during replay.
        operation: String,
        #[source]
        inner: Box<ReplayError>,
    },
}

#[derive(Debug, Error)]
pub enum SchemaLoadError {
    #[error("cannot read '{0}': {1}")]
    Io(String, #[source] std::io::Error),
    #[error("in schema '{path}': {source}")]
    Path {
        path: String,
        #[source]
        source: Box<SchemaLoadError>,
    },
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("table '{table}' defined in both '{a}' and '{b}'")]
    Merge { table: String, a: String, b: String },
    #[error("duplicate table '{0}' when merging schemas")]
    DuplicateTable(String),
    #[error("schema validation failed: {0}")]
    Validation(#[from] SchemaValidationError),
    #[error(transparent)]
    Sql(Box<crate::parsers::ParseError>),
}

impl From<crate::parsers::ParseError> for SchemaLoadError {
    fn from(error: crate::parsers::ParseError) -> Self {
        Self::Sql(Box::new(error))
    }
}
