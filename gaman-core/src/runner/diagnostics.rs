use std::error::Error as StdError;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::adapters::InspectionError;
use super::protocol::COMMAND_PROTOCOL_VERSION;
use crate::clarifier::Clarification;
use crate::graphs::GraphError;
use crate::migration_engine::{EngineError, ExecutorError, StoreError, TrackingError};
use crate::offline_planner::OfflineError;
use crate::redact_diagnostic_text;
use crate::sql_plan::SqlPlanError;
use crate::states::ReplayError;
/// Structured semantic or host-adapter failure from runner command execution.
#[derive(Debug, Error)]
pub enum CommandError {
    /// The host sent a command envelope using an unsupported protocol version.
    #[error("unsupported command protocol version {observed}; expected {expected}")]
    UnsupportedProtocolVersion {
        /// Protocol version accepted by this build.
        expected: u32,
        /// Protocol version supplied by the host.
        observed: u32,
    },
    /// Migration generation needs explicit caller-provided clarification decisions.
    #[error("migration generation needs clarification input")]
    NeedsInput(Vec<Clarification>),
    /// Migration lifecycle execution failed.
    #[error(transparent)]
    Migration(EngineError),
    /// Migration storage failed before lifecycle execution completed.
    #[error(transparent)]
    Store(StoreError),
    /// Applied-state tracking failed.
    #[error(transparent)]
    Tracking(TrackingError),
    /// SQL preparation or execution failed.
    #[error(transparent)]
    Execution(ExecutorError),
    /// SQL segmentation failed before database preparation.
    #[error("SQL parsing failed: {0}")]
    Parse(String),
    /// Live catalog inspection failed.
    #[error(transparent)]
    Inspection(#[from] InspectionError),
    /// The host supplied an invalid resolved command.
    #[error("invalid command: {0}")]
    Invalid(String),
}

impl From<EngineError> for CommandError {
    /// Preserves clarification suspension as a first-class runner error.
    fn from(error: EngineError) -> Self {
        match error {
            EngineError::NeedsInput(clarifications) => Self::NeedsInput(clarifications),
            EngineError::Store(error) => Self::Store(error),
            EngineError::Tracking(error) => Self::Tracking(error),
            EngineError::Executor(error) => Self::Execution(error),
            error => Self::Migration(error),
        }
    }
}

/// Stable command failure categories shared by non-Rust hosts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
    /// A transport consumer supplied an unsupported command protocol version.
    UnsupportedProtocolVersion,
    /// Migration generation requires explicit decisions.
    ClarificationRequired,
    /// The resolved command violates command-level constraints.
    InvalidCommand,
    /// Migration storage failed.
    MigrationStoreFailed,
    /// Applied-state tracking failed.
    TrackingFailed,
    /// SQL preparation or execution failed.
    ExecutionFailed,
    /// Live catalog inspection failed.
    InspectionFailed,
    /// Offline migration lifecycle work failed.
    MigrationFailed,
    /// SQL segmentation failed.
    ParseFailed,
}

/// Serializable host-facing diagnostic derived from one [`CommandError`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandDiagnostic {
    /// Stable diagnostic category.
    pub code: DiagnosticCode,
    /// Concise user-facing summary.
    pub summary: String,
    /// Optional actionable hint.
    pub hint: Option<String>,
    /// Additional concise diagnostic context.
    pub details: Vec<String>,
    /// Whether supplying new host input can make the same request succeed.
    pub retryable: bool,
}

/// Versioned portable command failure with optional clarification payload.
#[derive(Debug, Clone, Serialize)]
pub struct CommandFailure {
    /// Protocol version used to encode the failure.
    pub protocol_version: u32,
    /// Stable human and machine-readable diagnostic.
    pub diagnostic: CommandDiagnostic,
    /// Decisions requested from the caller, when applicable.
    pub clarifications: Vec<Clarification>,
}

