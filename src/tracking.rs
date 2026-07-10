use std::collections::HashSet;

use thiserror::Error;

use crate::executor::{BoxFuture, Executor, ExecutorError};
use gaman_core::dialects::{Dialect, DialectError};
use gaman_core::migrations::Migration;
use gaman_core::operations::Operation;
use gaman_core::sql_plan::{SqlPlanError, render_migration_sql};
use gaman_core::states::{Schema, TableBuilder};

/// Database table used by the built-in migration tracking store.
pub const TRACKING_TABLE: &str = "gaman_migrations";

#[derive(Debug, Error)]
/// Errors returned by migration tracking storage.
pub enum TrackingError {
    /// The database executor failed while installing or updating tracking state.
    #[error(transparent)]
    Executor(#[from] ExecutorError),
    /// Tracking table SQL could not be planned for the selected dialect.
    #[error(transparent)]
    SqlPlan(#[from] SqlPlanError),
    /// Tracking table SQL could not be made idempotent safely.
    #[error("tracking SQL could not be made idempotent: {0}")]
    Install(String),
}

/// Stores migration application state for a target environment.
///
/// Database-backed migration application stores ids in [`TRACKING_TABLE`].
/// Browser or embedded hosts can provide a different store later, such as
/// LocalStorage or IndexedDB, without changing offline planning.
pub trait TrackingStore: Send + Sync {
    /// Prepares the tracking store before migration application starts.
    fn install<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;

    /// Returns all applied migration ids in application order.
    fn applied_ids<'a>(
        &'a self,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>>;

    /// Records a migration as applied.
    fn record<'a>(
        &'a self,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;

    /// Removes a migration record during rollback.
    fn unrecord<'a>(
        &'a self,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;
}

/// Database-table implementation of migration tracking.
pub struct DatabaseTrackingStore;

impl TrackingStore for DatabaseTrackingStore {
    fn install<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            for sql in tracking_install_sql(dialect)? {
                executor.execute(&sql).await?;
            }
            Ok(())
        })
    }

    fn applied_ids<'a>(
        &'a self,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>> {
        Box::pin(async move {
            let sql = format!("SELECT id FROM {TRACKING_TABLE} ORDER BY applied_at, id");
            Ok(executor.fetch_strings(&sql).await?.into_iter().collect())
        })
    }

    fn record<'a>(
        &'a self,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            executor.execute(&record_sql(id)).await?;
            Ok(())
        })
    }

    fn unrecord<'a>(
        &'a self,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            executor.execute(&unrecord_sql(id)).await?;
            Ok(())
        })
    }
}

fn tracking_install_sql(dialect: Dialect) -> Result<Vec<String>, TrackingError> {
    let migration = Migration {
        id: "__gaman_install_tracking".to_string(),
        dependencies: vec![],
        operations: vec![Operation::CreateTable {
            table: tracking_table(dialect),
        }],
        atomic: true,
    };
    render_migration_sql(dialect, &migration, &Schema::default())?
        .into_iter()
        .map(make_install_sql_idempotent)
        .collect()
}

fn tracking_table(dialect: Dialect) -> gaman_core::states::Table {
    let (applied_at_type, applied_at_default) = match dialect {
        Dialect::Postgres => ("timestamptz", "now()"),
        Dialect::Sqlite => ("text", "CURRENT_TIMESTAMP"),
        Dialect::Mysql => ("datetime", "CURRENT_TIMESTAMP"),
    };
    TableBuilder::new(TRACKING_TABLE)
        .column("id", "text", |column| column.not_null())
        .column("applied_at", applied_at_type, |column| {
            column.not_null().default(applied_at_default)
        })
        .unique(format!("{TRACKING_TABLE}_id_key"), &["id"])
        .index(format!("{TRACKING_TABLE}_id_idx"), &["id"])
        .build()
}

fn make_install_sql_idempotent(sql: String) -> Result<String, TrackingError> {
    if let Some(rest) = sql.strip_prefix("CREATE TABLE ") {
        Ok(format!("CREATE TABLE IF NOT EXISTS {rest}"))
    } else if let Some(rest) = sql.strip_prefix("CREATE INDEX ") {
        Ok(format!("CREATE INDEX IF NOT EXISTS {rest}"))
    } else {
        Err(TrackingError::Install(sql))
    }
}

fn record_sql(id: &str) -> String {
    let escaped = id.replace('\'', "''");
    format!("INSERT INTO {TRACKING_TABLE} (id) VALUES ('{escaped}')")
}

fn unrecord_sql(id: &str) -> String {
    let escaped = id.replace('\'', "''");
    format!("DELETE FROM {TRACKING_TABLE} WHERE id = '{escaped}'")
}

impl From<DialectError> for TrackingError {
    fn from(value: DialectError) -> Self {
        Self::SqlPlan(SqlPlanError::Dialect {
            migration: "__gaman_install_tracking".to_string(),
            source: value,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies PostgreSQL tracking install SQL remains idempotent.
    #[test]
    fn postgres_tracking_install_sql_is_idempotent() {
        let sql = tracking_install_sql(Dialect::Postgres).unwrap();
        assert!(sql[0].starts_with("CREATE TABLE IF NOT EXISTS"));
        assert!(sql[1].starts_with("CREATE INDEX IF NOT EXISTS"));
        assert!(sql[0].contains(&format!("\"{TRACKING_TABLE}\"")));
    }

    /// Verifies SQLite tracking install SQL remains idempotent.
    #[test]
    fn sqlite_tracking_install_sql_is_idempotent() {
        let sql = tracking_install_sql(Dialect::Sqlite).unwrap();
        assert!(sql[0].starts_with("CREATE TABLE IF NOT EXISTS"));
        assert!(sql[1].starts_with("CREATE INDEX IF NOT EXISTS"));
        assert!(sql[0].contains(&format!("\"{TRACKING_TABLE}\"")));
    }
}
