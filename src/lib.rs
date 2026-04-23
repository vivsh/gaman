pub(crate) mod adapters;
pub mod sql;
pub mod cli;
pub(crate) mod column_type;
pub(crate) mod engine;
pub(crate) mod conf;
pub(crate) mod dialects;
pub(crate) mod environment;
pub(crate) mod diff;
#[allow(dead_code)]
pub(crate) mod diff_legacy;
pub(crate) mod disambiguator;
pub(crate) mod executor;
pub(crate) mod graphs;
pub(crate) mod migrator;
pub(crate) mod migrations;
pub(crate) mod operations;
pub(crate) mod prompter;
pub(crate) mod states;

// Everyday API.
pub use gaman_macros::{IntoTable, embedded_migrations};
pub use engine::{MigrationEngine, EngineError, EmbeddedMigrations};
pub use conf::{Config, TlsMode};
pub use migrations::Migration;

/// Schema types and builders.
pub mod schema {
    pub use crate::states::{
        Schema, Table, Column, FunctionDef, TriggerDef, ViewDef, ExtensionDef, EnumDef,
        Index, Constraint, ForeignKey, ColumnRef,
        Volatility, TriggerTiming, TriggerEvent, TriggerScope,
        SchemaBuilder, TableBuilder, ColumnBuilder, IntoTable, IntoSchema,
        ReplayError, SchemaLoadError,
        is_volatile, schema_qualified_key,
    };
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::operations::Operation;
    pub use crate::sql::SqlParseError;
}

/// Lower-level APIs for custom executors, sources, and integration work.
pub mod core {
    pub use crate::migrator::{Migrator, MigratorError};
    pub use crate::executor::{
        Executor, ExecutorError, Invoker, InvokerError, Introspectable,
        PostgresExecutor, SubprocessInvoker,
    };
    pub use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
    pub use crate::adapters::{MigrationSource, AdapterError, YamlAdapter, VecAdapter};
    pub use crate::graphs::{MigrationGraph, MigrationNode, GraphError};
    pub use crate::dialects::{Dialect, DialectError};
    pub use crate::disambiguator::{Answer, Clarification, ClarificationKind, Decision, PromptEngine, Severity};
    pub use crate::prompter::CliPromptEngine;
}
