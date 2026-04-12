use thiserror::Error;

use crate::operations::Operation;

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
}

mod postgres;
pub use postgres::col_def;
