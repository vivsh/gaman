//! Core schema diffing, migration planning, replay, clarification, and SQL rendering primitives.
//!
//! Offline planning turns authored schema input into deterministic migrations by replaying the
//! current graph, normalizing structural shorthand, clarifying risky choices, canonicalizing
//! dialect-specific names and types, and finally rendering SQL.
//!
//! ```text
//! [migrations] -> ReplayEngine -> current Schema
//!                                      |
//! [SQL/YAML/Rust schema] -> normalize -> desired Schema
//!                                      |
//! current + desired -> DiffEngine -> raw operations
//!                                      |
//! raw operations -> Clarifier -> resolved operations / Clarification prompts
//!                                      |
//! resolved operations -> dialect reorder -> Migration
//!                                      |
//! Migration replay -> normalize -> canonicalize -> validate
//!                                      |
//! SqlPlanRenderer + Dialect -> SQL statements
//! ```
//!
//! `parsers` loads SQL DDL into `Schema`; `offline_planner` owns offline generation; `sql_plan`
//! renders forward and rollback SQL from migrations.

pub mod clarifier;
pub mod column_type;
pub mod dialects;
pub mod diff;
pub mod graphs;
pub mod migrations;
pub mod operations;
pub mod parsers;
#[doc(hidden)]
pub mod sql_plan;
pub mod states;

mod offline_planner;
mod opaque;
mod replay;

pub use dialects::{Dialect, DialectError};
pub use migrations::Migration;
pub use offline_planner::{EmbeddedMigrations, OfflineError, OfflinePlanner};

pub mod schema {
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::operations::Operation;
    pub use crate::parsers::ParseError;
    pub use crate::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey,
        FunctionDef, Index, IntoSchema, IntoTable, PrimaryKey, ReplayError, Schema, SchemaBuilder,
        SchemaLoadError, SchemaValidationError, Table, TableBuilder, TriggerDef, TriggerEvent,
        TriggerScope, TriggerTiming, ViewDef, Volatility, is_volatile, schema_qualified_key,
    };
}
