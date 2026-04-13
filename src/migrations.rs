use serde::{Deserialize, Serialize};

use crate::operations::Operation;

fn bool_true() -> bool {
    true
}

/// A single migration: an ordered, dependency-aware set of operations.
/// `id` is derived from the filename at load time and is not stored in the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    #[serde(skip)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    pub operations: Vec<Operation>,
    /// When false, this migration runs outside a transaction. Required for
    /// operations that cannot run inside a transaction (e.g. CREATE INDEX CONCURRENTLY).
    /// Defaults to true so existing migration files are unaffected.
    #[serde(default = "bool_true", skip_serializing_if = "std::ops::Not::not")]
    pub atomic: bool,
}
