#[cfg(all(not(feature = "native"), not(feature = "offline")))]
compile_error!("enable either the 'offline' feature or a native feature set");

#[cfg(all(feature = "offline", not(feature = "native")))]
pub use gaman_core::*;

#[cfg(all(feature = "offline", not(feature = "native")))]
pub mod core {
    pub use gaman_core::clarifier::{
        Answer, Clarification, ClarificationKind, ClarificationMessage, ClarificationOption,
        Decision, OptionAction, PromptEngine, Severity, clarification_message,
    };
    pub use gaman_core::dialects::{Dialect, DialectError};
    pub use gaman_core::graphs::{GraphError, MigrationGraph, MigrationNode};
    pub use gaman_core::{EmbeddedMigrations, Migration, OfflineError, OfflinePlanner};
}

#[cfg(feature = "native")]
pub(crate) mod adapters;
#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "native")]
pub(crate) mod conf;
#[cfg(feature = "cli")]
pub(crate) mod engine;
#[cfg(feature = "db")]
pub(crate) mod environment;
#[cfg(feature = "db")]
pub(crate) mod executor;
#[cfg(feature = "db")]
pub(crate) mod inspection;
#[cfg(feature = "db")]
pub(crate) mod migrator;
#[cfg(feature = "cli")]
pub(crate) mod prompter;
#[cfg(feature = "native")]
pub mod schema_file;
#[cfg(feature = "db")]
pub(crate) mod tracking;
#[cfg(feature = "db")]
pub mod verification;

#[cfg(feature = "native")]
pub mod parsers {
    pub use gaman_core::parsers::*;
}

// Everyday API.
#[cfg(feature = "native")]
pub use conf::{Config, ConfigError, TlsMode};
#[cfg(feature = "cli")]
pub use engine::{EmbeddedMigrations, EngineError, MigrationEngine};
#[cfg(feature = "native")]
pub use gaman_core::Migration;

/// Schema types and builders.
#[cfg(feature = "native")]
pub mod schema {
    pub use gaman_core::column_type::{ColumnDesc, ColumnType};
    pub use gaman_core::operations::Operation;
    pub use gaman_core::parsers::ParseError;
    pub use gaman_core::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey,
        FunctionDef, Index, IntoSchema, IntoTable, PrimaryKey, ReplayError, Schema, SchemaBuilder,
        SchemaLoadError, SchemaValidationError, Table, TableBuilder, TriggerDef, TriggerEvent,
        TriggerScope, TriggerTiming, ViewDef, Volatility, is_volatile, schema_qualified_key,
    };
}

/// Lower-level APIs for custom executors, sources, and integration work.
#[cfg(feature = "native")]
pub mod core {
    pub use crate::adapters::{AdapterError, MigrationSource, VecAdapter, YamlAdapter};
    #[cfg(feature = "db")]
    pub use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
    #[cfg(feature = "postgres")]
    pub use crate::executor::PostgresExecutor;
    #[cfg(feature = "sqlite")]
    pub use crate::executor::SqliteExecutor;
    #[cfg(feature = "db")]
    pub use crate::executor::{BoxFuture, Executor, ExecutorError, Introspectable};
    #[cfg(feature = "db")]
    pub use crate::migrator::{Migrator, MigratorError};
    #[cfg(feature = "cli")]
    pub use crate::prompter::CliPromptEngine;
    #[cfg(feature = "db")]
    pub use crate::tracking::{DatabaseTrackingStore, TrackingError, TrackingStore};
    #[cfg(feature = "db")]
    pub use crate::verification::{DriftFinding, VerificationReport};
    pub use gaman_core::clarifier::{
        Answer, Clarification, ClarificationKind, ClarificationMessage, ClarificationOption,
        Decision, OptionAction, PromptEngine, Severity, clarification_message,
    };
    pub use gaman_core::dialects::{Dialect, DialectError};
    pub use gaman_core::graphs::{GraphError, MigrationGraph, MigrationNode};
}
