//! MySQL 8.4 schema semantics and SQL rendering.

mod data_types;
pub(super) mod type_compare;

use crate::dialects::{DialectError, DialectProcessor};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::parsers::tokens::{MYSQL_TOKENIZER, SqlTokenizer};
use crate::states::types::EntityKind;
use crate::states::{Column, Schema, SchemaValidationError};

use super::mysql_family::{self, FamilyFlavor};

pub(super) static MYSQL: MysqlProcessor = MysqlProcessor;

pub(super) struct MysqlProcessor;

impl DialectProcessor for MysqlProcessor {
    fn tokenizer(&self) -> &'static dyn SqlTokenizer {
        &MYSQL_TOKENIZER
    }

    fn migration_to_sql(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        mysql_family::migration_to_sql(FamilyFlavor::Mysql, migration, start)
    }

    fn finalize_diff_operations(
        &self,
        ops: Vec<Operation>,
        _: &Schema,
        _: &Schema,
    ) -> Vec<Operation> {
        ops
    }

    fn should_merge(&self, _: &str, op: &Operation) -> bool {
        matches!(
            op,
            Operation::AddForeignKey { .. } | Operation::AddConstraint { .. }
        )
    }

    fn canonicalize_schema_name(&self, _: EntityKind, schema: Option<&str>) -> Option<String> {
        schema.map(str::to_string)
    }

    fn normalize_type<'a>(&self, value: &'a str) -> &'a str {
        value.trim()
    }

    fn canonical_type(&self, value: &str) -> String {
        type_compare::canonical(value)
    }

    fn type_comparison_key(&self, value: &str) -> String {
        type_compare::key(value)
    }

    fn is_catalog_type(&self, value: &str) -> bool {
        data_types::contains(value)
    }

    fn type_suggestions(&self, value: &str) -> Vec<String> {
        data_types::suggestions(value)
    }

    fn validate_schema(&self, schema: &Schema) -> Result<(), SchemaValidationError> {
        mysql_family::validate_schema(schema, FamilyFlavor::Mysql)
    }

    fn validate_migration(&self, migration: &Migration) -> Result<(), DialectError> {
        mysql_family::validate_migration(migration, FamilyFlavor::Mysql)
    }

    fn validate_migration_with_state(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<(), DialectError> {
        mysql_family::validate_migration_with_state(migration, start, FamilyFlavor::Mysql)
    }

    fn drift_registry(&self) -> &'static crate::drift::DriftRegistry {
        crate::drift::mysql::registry()
    }

    fn normalize_inspected_schema(&self, schema: Schema) -> Result<Schema, SchemaValidationError> {
        schema.prepare(crate::dialects::Dialect::Mysql)
    }

    fn column_for_repair(&self, expected: &Column, observed: &Column) -> Column {
        mysql_family::column_for_repair(expected, observed, FamilyFlavor::Mysql)
    }

    fn supports_transactional_ddl(&self) -> bool {
        false
    }

    fn tracking_install_sql(&self, table: &str) -> Option<Vec<String>> {
        Some(vec![format!(
            "CREATE TABLE IF NOT EXISTS `{}` (id VARCHAR(255) PRIMARY KEY, applied_at TIMESTAMP(6) NOT NULL DEFAULT CURRENT_TIMESTAMP(6))",
            table.replace('`', "``")
        )])
    }

    fn tracking_list_sql(&self, table: &str) -> Option<String> {
        Some(format!(
            "SELECT id FROM `{}` ORDER BY applied_at, id",
            table.replace('`', "``")
        ))
    }

    fn tracking_record_sql(&self, table: &str, id: &str) -> Option<String> {
        Some(format!(
            "INSERT INTO `{}` (id) VALUES ('{}')",
            table.replace('`', "``"),
            id.replace('\'', "''")
        ))
    }

    fn tracking_unrecord_sql(&self, table: &str, id: &str) -> Option<String> {
        Some(format!(
            "DELETE FROM `{}` WHERE id = '{}'",
            table.replace('`', "``"),
            id.replace('\'', "''")
        ))
    }
}
