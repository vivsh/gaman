//! Neutral root-entity filter model shared by planning and host adapters.

use serde::{Deserialize, Serialize};

use crate::states::EntityKind;

/// A root entity kind and glob used to select canonical schema identities.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EntityFilter {
    /// Root entity kind selected by this filter.
    pub kind: EntityKind,
    /// Glob matched against canonical qualified identities.
    pub pattern: String,
}
