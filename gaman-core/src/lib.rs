pub mod column_type;
pub mod dialects;
pub mod diff;
pub mod disambiguator;
pub mod graphs;
pub mod migrations;
pub mod operations;
pub mod sql;
pub mod states;

mod offline;
mod opaque;

pub use dialects::{Dialect, DialectError};
pub use migrations::Migration;
pub use offline::{EmbeddedMigrations, OfflineError, OfflinePlanner};

pub mod schema {
    pub use crate::column_type::{ColumnDesc, ColumnType};
    pub use crate::operations::Operation;
    pub use crate::sql::SqlParseError;
    pub use crate::states::{
        Column, ColumnBuilder, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey,
        FunctionDef, Index, IntoSchema, IntoTable, PrimaryKey, ReplayError, Schema, SchemaBuilder,
        SchemaLoadError, Table, TableBuilder, TriggerDef, TriggerEvent, TriggerScope,
        TriggerTiming, ViewDef, Volatility, is_volatile, schema_qualified_key,
    };
}