impl CommandError {
    /// Returns concise shared diagnostic language without discarding the typed error.
    pub fn diagnostic(&self) -> CommandDiagnostic {
        match self {
            Self::UnsupportedProtocolVersion { expected, observed } => command_diagnostic(
                DiagnosticCode::UnsupportedProtocolVersion,
                format!("unsupported command protocol version {observed}; expected {expected}"),
                Some("update the host binding and retry the command".to_string()),
                Vec::new(),
                false,
            ),
            Self::NeedsInput(_) | Self::Migration(EngineError::NeedsInput(_)) => {
                command_diagnostic(
                    DiagnosticCode::ClarificationRequired,
                    "migration generation needs clarification input".to_string(),
                    Some("supply decisions and run the command again".to_string()),
                    Vec::new(),
                    true,
                )
            }
            Self::Invalid(message) => command_diagnostic(
                DiagnosticCode::InvalidCommand,
                message.clone(),
                None,
                Vec::new(),
                false,
            ),
            Self::Inspection(error) => inspection_diagnostic(error),
            Self::Store(error) => adapter_diagnostic(
                DiagnosticCode::MigrationStoreFailed,
                "migration storage failed",
                error,
                "check the migration directory and resolve any conflicting migration id",
            ),
            Self::Tracking(error) => adapter_diagnostic(
                DiagnosticCode::TrackingFailed,
                "migration tracking failed",
                error,
                "check database connectivity, tracking-table permissions, and migration lock state",
            ),
            Self::Execution(error) => adapter_diagnostic(
                DiagnosticCode::ExecutionFailed,
                "database operation failed",
                error,
                "check database connectivity, permissions, and the reported database error",
            ),
            Self::Parse(error) => diagnostic_with_detail(
                DiagnosticCode::ParseFailed,
                "SQL parsing failed",
                error,
                "correct the SQL input and retry the command",
                false,
            ),
            Self::Migration(error) => migration_diagnostic(error),
        }
    }

