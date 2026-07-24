//! Portable command protocol and uniform migration lifecycle runner.

mod adapters;
mod diagnostics;
mod dispatch;
mod protocol;
mod selector;

pub use adapters::{InspectionError, SchemaInspector};
pub use diagnostics::{CommandDiagnostic, CommandError, CommandFailure, DiagnosticCode};
pub use dispatch::MigrationRunner;
pub use protocol::{
    ApplyCommand, COMMAND_PROTOCOL_VERSION, Command, CommandEnvelope, CommandRequest,
    CommandResponse, CommandResult, MakeCommand, MakeResult, MigrationStatus, RepairOptions,
    RepairReport, SchemaCheckFailure, SchemaCheckInput, SchemaCheckResult, SchemaCheckStatus,
    SqlInput,
};
pub use selector::EntityFilter;
