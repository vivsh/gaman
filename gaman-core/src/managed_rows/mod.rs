//! Top-level managed-row state, validation, migration planning, and SQL rendering.

mod diff;
pub mod drift;
mod execution;
mod model;
mod replay;
pub mod sql;
mod update;
mod validation;

pub(crate) use diff::{diff_schemas, order_operations};
#[cfg(test)]
pub(crate) use execution::ensure_one_affected;
pub(crate) use execution::execute_checked;
pub use model::{ManagedRow, ManagedRows, ManagedValue};
pub(crate) use replay::apply_operation;
pub(crate) use validation::canonicalize_schema;
#[doc(hidden)]
pub use validation::merge_declaration;
pub(crate) use validation::validate_schema;

#[cfg(test)]
mod production_tests;
#[cfg(test)]
mod roundtrip_tests;
#[cfg(test)]
mod tests;