    /// Returns sanitized internal causes for host-specific verbose diagnostics.
    pub fn verbose_causes(&self) -> Vec<String> {
        let mut causes = Vec::new();
        let mut current: &(dyn StdError + 'static) = self;
        while let Some(source) = current.source() {
            let rendered = redact_diagnostic_text(&source.to_string());
            if causes.last() != Some(&rendered) {
                causes.push(rendered);
            }
            current = source;
        }
        if causes.is_empty() {
            causes.push(redact_diagnostic_text(&self.to_string()));
        }
        causes
    }

    /// Returns a serializable failure preserving typed clarification requests.
    pub fn failure(&self) -> CommandFailure {
        let clarifications = match self {
            Self::NeedsInput(clarifications)
            | Self::Migration(EngineError::NeedsInput(clarifications)) => clarifications.clone(),
            _ => Vec::new(),
        };
        CommandFailure {
            protocol_version: COMMAND_PROTOCOL_VERSION,
            diagnostic: self.diagnostic(),
            clarifications,
        }
    }
}

/// Projects one migration-engine error without discarding nested lifecycle context.
fn migration_diagnostic(error: &EngineError) -> CommandDiagnostic {
    match error {
        EngineError::MigrationExecution {
            migration,
            direction,
            statement_ordinal,
            statement,
            source,
        } => execution_diagnostic(migration, direction, *statement_ordinal, statement, source),
        EngineError::Graph(error) => graph_diagnostic(error),
        EngineError::Offline(error) => offline_diagnostic(error),
        EngineError::SqlPlan(error) => sql_plan_diagnostic(error),
        EngineError::Config(message) => diagnostic_with_detail(
            DiagnosticCode::MigrationFailed,
            "migration command cannot run",
            message,
            "correct the migration configuration and retry the command",
            false,
        ),
        EngineError::NeedsInput(_) => command_diagnostic(
            DiagnosticCode::ClarificationRequired,
            "migration generation needs clarification input".to_string(),
            Some("supply decisions and run the command again".to_string()),
            Vec::new(),
            true,
        ),
        EngineError::Store(error) => adapter_diagnostic(
            DiagnosticCode::MigrationStoreFailed,
            "migration storage failed",
            error,
            "check the migration directory and resolve any conflicting migration id",
        ),
        EngineError::Tracking(error) => adapter_diagnostic(
            DiagnosticCode::TrackingFailed,
            "migration tracking failed",
            error,
            "check database connectivity, tracking-table permissions, and migration lock state",
        ),
        EngineError::Executor(error) => adapter_diagnostic(
            DiagnosticCode::ExecutionFailed,
            "database operation failed",
            error,
            "check database connectivity, permissions, and the reported database error",
        ),
    }
}

/// Projects one rendered migration statement failure without exposing its complete SQL text.
fn execution_diagnostic(
    migration: &str,
    direction: &str,
    statement_ordinal: usize,
    statement: &crate::migration_engine::execution_diagnostic::StatementDiagnostic,
    source: &ExecutorError,
) -> CommandDiagnostic {
    let mut details = vec![
        format!("migration: {migration}"),
        format!(
            "{direction} statement {statement_ordinal}: {}",
            statement.signature
        ),
        database_failure_detail(source),
    ];
    if let Some(location) = &statement.location {
        details.push(format!(
            "at {} line {}, column {}",
            location.source.label(),
            location.line,
            location.column
        ));
        details.push(format!("  {}", location.excerpt));
        details.push(format!("  {}^", " ".repeat(location.caret_offset)));
    }
    command_diagnostic(
        DiagnosticCode::ExecutionFailed,
        "database operation failed".to_string(),
        Some(format!(
            "inspect migration '{migration}' with `gaman show {migration}`"
        )),
        details,
        false,
    )
}

/// Formats a stable database message without relying on driver display text conventions.
fn database_failure_detail(error: &ExecutorError) -> String {
    match error.database_failure() {
        Some(failure) => match &failure.code {
            Some(code) => format!("execute failed [{code}]: {}", failure.message),
            None => format!("execute failed: {}", failure.message),
        },
        None => error.to_string(),
    }
}

/// Projects planning failures by preserving the most actionable nested category.
fn offline_diagnostic(error: &OfflineError) -> CommandDiagnostic {
    match error {
        OfflineError::Graph(error) => graph_diagnostic(error),
        OfflineError::Replay(error) => replay_diagnostic(error),
        OfflineError::SqlPlan(error) => sql_plan_diagnostic(error),
        OfflineError::Schema(message) => diagnostic_with_detail(
            DiagnosticCode::MigrationFailed,
            "schema validation failed during migration planning",
            message,
            "correct the schema or migration history and retry the command",
            false,
        ),
        OfflineError::NeedsInput(_) => command_diagnostic(
            DiagnosticCode::ClarificationRequired,
            "migration generation needs clarification input".to_string(),
            Some("supply decisions and run the command again".to_string()),
            Vec::new(),
            true,
        ),
        _ => diagnostic_with_detail(
            DiagnosticCode::MigrationFailed,
            "migration planning failed",
            error,
            "correct the reported schema or migration definition and retry the command",
            false,
        ),
    }
}

/// Projects SQL planning failures while retaining replay and graph causes.
fn sql_plan_diagnostic(error: &SqlPlanError) -> CommandDiagnostic {
    match error {
        SqlPlanError::Graph(error) => graph_diagnostic(error),
        SqlPlanError::Replay(error) => replay_diagnostic(error),
        _ => diagnostic_with_detail(
            DiagnosticCode::MigrationFailed,
            "migration SQL planning failed",
            error,
            "correct the reported migration or use explicit SQL for unsupported changes",
            false,
        ),
    }
}

/// Projects replay errors with migration and operation context in normal output.
fn replay_diagnostic(error: &ReplayError) -> CommandDiagnostic {
    match error {
        ReplayError::WithContext {
            migration,
            op_num,
            operation,
            inner,
        } => command_diagnostic(
            DiagnosticCode::MigrationFailed,
            format!("cannot replay migration '{migration}'"),
            Some(replay_hint(inner)),
            vec![format!("operation {op_num} ({operation}): {}", inner)],
            false,
        ),
        _ => diagnostic_with_detail(
            DiagnosticCode::MigrationFailed,
            "cannot replay migration history",
            error,
            "correct the reported migration definition and retry the command",
            false,
        ),
    }
}

fn replay_hint(error: &ReplayError) -> String {
    match error {
        ReplayError::InvalidOpaqueCreate { .. } => {
            "replace the opaque source with one plain CREATE statement; Gaman owns existence and replacement"
                .to_string()
        }
        _ => "correct the migration order or restore the missing prerequisite migration".to_string(),
    }
}

/// Projects invalid migration-graph state into a corrective diagnostic.
fn graph_diagnostic(error: &GraphError) -> CommandDiagnostic {
    diagnostic_with_detail(
        DiagnosticCode::MigrationFailed,
        "migration graph is invalid",
        error,
        graph_hint(error),
        false,
    )
}

/// Returns the narrowest safe remediation for a migration-graph failure.
fn graph_hint(error: &GraphError) -> &'static str {
    match error {
        GraphError::DuplicateId(_) => "rename or remove the duplicate migration id",
        GraphError::CycleDetected => "remove the dependency cycle between migrations",
        GraphError::Conflict => "run make --merge after resolving the competing migration heads",
        GraphError::UnknownDependency { .. } => {
            "restore the referenced dependency or correct the migration dependencies"
        }
        GraphError::InvalidId(_) => "rename the migration using a valid lowercase id",
        GraphError::UnknownId(_) | GraphError::AmbiguousId { .. } => {
            "use one exact migration id or an unambiguous prefix"
        }
        GraphError::Empty => "create a migration before requesting this operation",
    }
}

