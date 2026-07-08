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
use gaman_core::dialects::Dialect;
use gaman_core::states::{Schema, SchemaValidationError};

/// Errors returned while reflecting a live database into schema IR.
#[derive(Debug, Error)]
pub enum InspectionError {
    /// The backend failed while reading catalog metadata.
    #[error("database inspection failed: {0}")]
    Executor(#[from] ExecutorError),
    /// The reflected schema cannot be prepared for the selected dialect.
    #[error("inspected schema preparation failed: {0}")]
    Dialect(#[from] SchemaValidationError),
}

/// Reflects live database catalogs into a dialect-prepared `Schema`.
///
/// This is the shared live inspection path used by both `inspect-db` and the
/// live side of `verify-db`. It does not apply verification projection; callers
/// that compare drift must use the verification module after inspection.
pub(crate) async fn inspect_database(
    executor: &mut (dyn EnvironmentExecutor + Send),
    schemas: &[&str],
    dialect: Dialect,
) -> Result<Schema, InspectionError> {
    let schema = executor.inspect_db(schemas).await?;
    Ok(prepare_inspected_schema(schema, dialect)?)
}

/// Prepares reflected schema without dropping onboarding metadata.
///
/// Inspection preparation is limited to the normal dialect lifecycle. It should
/// not remove opaque bodies, unverified defaults, or other useful catalog data;
/// verification owns those comparison-specific choices.
pub(crate) fn prepare_inspected_schema(
    schema: Schema,
    dialect: Dialect,
) -> Result<Schema, SchemaValidationError> {
    schema.prepare(dialect)
}

#[cfg(test)]
mod tests {
    use gaman_core::dialects::Dialect;
    use gaman_core::states::{Column, FunctionDef, Schema, Table, Volatility};

    use super::prepare_inspected_schema;

    /// Verifies inspection preparation preserves opaque source used for onboarding exports.
    #[test]
    fn prepare_inspected_schema_preserves_function_body() {
        let mut schema = Schema::default();
        schema.functions.insert(
            "audit_users".to_string(),
            FunctionDef {
                name: "audit_users".to_string(),
                schema: None,
                arguments: String::new(),
                returns: "trigger".to_string(),
                language: "plpgsql".to_string(),
                body: "BEGIN RETURN NEW; END".to_string(),
                volatility: Volatility::Volatile,
                security_definer: false,
            },
        );

        let inspected = prepare_inspected_schema(schema, Dialect::Postgres).unwrap();

        assert_eq!(
            inspected
                .functions
                .values()
                .next()
                .map(|function| function.body.as_str()),
            Some("BEGIN RETURN NEW; END")
        );
    }

    /// Verifies inspection preparation keeps reflected defaults for schema onboarding.
    #[test]
    fn prepare_inspected_schema_preserves_column_defaults() {
        let mut schema = Schema::default();
        schema.tables.insert(
            "users".to_string(),
            Table {
                name: "users".to_string(),
                schema: None,
                primary_key: None,
                columns: vec![Column {
                    name: "status".to_string(),
                    col_type: "text".to_string(),
                    default: Some("'active'::text".to_string()),
                    ..Default::default()
                }],
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                triggers: Vec::new(),
            },
        );

        let inspected = prepare_inspected_schema(schema, Dialect::Postgres).unwrap();

        assert_eq!(
            inspected.tables["users"].columns[0].default.as_deref(),
            Some("'active'::text")
        );
    }
}
