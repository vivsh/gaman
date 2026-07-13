use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::adapters::InspectionError;
use super::protocol::COMMAND_PROTOCOL_VERSION;
use crate::clarifier::Clarification;
use crate::migration_engine::{EngineError, ExecutorError, StoreError, TrackingError};
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
            Self::UnsupportedProtocolVersion { expected, observed } => CommandDiagnostic {
                code: DiagnosticCode::UnsupportedProtocolVersion,
                summary: format!(
                    "unsupported command protocol version {observed}; expected {expected}"
                ),
                hint: Some("update the host binding and retry the command".to_string()),
                details: Vec::new(),
                retryable: false,
            },
            Self::NeedsInput(_) | Self::Migration(EngineError::NeedsInput(_)) => {
                CommandDiagnostic {
                    code: DiagnosticCode::ClarificationRequired,
                    summary: "migration generation needs clarification input".to_string(),
                    hint: Some("supply decisions and run the command again".to_string()),
                    details: Vec::new(),
                    retryable: true,
                }
            }
            Self::Invalid(message) => CommandDiagnostic {
                code: DiagnosticCode::InvalidCommand,
                summary: message.clone(),
                hint: None,
                details: Vec::new(),
                retryable: false,
            },
            Self::Inspection(error) => CommandDiagnostic {
                code: DiagnosticCode::InspectionFailed,
                summary: error.to_string(),
                hint: Some("check database connectivity and selected namespaces".to_string()),
                details: Vec::new(),
                retryable: true,
            },
            Self::Store(error) => diagnostic(DiagnosticCode::MigrationStoreFailed, error),
            Self::Tracking(error) => diagnostic(DiagnosticCode::TrackingFailed, error),
            Self::Execution(error) => diagnostic(DiagnosticCode::ExecutionFailed, error),
            Self::Parse(error) => diagnostic(DiagnosticCode::ParseFailed, error),
            Self::Migration(error) => diagnostic(DiagnosticCode::MigrationFailed, error),
        }
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

fn diagnostic(code: DiagnosticCode, error: &impl std::fmt::Display) -> CommandDiagnostic {
    CommandDiagnostic {
        code,
        summary: error.to_string(),
        hint: None,
        details: Vec::new(),
        retryable: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies protocol mismatches use the frozen protocol-v2 diagnostic contract.
    #[test]
    fn protocol_mismatch_is_structured() {
        let failure = CommandError::UnsupportedProtocolVersion {
            expected: COMMAND_PROTOCOL_VERSION,
            observed: 1,
        }
        .failure();

        assert_eq!(failure.protocol_version, 2);
        assert_eq!(
            failure.diagnostic.code,
            DiagnosticCode::UnsupportedProtocolVersion
        );
        assert!(!failure.diagnostic.retryable);
    }
}
