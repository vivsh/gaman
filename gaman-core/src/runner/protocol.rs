use serde::{Deserialize, Serialize, Serializer};

use super::diagnostics::CommandError;
use super::selector::EntityFilter;
use crate::clarifier::Decision;
use crate::drift::{DriftFinding, VerificationReport};
use crate::migration_engine::{MigrationArtifact, MigrationMovement};
use crate::operations::Operation;
use crate::states::Schema;
/// Resolved lifecycle command accepted by [`MigrationRunner`].
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "command", content = "arguments", rename_all = "snake_case")]
pub enum Command {
    /// Generate, inspect, or check a desired schema against migration history.
    Make(MakeCommand),
    /// Apply, plan, or check migration movement.
    Apply(ApplyCommand),
    /// Return migration application state.
    Status {
        reverse: bool,
        search: Option<String>,
    },
    /// Return canonical migration content.
    Show {
        id: Option<String>,
        reverse: bool,
        search: Option<String>,
    },
    /// Render forward or rollback migration SQL.
    Sql { id: Option<String>, backwards: bool },
    /// Prepare SQL source segments without executing them.
    CheckSchema { inputs: Vec<SchemaCheckInput> },
    /// Reflect selected database namespaces.
    Inspect {
        schemas: Vec<String>,
        /// Root entities selected from catalog inspection.
        #[serde(default)]
        filters: Vec<EntityFilter>,
        /// Legacy single-table selector retained for protocol-v2 consumers.
        #[serde(default)]
        table: Option<String>,
    },
    /// Compare replayed migration ownership against live inspection.
    Verify { schemas: Vec<String> },
    /// Plan or apply one-off repair SQL from verified drift.
    Repair {
        schemas: Vec<String>,
        options: RepairOptions,
    },
}

impl Command {
    /// Returns the clarification decisions already attached to this command.
    pub fn decisions(&self) -> Option<&[Decision]> {
        match self {
            Self::Make(MakeCommand::Generate { decisions, .. })
            | Self::Make(MakeCommand::Check { decisions, .. }) => Some(decisions),
            _ => None,
        }
    }

    /// Returns a retry command with additional clarification decisions attached.
    pub fn with_decisions(&self, additional: Vec<Decision>) -> Result<Self, CommandError> {
        let mut command = self.clone();
        match &mut command {
            Self::Make(MakeCommand::Generate { decisions, .. })
            | Self::Make(MakeCommand::Check { decisions, .. }) => {
                decisions.extend(additional);
                Ok(command)
            }
            _ => Err(CommandError::Invalid(
                "this command does not accept clarification decisions".to_string(),
            )),
        }
    }
}

/// Portable command vocabulary used by textual, WASM, and other hosts.
pub type CommandRequest = Command;

/// Current version of the portable command request and response protocol.
pub const COMMAND_PROTOCOL_VERSION: u32 = 4;

/// Versioned portable command request envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandEnvelope {
    /// Protocol version used to encode the request.
    pub protocol_version: u32,
    /// Resolved lifecycle command.
    pub command: CommandRequest,
}

/// Requested migration-generation behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum MakeCommand {
    /// Generate a normal migration, optionally persisting it.
    Generate {
        schema: Schema,
        name: Option<String>,
        dry_run: bool,
        decisions: Vec<Decision>,
        /// Invocation-scoped root filters; empty preserves complete generation.
        #[serde(default)]
        filters: Vec<EntityFilter>,
    },
    /// Create a named empty migration.
    Empty { name: String },
    /// Create a named merge migration.
    Merge { name: String },
    /// Report whether the prepared schema has unapplied changes.
    Check {
        schema: Schema,
        decisions: Vec<Decision>,
    },
}

/// Requested migration-application behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum ApplyCommand {
    /// Execute pending migrations, optionally converging on a target.
    Execute {
        target: Option<String>,
        fake: bool,
        /// Verifies candidate owned state against the live database before faking.
        #[serde(default)]
        fake_verified: bool,
        /// Namespaces used only by verified fake application.
        #[serde(default)]
        schemas: Vec<String>,
    },
    /// Return pending migration identifiers without mutation.
    Plan,
    /// Fail when pending migrations exist without mutating state.
    Check,
}

/// One in-memory SQL source supplied by a host for database preparation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SqlInput {
    /// Host-visible label used in check results.
    pub name: String,
    /// SQL source to segment and prepare.
    pub sql: String,
}

/// One host-resolved input participating in SQL schema validation.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SchemaCheckInput {
    /// SQL source that should be segmented and prepared against the live target.
    Sql(SqlInput),
    /// Non-SQL schema input retained in the report without opening a connection.
    Ignored {
        /// Host-visible input label.
        name: String,
        /// Concise reason the input is not prepared as SQL.
        reason: String,
    },
}

