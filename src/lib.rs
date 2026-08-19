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

#[cfg(feature = "cli")]
pub mod cli;
#[cfg(feature = "native")]
pub(crate) mod conf;
#[cfg(feature = "db")]
pub(crate) mod environment;
#[cfg(feature = "db")]
pub(crate) mod executor;
#[cfg(feature = "fs")]
mod migration_store;
#[cfg(feature = "cli")]
pub(crate) mod prompter;
#[cfg(feature = "db")]
pub mod runner_factory;
#[cfg(feature = "native")]
pub mod schema_file;

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
pub use gaman_core::{
    ApplyCommand, COMMAND_PROTOCOL_VERSION, Command as RunnerCommand, CommandDiagnostic,
    CommandEnvelope, CommandError as RunnerCommandError, CommandFailure, CommandRequest,
    CommandResponse, CommandResult, DatabaseTrackingStore, DiagnosticCode, Executor, ExecutorError,
    InspectionError, MakeCommand, MakeResult, MigrationArtifact, MigrationCatalog, MigrationEngine,
    MigrationMovement, MigrationRunner, MigrationStatus, RepairOptions, RepairReport,
    SchemaCheckFailure, SchemaCheckInput, SchemaCheckResult, SchemaCheckStatus, SchemaInspector,
    SqlInput, TrackingStore,
};
#[cfg(feature = "native")]
pub use gaman_core::{EmbeddedMigrations, EngineError, Migration, OfflineError, OfflinePlanner};

/// Schema types and builders.
#[cfg(feature = "native")]
pub mod schema {
    pub use gaman_core::column_type::{ColumnDesc, ColumnType};
    pub use gaman_core::entity_selector::EntityDependency;
    pub use gaman_core::operations::Operation;
    pub use gaman_core::parsers::ParseError;
    pub use gaman_core::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, ConstraintInput, EnumDef, EnumInput,
        ExtensionDef, ExtensionInput, ForeignKey, FunctionBuilder, FunctionDef, FunctionIdentity,
        FunctionInput, FunctionParameter, GeneratedStorage,
        Index, IndexInput, InputSchema, IntoTable, PostgresRangePartition,
        PostgresRangePartitioning, PrimaryKey, ReplayError, Schema, SchemaBuilder, SchemaLoadError,
        SchemaValidationError, Table, TableBuilder, TableInput, TriggerDef, TriggerEvent,
        TriggerInput, TriggerScope, TriggerTiming, ViewDef, ViewInput, Volatility, is_volatile,
        schema_qualified_key,
    };
}

/// Lower-level APIs for custom executors, sources, and integration work.
#[cfg(feature = "native")]
pub mod core {
    #[cfg(feature = "db")]
    pub use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
    #[cfg(any(feature = "mysql", feature = "mariadb"))]
    pub use crate::executor::MysqlFamilyExecutor;
    #[cfg(feature = "postgres")]
    pub use crate::executor::PostgresExecutor;
    #[cfg(feature = "sqlite")]
    pub use crate::executor::SqliteExecutor;
    #[cfg(feature = "db")]
    pub use gaman_core::clarifier::{
        Answer, Clarification, ClarificationKind, ClarificationMessage, ClarificationOption,
        Decision, OptionAction, PromptEngine, Severity, clarification_message,
    };
    pub use gaman_core::dialects::{Dialect, DialectError};
    #[cfg(feature = "db")]
    pub use gaman_core::drift::{DriftFinding, VerificationReport};
    pub use gaman_core::graphs::{GraphError, MigrationGraph, MigrationNode};
    pub use gaman_core::sql_plan::{SqlPlanError, SqlPlanRenderer};
    pub use gaman_core::{
        BoxFuture, DatabaseTrackingStore, Executor, ExecutorError, MigrationEngine, MigrationStore,
        OfflineError, OfflinePlanner, SchemaInspector, StoreError, TRACKING_TABLE, TrackingStore,
    };
}
