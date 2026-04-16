pub(crate) mod adapters;
pub(crate) mod column_type;
pub(crate) mod embed;
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

// Flat public API — all symbols accessible directly from the crate root.

pub use gaman_macros::{IntoTable, include_migrations};

pub use states::{
    Schema, Table, Column, FunctionDef, TriggerDef, ViewDef, ExtensionDef, EnumDef,
    Index, Constraint, ForeignKey, ColumnRef,
    Volatility, TriggerTiming, TriggerEvent, TriggerScope,
    SchemaBuilder, TableBuilder, ColumnBuilder, IntoTable,
    ReplayError, SchemaLoadError,
    is_volatile, schema_qualified_key,
};

pub use operations::Operation;

pub use dialects::{Dialect, DialectError};

pub use adapters::{MigrationSource, AdapterError, YamlAdapter, VecAdapter};

pub use migrations::Migration;

pub use graphs::{MigrationGraph, MigrationNode, GraphError};

pub use migrator::{Migrator, MigratorError};

pub use executor::{
    Executor, ExecutorError, InvokerError, Invoker, Introspectable,
    PostgresExecutor, SubprocessInvoker,
};

pub use diff::{DiffEngine, DiffError};

pub use conf::Config;

pub use embed::{EmbedSource, EmbedError};

pub use column_type::{ColumnDesc, ColumnType};

pub use disambiguator::{
    Severity, ClarificationKind, Clarification, Answer, Decision,
    DisambiguationResult, DisambiguatorError, PromptError, PromptEngine, Disambiguator,
};

pub use prompter::{
    OptionAction, ClarificationOption, ClarificationMessage, clarification_message, CliPromptEngine,
};
