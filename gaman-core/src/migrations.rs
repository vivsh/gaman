use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::operations::Operation;
use crate::states::types::EntityKind;

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

impl Migration {
    pub fn from_yaml_str(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }

    pub fn to_yaml_string(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }

    /// Returns the set of top-level entities touched by this migration.
    /// Includes FK target tables so callers can detect cross-namespace references.
    pub fn get_entities(&self) -> HashSet<(EntityKind, String)> {
        self.operations
            .iter()
            .flat_map(Operation::touched_entities)
            .collect()
    }
}
