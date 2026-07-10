//! Live catalog reflection for onboarding and database inspection.
//!
//! Inspection turns database catalog metadata into Gaman `Schema` IR with the
//! highest fidelity the selected backend can provide. It is intentionally
//! separate from verification: `inspect-db` should preserve useful reflected
//! state for onboarding, even when some properties are too lossy or unstable to
//! drive drift detection.

use thiserror::Error;

use crate::environment::EnvironmentExecutor;
use crate::executor::ExecutorError;
use gaman_core::states::Schema;

/// Errors returned while reflecting a live database into schema IR.
#[derive(Debug, Error)]
pub enum InspectionError {
    /// The backend failed while reading catalog metadata.
    #[error("database inspection failed: {0}")]
    Executor(#[from] ExecutorError),
}

/// Reflects live database catalogs into a high-fidelity `Schema`.
///
/// This is the shared live inspection path used by both `inspect-db` and the
/// live side of `verify-db`. It does not apply semantic drift normalization;
/// callers that compare drift must use the core drift module after inspection.
pub(crate) async fn inspect_database(
    executor: &mut (dyn EnvironmentExecutor + Send),
    schemas: &[&str],
) -> Result<Schema, InspectionError> {
    Ok(executor.inspect_db(schemas).await?)
}
