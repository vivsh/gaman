use crate::dialects::{DialectError, DialectProcessor};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{Schema, SchemaValidationError};

pub(super) static MYSQL: MysqlProcessor = MysqlProcessor;

pub(super) struct MysqlProcessor;

impl DialectProcessor for MysqlProcessor {
    fn migration_to_sql(
        &self,
        _migration: &Migration,
        _start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        Err(unsupported())
    }

    fn finalize_diff_operations(
        &self,
        ops: Vec<Operation>,
        _previous: &Schema,
        _current: &Schema,
    ) -> Vec<Operation> {
        ops
    }

    fn should_merge(&self, _table_name: &str, _op: &Operation) -> bool {
        false
    }

    fn canonicalize_schema_name(
        &self,
        _object: EntityKind,
        schema: Option<&str>,
    ) -> Option<String> {
        schema.map(str::to_string)
    }

    fn normalize_type<'a>(&self, t: &'a str) -> &'a str {
        t
    }

    fn canonical_type(&self, t: &str) -> String {
        t.to_string()
    }

    fn is_catalog_type(&self, _t: &str) -> bool {
        false
    }

    fn type_suggestions(&self, _t: &str) -> Vec<String> {
        Vec::new()
    }

    fn validate_schema(&self, _schema: &Schema) -> Result<(), SchemaValidationError> {
        Err(SchemaValidationError::Invalid(
            "MySQL dialect is not implemented".to_string(),
        ))
    }

    fn drift_registry(&self) -> &'static crate::drift::DriftRegistry {
        crate::drift::mysql::registry()
    }

    fn normalize_inspected_schema(&self, schema: Schema) -> Result<Schema, SchemaValidationError> {
        schema.prepare(crate::dialects::Dialect::Mysql)
    }

    fn validate_migration(&self, _migration: &Migration) -> Result<(), DialectError> {
        Err(unsupported())
    }

    fn validate_migration_with_state(
        &self,
        _migration: &Migration,
        _start: &Schema,
    ) -> Result<(), DialectError> {
        Err(unsupported())
    }
}

fn unsupported() -> DialectError {
    DialectError::Unsupported(
        "mysql".to_string(),
        "MySQL dialect is not implemented".to_string(),
    )
}
