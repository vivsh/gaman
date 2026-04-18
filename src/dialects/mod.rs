use thiserror::Error;

use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::Schema;

#[derive(Debug, Error)]
pub enum DialectError {
    #[error("unsupported operation '{0}': {1}")]
    Unsupported(String, String),
}

pub enum Dialect {
    Postgres,
}

impl Dialect {
    pub fn operation_to_sql(&self, op: &Operation) -> Result<Vec<String>, DialectError> {
        match self {
            Dialect::Postgres => postgres::operation_to_sql(op),
        }
    }

    /// Reorders operations to satisfy database-specific execution constraints.
    /// The default is a no-op — only databases with ordering requirements need to override.
    /// Called once per migration after diffing, before SQL generation or writing.
    pub fn reorder(&self, ops: Vec<Operation>, previous: &Schema, current: &Schema) -> Vec<Operation> {
        match self {
            Dialect::Postgres => postgres::reorder_ops(ops, previous, current),
        }
    }

    /// Whether a decomposed sub-entity op should be folded back into its
    /// parent CreateTable. Postgres can inline everything; other dialects
    /// (e.g. SQLite) may need FKs kept inline while indexes stay separate.
    pub fn should_merge(&self, _table_name: &str, _op: &Operation) -> bool {
        match self {
            Dialect::Postgres => true,
        }
    }

    /// Returns the DDL statements to bootstrap the migration tracking table.
    /// Uses CREATE TABLE IF NOT EXISTS so it is safe to call repeatedly.
    pub fn create_tracking_table_sql(&self) -> Vec<String> {
        match self {
            Dialect::Postgres => postgres::create_tracking_table_sql(),
        }
    }

    /// SQL to fetch all applied migration ids in application order.
    pub fn applied_migrations_sql(&self) -> &'static str {
        "SELECT id FROM gaman_migrations ORDER BY applied_at, id"
    }

    /// SQL to record a migration id as applied without running its operations.
    pub fn record_sql(&self, id: &str) -> String {
        let escaped = id.replace('\'', "''");
        format!("INSERT INTO gaman_migrations (id) VALUES ('{escaped}')")
    }

    /// SQL to remove a migration id from the tracking table.
    pub fn unrecord_sql(&self, id: &str) -> String {
        let escaped = id.replace('\'', "''");
        format!("DELETE FROM gaman_migrations WHERE id = '{escaped}'")
    }

    pub fn normalize_type<'a>(&self, t: &'a str) -> &'a str {
        match self {
            Dialect::Postgres => postgres::normalize_type(t),
        }
    }

    pub fn validate_migration(&self, m: &Migration) -> Result<(), DialectError> {
        match self {
            Dialect::Postgres => postgres::validate_migration(m),
        }
    }
}

mod postgres;
