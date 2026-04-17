pub(crate) mod adapters;
pub mod sql;
pub mod cli;
pub(crate) mod column_type;
pub(crate) mod embed;
pub(crate) mod engine;
pub(crate) mod conf;
pub(crate) mod dialects;
pub(crate) mod diff;
pub(crate) mod diff2;
pub(crate) mod disambiguator;
pub(crate) mod executor;
pub(crate) mod graphs;
pub(crate) mod migrator;
pub(crate) mod migrations;
pub(crate) mod operations;
pub(crate) mod prompter;
pub(crate) mod states;

// Primary entry points — the everyday API.
pub use gaman_macros::{IntoTable, include_migrations};
pub use engine::{MigrationEngine, EngineError, TlsMode};
pub use conf::Config;
pub use migrations::Migration;

/// All types used to describe and build a database schema.
pub mod schema {
    pub use crate::states::{
        Schema, Table, Column, FunctionDef, TriggerDef, ViewDef, ExtensionDef, EnumDef,
        Index, Constraint, ForeignKey, ColumnRef,
        Volatility, TriggerTiming, TriggerEvent, TriggerScope,
        SchemaBuilder, TableBuilder, ColumnBuilder, IntoTable,
        ReplayError, SchemaLoadError,
        is_volatile, schema_qualified_key,
    };
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::operations::Operation;
    pub use crate::sql::SqlParseError;
}

/// Advanced types for custom executors, migration sources, and programmatic control.
pub mod core {
    pub use crate::migrator::{Migrator, MigratorError};
    pub use crate::executor::{
        Executor, ExecutorError, Invoker, InvokerError, Introspectable,
        PostgresExecutor, SubprocessInvoker,
    };
    pub use crate::adapters::{MigrationSource, AdapterError, YamlAdapter, VecAdapter};
    pub use crate::graphs::{MigrationGraph, MigrationNode, GraphError};
    pub use crate::dialects::{Dialect, DialectError};
    pub use crate::disambiguator::{Decision, PromptEngine};
    pub use crate::prompter::CliPromptEngine;
}
