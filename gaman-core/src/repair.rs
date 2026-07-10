//! Drift repair planning without database I/O.
//!
//! Repair planning is the recovery counterpart to semantic drift detection. It
//! consumes a `VerificationReport`, keeps the operations that can repair the
//! verified drift, and reports findings that still require manual handling.

use crate::drift::{DriftFinding, VerificationReport};
use crate::operations::Operation;

/// Database-I/O-free repair plan derived from a drift verification report.
#[derive(Debug, Clone, Default)]
pub struct RepairPlan {
    /// Operations that can be rendered and applied as one-off repair SQL.
    pub operations: Vec<Operation>,
    /// Drift findings not represented by repair operations.
    pub skipped_findings: Vec<DriftFinding>,
}

/// Builds a one-off repair plan from verified drift.
///
/// This function does not infer new repairs. It trusts the drift engine's
/// operation projection and marks all findings as skipped when no repair
/// operation is available.
pub fn plan_repair(report: &VerificationReport) -> RepairPlan {
    if report.findings.is_empty() {
        return RepairPlan::default();
    }
    if report.operations.is_empty() {
        return RepairPlan {
            operations: Vec::new(),
            skipped_findings: report.findings.clone(),
        };
    }
    RepairPlan {
        operations: report.operations.clone(),
        skipped_findings: Vec::new(),
    }
}
