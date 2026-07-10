//! Structured semantic drift results shared by verification, repair, and CLI reporting.

use crate::operations::Operation;
use crate::states::EntityKind;

/// Result of comparing replayed migration state with inspected database state.
#[derive(Debug, Clone, Default)]
pub struct VerificationReport {
    /// Property and presence differences found in migration-owned entities.
    pub findings: Vec<DriftFinding>,
    /// Coarse repair operations that can restore registered expected state.
    pub operations: Vec<Operation>,
    /// Migration ids that must normally be applied before drift can be repaired.
    pub pending_migrations: Vec<String>,
}

/// One actionable entity-property difference reported by semantic drift.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftFinding {
    /// Repair operation category associated with this difference.
    pub operation: &'static str,
    /// Gaman entity kind containing the differing property.
    pub entity_kind: EntityKind,
    /// Stable qualified identity used to locate the entity.
    pub entity_name: String,
    /// Registered property that failed semantic comparison.
    pub property: &'static str,
    /// Value expected from replayed migration state.
    pub expected: String,
    /// Value observed from live database inspection.
    pub observed: String,
    /// Optional explanation of comparison limits or canonicalization.
    pub note: Option<String>,
}
