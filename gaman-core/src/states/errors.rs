use thiserror::Error;

#[derive(Debug, Error, PartialEq)]
pub enum SchemaValidationError {
    #[error("{0}")]
    Invalid(String),
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
