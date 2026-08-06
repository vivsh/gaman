//! Core schema modeling, lexical diffing, semantic drift, migration planning,
//! replay, clarification, and SQL rendering primitives.
//!
//! Gaman works with three schema sources: authored input, replayed migration
//! state, and live inspected state. Authored input is normalized and lexically
//! diffed against replayed state to generate migrations. Live inspected state is
//! normalized through the selected dialect and semantically drift-diffed against
//! replayed state using dialect comparator callbacks.
//!
//! ```text
//! input Schema -> normalize + prepare(dialect)
//!                         |
//! replayed Schema --------+-> DiffEngine -> raw operations -> Clarifier -> Migration
//!
//! inspected Schema -> normalize_inspected_schema(dialect)
//!                         |
//! replayed Schema --------+-> drift::diff -> VerificationReport
//!
//! VerificationReport -> repair::plan_repair -> repair operations -> SQL
//!
//! Migration replay -> prepare(dialect) -> SqlPlanRenderer + Dialect -> SQL
//! ```
//!
//! `parsers` loads SQL DDL into `Schema`; `offline_planner` owns offline
//! generation; `drift` owns semantic live drift reports; `repair` owns
//! database-I/O-free drift repair planning; `sql_plan` renders forward,
//! rollback, and repair SQL from migrations.

pub mod clarifier;
pub mod column_type;
#[cfg(feature = "command-args")]
pub mod command_args;
pub mod dialects;
pub mod diff;
pub mod drift;
mod entity_filter;
pub mod graphs;
pub mod managed_rows;
pub mod migration_engine;
mod migration_filter;
pub mod migrations;
pub mod operations;
pub mod parsers;
pub mod redaction;
pub mod repair;
pub mod replay;
pub mod runner;
#[doc(hidden)]
pub mod sql_plan;
pub mod states;

mod migration_normalize;
mod offline_planner;
mod opaque;

pub use dialects::{Dialect, DialectError};
pub use entity_filter::EntityFilter;
pub use migration_engine::{
    BoxFuture, DatabaseTrackingStore, EngineError, Executor, ExecutorError, MigrationArtifact,
    MigrationCatalog, MigrationEngine, MigrationMovement, MigrationStore, StoreError,
    TRACKING_TABLE, TrackingError, TrackingStore,
};
pub use migrations::Migration;
pub use offline_planner::{EmbeddedMigrations, OfflineError, OfflinePlanner};
pub use redaction::redact_diagnostic_text;
pub use runner::{
    ApplyCommand, COMMAND_PROTOCOL_VERSION, Command, CommandDiagnostic, CommandEnvelope,
    CommandError, CommandFailure, CommandRequest, CommandResponse, CommandResult, DiagnosticCode,
    InspectionError, MakeCommand, MakeResult, MigrationRunner, MigrationStatus, RepairOptions,
    RepairReport, SchemaCheckFailure, SchemaCheckInput, SchemaCheckResult, SchemaCheckStatus,
    SchemaInspector, SqlInput,
};

pub mod schema {
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::managed_rows::{ManagedRow, ManagedRows, ManagedValue};
    pub use crate::operations::Operation;
    pub use crate::parsers::ParseError;
    pub use crate::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, ConstraintInput, EnumDef, EnumInput,
        ExtensionDef, ExtensionInput, ForeignKey, FunctionDef, FunctionInput, GeneratedStorage,
        Index, IndexInput, InputSchema, IntoTable, PostgresRangePartition,
        PostgresRangePartitioning, PrimaryKey, ReplayError, Schema, SchemaBuilder, SchemaLoadError,
        SchemaValidationError, Table, TableBuilder, TableInput, TriggerDef, TriggerEvent,
        TriggerInput, TriggerScope, TriggerTiming, ViewDef, ViewInput, Volatility, is_volatile,
        schema_qualified_key,
    };
}
