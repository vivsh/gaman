#![allow(clippy::result_large_err)]

mod canonicalize;
mod input;
mod load;
pub mod names;
mod normalize;
mod partitioning;
mod raw_validation;
mod replay;
mod validate;
pub(crate) use partitioning::{
    reject_postgres_range_partitioning, validate_postgres_range_partitioning,
};
pub(crate) use raw_validation::{parse_qualified_name, validate_authored_raw};
pub(crate) use validate::reject_family_column_options;

pub mod builder;
pub mod errors;
pub mod types;

pub use builder::*;
pub use errors::*;
pub use input::*;
pub use types::*;

#[cfg(test)]
mod function_tests;
#[cfg(test)]
mod load_tests;
#[cfg(test)]
mod tests;
