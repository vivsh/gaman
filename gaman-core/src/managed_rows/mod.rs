//! Top-level managed-row state, validation, migration planning, and SQL rendering.

mod diff;
pub mod drift;
mod model;
mod replay;
pub mod sql;
mod validation;

pub(crate) use diff::{diff_schemas, order_operations};
pub use model::{ManagedRow, ManagedRows, ManagedValue};
pub(crate) use replay::apply_operation;
pub(crate) use validation::canonicalize_schema;
#[doc(hidden)]
pub use validation::merge_declaration;
pub(crate) use validation::validate_schema;

pub(crate) fn ensure_one_affected(
    affected: u64,
) -> Result<(), crate::migration_engine::ExecutorError> {
    match affected {
        1 => Ok(()),
        0 => Err(crate::migration_engine::ExecutorError::Execute(
            "managed row expected-state precondition did not match".to_string(),
        )),
        affected => Err(crate::migration_engine::ExecutorError::Execute(format!(
            "managed row integrity failure: expected one affected row, got {affected}"
        ))),
    }
}

#[cfg(test)]
mod production_tests;
#[cfg(test)]
mod roundtrip_tests;
#[cfg(test)]
mod tests;
