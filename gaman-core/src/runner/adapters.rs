use thiserror::Error;

use crate::migration_engine::{BoxFuture, ExecutorError};
use crate::states::Schema;
/// Reflects high-fidelity schema state from a live target without defining drift policy.
pub trait SchemaInspector: Send {
    /// Reads requested namespaces from the target database.
    fn inspect<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>>;
}

impl<T> SchemaInspector for &mut T
where
    T: SchemaInspector + ?Sized,
{
    fn inspect<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        (**self).inspect(schemas)
    }
}

/// Failure returned by a host-provided schema inspector.
#[derive(Debug, Error)]
pub enum InspectionError {
    /// The selected host cannot inspect live catalog state.
    #[error("inspection is unavailable for {dialect}")]
    Unavailable { dialect: String },
    /// A catalog query or interpretation step failed.
    #[error("database inspection failed: {message}")]
    Query { message: String },
    /// Reflected catalog state violated a required schema invariant.
    #[error("invalid inspected catalog state: {message}")]
    InvalidCatalogState { message: String },
}

impl InspectionError {
    /// Creates a catalog-query failure from host adapter context.
    pub fn query(message: impl Into<String>) -> Self {
        Self::Query {
            message: message.into(),
        }
    }
}

impl From<ExecutorError> for InspectionError {
    /// Converts a catalog adapter failure into the runner's inspection boundary error.
    fn from(error: ExecutorError) -> Self {
        Self::Query {
            message: error.to_string(),
        }
    }
}
