use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::operations::Operation;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Fatal,
    Warning,
    Suggestion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClarificationKind {
    RenameTable {
        old: String,
        candidates: Vec<String>,
    },
    RenameColumn {
        table: String,
        old: String,
        candidates: Vec<String>,
    },
    RenameEnumValue {
        enum_name: String,
        old: String,
        candidates: Vec<String>,
    },
    NotNullAdd {
        table: String,
        column: String,
        col_type: String,
    },
    NotNullChange {
        table: String,
        column: String,
    },
    TypeCast {
        table: String,
        column: String,
        from: String,
        to: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clarification {
    pub id: String,
    pub severity: Severity,
    pub kind: ClarificationKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Answer {
    RenameTo(String),
    RenameNo,
    NotNullDefault(String),
    NotNullNullable,
    NotNullManual,
    TypeCast(String),
    TypeCastImplicit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub clarification_id: String,
    pub answer: Answer,
}

#[derive(Debug, Clone)]
pub enum DisambiguationResult {
    NeedsInput(Vec<Clarification>),
    Resolved(Vec<Operation>),
}

#[derive(Debug, Error, PartialEq)]
pub enum DisambiguatorError {
    #[error("decision references unknown clarification '{0}'")]
    UnknownDecision(String),
    #[error("decision for '{id}' has an answer incompatible with the clarification kind")]
    InvalidAnswer { id: String },
    #[error("decision for '{id}' chose '{chosen}' which is not a valid candidate")]
    InvalidCandidate { id: String, chosen: String },
    #[error("duplicate decision for clarification '{0}'")]
    DuplicateDecision(String),
    #[error("multiple rename decisions target '{target}' in {scope}")]
    ConflictingRenameTarget { scope: String, target: String },
    #[error("decision for '{id}' requires a non-empty value")]
    EmptyInput { id: String },
}

#[derive(Debug, Error)]
pub enum PromptError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    #[allow(dead_code)]
    Other(String),
}

pub trait PromptEngine {
    fn prompt(&self, clarifications: &[Clarification]) -> Result<Vec<Decision>, PromptError>;
}
