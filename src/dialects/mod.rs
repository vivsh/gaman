use thiserror::Error;

use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::Schema;

#[derive(Debug, Error)]
pub enum DialectError {
    #[error("unsupported operation '{0}': {1}")]
    Unsupported(String, String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl Dialect {
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("postgres") || value.eq_ignore_ascii_case("postgresql") {
            Some(Self::Postgres)
        } else if value.eq_ignore_ascii_case("sqlite") || value.eq_ignore_ascii_case("sqlite3") {
            #[cfg(feature = "sqlite")]
            {
                Some(Self::Sqlite)
            }
            #[cfg(not(feature = "sqlite"))]
            {
                None
            }
        } else {
            None
        }
    }

    pub fn operation_to_sql(&self, op: &Operation) -> Result<Vec<String>, DialectError> {
        match self {
            Dialect::Postgres => postgres::operation_to_sql(op),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::operation_to_sql(op),
        }
    }

    pub(crate) fn migration_to_sql(
        &self,
        migration: &Migration,
        _start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        match self {
            Dialect::Postgres => migration
                .operations
                .iter()
                .map(postgres::operation_to_sql)
                .collect::<Result<Vec<_>, _>>()
                .map(|chunks| chunks.into_iter().flatten().collect()),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::migration_to_sql(migration, _start),
        }
    }

    /// Reorders operations to satisfy database-specific execution constraints.
    /// The default is a no-op — only databases with ordering requirements need to override.
    /// Called once per migration after diffing, before SQL generation or writing.
    pub fn reorder(&self, ops: Vec<Operation>, previous: &Schema, current: &Schema) -> Vec<Operation> {
        match self {
            Dialect::Postgres => postgres::reorder_ops(ops, previous, current),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => ops,
        }
    }

    /// Whether a decomposed sub-entity op should be folded back into its
    /// parent CreateTable. Postgres can inline everything; other dialects
    /// (e.g. SQLite) may need FKs kept inline while indexes stay separate.
    pub fn should_merge(&self, _table_name: &str, _op: &Operation) -> bool {
        match self {
            Dialect::Postgres => true,
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => matches!(
                _op,
                Operation::AddForeignKey { .. } | Operation::AddConstraint { .. }
            ),
        }
    }

    /// Returns the DDL statements to bootstrap the migration tracking table.
    /// Uses CREATE TABLE IF NOT EXISTS so it is safe to call repeatedly.
    pub fn create_tracking_table_sql(&self) -> Vec<String> {
        match self {
            Dialect::Postgres => postgres::create_tracking_table_sql(),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::create_tracking_table_sql(),
        }
    }

    /// SQL to fetch all applied migration ids in application order.
    pub fn applied_migrations_sql(&self) -> &'static str {
        match self {
            Dialect::Postgres => "SELECT id FROM gaman_migrations ORDER BY applied_at, id",
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => "SELECT id FROM gaman_migrations ORDER BY applied_at, id",
        }
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
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::normalize_type(t),
        }
    }

    pub fn validate_migration(&self, m: &Migration) -> Result<(), DialectError> {
        match self {
            Dialect::Postgres => postgres::validate_migration(m),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::validate_migration(m),
        }
    }

    pub(crate) fn validate_migration_with_state(
        &self,
        m: &Migration,
        _start: &Schema,
    ) -> Result<(), DialectError> {
        match self {
            Dialect::Postgres => postgres::validate_migration(m),
            #[cfg(feature = "sqlite")]
            Dialect::Sqlite => sqlite::migration_to_sql(m, _start).map(|_| ()),
        }
    }
}

mod postgres;
#[cfg(feature = "sqlite")]
mod sqlite;
