use serde::{Deserialize, Serialize};

use crate::operations::Operation;

/// A single migration: an ordered, dependency-aware set of operations.
/// `id` is derived from the filename at load time and is not stored in the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    #[serde(skip)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    pub operations: Vec<Operation>,
}
