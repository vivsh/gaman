//! Exact-one-row enforcement for managed writes and read-only ownership transitions.

use crate::migration_engine::{Executor, ExecutorError};
use crate::operations::Operation;

/// Checks ownership-only transitions without firing update triggers; writes retain affected counts.
pub(crate) async fn execute_checked<E: Executor>(
    executor: &mut E,
    operation: &Operation,
    statement: &str,
) -> Result<(), ExecutorError> {
    let count = if matches!(operation, Operation::UpdateRow { old, new, .. }
        if super::update::removes_only(old, new))
    {
        executor.fetch_strings(statement).await?.len() as u64
    } else {
        executor.execute_affected(statement).await?
    };
    ensure_one_affected(count)
}

/// Rejects absent/stale rows and broken uniqueness for both writes and ownership guards.
pub(crate) fn ensure_one_affected(affected: u64) -> Result<(), ExecutorError> {
    match affected {
        1 => Ok(()),
        0 => Err(ExecutorError::Execute(
            "managed row expected-state precondition did not match".to_string(),
        )),
        affected => Err(ExecutorError::Execute(format!(
            "managed row integrity failure: expected one affected row, got {affected}"
        ))),
    }
}