/// Projects catalog inspection errors without exposing host-adapter internals in the summary.
fn inspection_diagnostic(error: &InspectionError) -> CommandDiagnostic {
    diagnostic_with_detail(
        DiagnosticCode::InspectionFailed,
        "database inspection failed",
        error,
        "check database connectivity, selected namespaces, and catalog permissions",
        true,
    )
}

/// Projects one host adapter failure with stable action wording and sanitized detail.
fn adapter_diagnostic(
    code: DiagnosticCode,
    summary: &str,
    error: &impl std::fmt::Display,
    hint: &str,
) -> CommandDiagnostic {
    diagnostic_with_detail(code, summary, error, hint, false)
}

/// Creates one protocol-v2 diagnostic while keeping sensitive adapter text out of public fields.
fn diagnostic_with_detail(
    code: DiagnosticCode,
    summary: &str,
    detail: &impl std::fmt::Display,
    hint: &str,
    retryable: bool,
) -> CommandDiagnostic {
    command_diagnostic(
        code,
        summary.to_string(),
        Some(hint.to_string()),
        vec![detail.to_string()],
        retryable,
    )
}

/// Constructs a diagnostic after redacting every potentially adapter-controlled field.
fn command_diagnostic(
    code: DiagnosticCode,
    summary: String,
    hint: Option<String>,
    details: Vec<String>,
    retryable: bool,
) -> CommandDiagnostic {
    CommandDiagnostic {
        code,
        summary: redact_diagnostic_text(&summary),
        hint: hint.map(|value| redact_diagnostic_text(&value)),
        details: details
            .into_iter()
            .map(|value| redact_diagnostic_text(&value))
            .collect(),
        retryable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies protocol mismatches use the current protocol-v4 diagnostic contract.
    #[test]
    fn protocol_mismatch_is_structured() {
        let failure = CommandError::UnsupportedProtocolVersion {
            expected: COMMAND_PROTOCOL_VERSION,
            observed: 1,
        }
        .failure();

        assert_eq!(failure.protocol_version, 4);
        assert_eq!(
            failure.diagnostic.code,
            DiagnosticCode::UnsupportedProtocolVersion
        );
        assert!(!failure.diagnostic.retryable);
    }

    /// Verifies replay diagnostics preserve migration, operation, and root-cause context.
    #[test]
    fn replay_failure_is_actionable() {
        let error = CommandError::Migration(EngineError::Offline(OfflineError::Replay(
            ReplayError::WithContext {
                migration: "0002_add_posts".to_string(),
                op_num: 2,
                operation: "add foreign key posts.author_id".to_string(),
                inner: Box::new(ReplayError::TableNotFound("posts".to_string())),
            },
        )));

        let diagnostic = error.diagnostic();
        assert_eq!(diagnostic.code, DiagnosticCode::MigrationFailed);
        assert_eq!(
            diagnostic.summary,
            "cannot replay migration '0002_add_posts'"
        );
        assert_eq!(
            diagnostic.details,
            ["operation 2 (add foreign key posts.author_id): table 'posts' not found"]
        );
        assert!(
            diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("migration order"))
        );
    }

    /// Verifies opaque migration failures explain the plain-CREATE remediation.
    #[test]
    fn opaque_create_replay_failure_has_specific_hint() {
        let error = CommandError::Migration(EngineError::Offline(OfflineError::Replay(
            ReplayError::WithContext {
                migration: "0003_active_users".to_string(),
                op_num: 1,
                operation: "create view active_users".to_string(),
                inner: Box::new(ReplayError::InvalidOpaqueCreate {
                    entity: "active_users".to_string(),
                    reason: "CREATE OR REPLACE is not accepted; Gaman owns replacement".to_string(),
                }),
            },
        )));

        let diagnostic = error.diagnostic();
        assert!(diagnostic.details[0].contains("CREATE OR REPLACE"));
        assert!(
            diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("plain CREATE"))
        );
    }

    /// Verifies adapter text cannot expose URL credentials or common secret assignments.
    #[test]
    fn diagnostics_redact_sensitive_adapter_text() {
        let error = CommandError::Execution(ExecutorError::Connect(
            "postgres://gaman:secret@localhost/app password=hunter2 token=abc".to_string(),
        ));

        let diagnostic = error.diagnostic();
        let detail = &diagnostic.details[0];
        assert!(!detail.contains("secret"));
        assert!(!detail.contains("hunter2"));
        assert!(!detail.contains("abc"));
        assert!(detail.contains("gaman:***@localhost"));
        let failure = serde_json::to_string(&error.failure()).expect("serialize command failure");
        assert!(!failure.contains("secret"));
        assert!(!error.verbose_causes().join(" ").contains("hunter2"));
    }

    /// Verifies migration execution diagnostics retain bounded statement context and SQLSTATE.
    #[test]
    fn migration_execution_failure_is_compact_and_actionable() {
        let error = CommandError::Migration(EngineError::MigrationExecution {
            migration: "0012_reports".to_string(),
            direction: "apply",
            statement_ordinal: 1,
            statement: Box::new(crate::migration_engine::execution_diagnostic::StatementDiagnostic {
                signature: "CREATE OR REPLACE FUNCTION dynrs_daily_report()".to_string(),
                location: Some(
                    crate::migration_engine::execution_diagnostic::StatementLocation {
                        source: crate::migration_engine::execution_diagnostic::StatementLocationSource::Internal,
                        line: 18,
                        column: 9,
                        excerpt: "SELECT session_type_provider_tip".to_string(),
                        caret_offset: 8,
                    },
                ),
            }),
            source: Box::new(ExecutorError::ExecuteDatabase(
                crate::migration_engine::DatabaseFailure::message(
                    "column reference is ambiguous",
                )
                .with_code("42702"),
            )),
        });
        let diagnostic = error.diagnostic();
        assert_eq!(diagnostic.summary, "database operation failed");
        assert!(
            diagnostic
                .details
                .iter()
                .any(|detail| detail == "migration: 0012_reports")
        );
        assert!(
            diagnostic.details.iter().any(|detail| {
                detail == "execute failed [42702]: column reference is ambiguous"
            })
        );
        assert!(
            diagnostic
                .hint
                .as_deref()
                .is_some_and(|hint| hint.contains("gaman show 0012_reports"))
        );
    }
}