/// Options that control one-off drift repair.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct RepairOptions {
    /// Execute rendered repair SQL instead of returning a dry-run plan.
    pub apply: bool,
    /// Allow repair while migrations remain pending.
    pub allow_pending: bool,
    /// Apply supported repair operations and retain unsupported findings.
    pub allow_partial: bool,
    /// Request SQL-oriented host presentation.
    pub sql_only: bool,
}

/// Structured result of one lifecycle command.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "result", content = "value", rename_all = "snake_case")]
pub enum CommandResult {
    /// Result of normal, empty, merge, or check migration generation.
    Make(MakeResult),
    /// Applied or reverted migration counts.
    Movement(MigrationMovement),
    /// Pending migration identifiers.
    Pending(Vec<String>),
    /// Migration identifiers with applied flags.
    Status(Vec<MigrationStatus>),
    /// Canonical migration YAML artifacts.
    Show(Vec<MigrationArtifact>),
    /// Rendered SQL statements.
    Sql(Vec<String>),
    /// Per-source database preparation results.
    SchemaCheck(Vec<SchemaCheckResult>),
    /// High-fidelity inspected schema.
    Inspect(Schema),
    /// Semantic drift report.
    Verify(VerificationReport),
    /// One-off repair plan or application report.
    Repair(RepairReport),
}

/// Versioned portable command response envelope.
#[derive(Debug, Clone, Serialize)]
pub struct CommandResponse {
    /// Protocol version used to encode the response.
    pub protocol_version: u32,
    /// Structured lifecycle result.
    pub result: CommandResult,
}

impl CommandResponse {
    /// Wraps one command result in the current protocol version.
    pub fn new(result: CommandResult) -> Self {
        Self {
            protocol_version: COMMAND_PROTOCOL_VERSION,
            result,
        }
    }
}

/// Outcome of migration generation with persistence semantics preserved for hosts.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "outcome", content = "migration", rename_all = "snake_case")]
pub enum MakeResult {
    /// A generated migration was persisted to the configured migration store.
    Created(#[serde(serialize_with = "serialize_migration_with_id")] crate::migrations::Migration),
    /// A generated migration was returned without being persisted.
    Preview(#[serde(serialize_with = "serialize_migration_with_id")] crate::migrations::Migration),
    /// Desired schema already matches committed migration history.
    NoChanges,
    /// Schema-check mode confirmed that no migration is required.
    CheckPassed,
}

/// Serializes a command-result migration with its filename-derived identifier included.
fn serialize_migration_with_id<S>(
    migration: &crate::migrations::Migration,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    #[derive(Serialize)]
    struct MigrationValue<'a> {
        id: &'a str,
        dependencies: &'a [String],
        operations: &'a [Operation],
        atomic: bool,
    }

    MigrationValue {
        id: &migration.id,
        dependencies: &migration.dependencies,
        operations: &migration.operations,
        atomic: migration.atomic,
    }
    .serialize(serializer)
}

/// One migration id and its tracked application state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationStatus {
    /// Migration identifier.
    pub id: String,
    /// Whether target tracking state marks this migration as applied.
    pub applied: bool,
}

/// Database preparation outcome for one supplied SQL source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaCheckResult {
    /// Host-visible schema input name.
    pub name: String,
    /// Structured validation status for this input.
    pub status: SchemaCheckStatus,
}

/// Result category for one schema-check input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SchemaCheckStatus {
    /// SQL source was segmented and each segment was prepared independently.
    Checked {
        /// Number of statements prepared successfully.
        passed: usize,
        /// Segmentation or statement preparation failures.
        failures: Vec<SchemaCheckFailure>,
    },
    /// Input is not SQL and was intentionally excluded from live preparation.
    Ignored {
        /// Host-provided explanation for the exclusion.
        reason: String,
    },
}

/// One structured SQL schema validation failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SchemaCheckFailure {
    /// SQL source could not be segmented safely.
    Segmentation {
        /// One-based source line when available.
        line: Option<usize>,
        /// One-based source column when available.
        column: Option<usize>,
        /// Segmentation diagnostic.
        message: String,
    },
    /// One segmented statement could not be prepared by the target database.
    Statement {
        /// One-based statement ordinal in the input.
        ordinal: usize,
        /// One-based source line where the segment begins.
        line: usize,
        /// One-based source column where the segment begins.
        column: usize,
        /// Driver-provided preparation diagnostic.
        message: String,
    },
}

/// Result of planning or applying one-off repair SQL.
#[derive(Debug, Clone, Serialize)]
pub struct RepairReport {
    /// Verification report before a dry-run or after an applied repair.
    pub verification: VerificationReport,
    /// Repair operations selected from verified findings.
    pub operations: Vec<Operation>,
    /// SQL rendered from repair operations.
    pub sql: Vec<String>,
    /// Whether SQL was applied to the target database.
    pub applied: bool,
    /// Findings that were intentionally left for manual handling.
    pub skipped_findings: Vec<DriftFinding>,
}
