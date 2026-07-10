//! Native database execution, filesystem integration, and CLI APIs for Gaman.
//!
//! The root crate connects the database-I/O-free `gaman-core` lifecycle to
//! migration sources, live database executors, inspection, and user-facing
//! commands. Offline-only builds re-export the core planning API.

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
#[cfg(feature = "db")]
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
#[cfg(feature = "db")]
mod schema_check;
#[cfg(feature = "native")]
pub mod schema_file;
#[cfg(feature = "db")]
pub(crate) mod tracking;

#[cfg(feature = "native")]
pub mod parsers {
    pub use gaman_core::parsers::*;
}

/// Semantic drift reports, contracts, comparison, and formatting.
#[cfg(feature = "native")]
pub mod drift {
    pub use gaman_core::drift::*;
}

// Everyday API.
#[cfg(feature = "native")]
pub use conf::{Config, ConfigError, TlsMode};
#[cfg(feature = "db")]
pub use engine::{EmbeddedMigrations, EngineError, MigrationEngine};
#[cfg(feature = "native")]
pub use gaman_core::Migration;
#[cfg(feature = "db")]
pub use migrator::{
    MigrationArtifact, MigrationListing, MigrationMovement, RepairOptions, RepairReport,
};
#[cfg(feature = "db")]
pub use schema_check::{
    SchemaCheckFailure, SchemaCheckFileReport, SchemaCheckFileStatus, SchemaCheckReport,
    SqlSchemaInput,
};

/// Schema types and builders.
#[cfg(feature = "native")]
pub mod schema {
    pub use gaman_core::column_type::{ColumnDesc, ColumnType};
    pub use gaman_core::operations::Operation;
    pub use gaman_core::parsers::ParseError;
    pub use gaman_core::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, ConstraintInput, EnumDef, EnumInput,
        ExtensionDef, ExtensionInput, ForeignKey, FunctionDef, FunctionInput, Index, IndexInput,
        InputSchema, IntoTable, PrimaryKey, ReplayError, Schema, SchemaBuilder, SchemaLoadError,
        SchemaValidationError, Table, TableBuilder, TableInput, TriggerDef, TriggerEvent,
        TriggerInput, TriggerScope, TriggerTiming, ViewDef, ViewInput, Volatility, is_volatile,
        schema_qualified_key,
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
    pub use crate::migrator::{
        MigrationMovement, Migrator, MigratorError, RepairOptions, RepairReport,
    };
    #[cfg(feature = "db")]
    pub use crate::tracking::{
        DatabaseTrackingStore, TRACKING_TABLE, TrackingError, TrackingStore,
    };
    pub use gaman_core::clarifier::{
        Answer, Clarification, ClarificationKind, ClarificationMessage, ClarificationOption,
        Decision, OptionAction, PromptEngine, Severity, clarification_message,
    };
    pub use gaman_core::dialects::{Dialect, DialectError};
    #[cfg(feature = "db")]
    pub use gaman_core::drift::{DriftFinding, VerificationReport};
    pub use gaman_core::graphs::{GraphError, MigrationGraph, MigrationNode};
}
