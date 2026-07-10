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
pub mod dialects;
pub mod diff;
pub mod drift;
pub mod graphs;
pub mod migrations;
pub mod operations;
pub mod parsers;
pub mod repair;
pub mod replay;
#[doc(hidden)]
pub mod sql_plan;
pub mod states;

mod offline_planner;
mod opaque;

pub use dialects::{Dialect, DialectError};
pub use migrations::Migration;
pub use offline_planner::{EmbeddedMigrations, OfflineError, OfflinePlanner};

pub mod schema {
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::operations::Operation;
    pub use crate::parsers::ParseError;
    pub use crate::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, ConstraintInput, EnumDef, EnumInput,
        ExtensionDef, ExtensionInput, ForeignKey, FunctionDef, FunctionInput, Index, IndexInput,
        InputSchema, IntoTable, PrimaryKey, ReplayError, Schema, SchemaBuilder, SchemaLoadError,
        SchemaValidationError, Table, TableBuilder, TableInput, TriggerDef, TriggerEvent,
        TriggerInput, TriggerScope, TriggerTiming, ViewDef, ViewInput, Volatility, is_volatile,
        schema_qualified_key,
    };
}
