use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;

use thiserror::Error;

use crate::dialects::Dialect;
use crate::migrations::Migration;

use super::execution_diagnostic::DatabaseFailure;
/// A boxed future that can be used by native hosts and single-threaded WASM adapters.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Errors returned by a caller-owned migration store.
#[derive(Debug, Error)]
pub enum StoreError {
    /// Loading migration history failed.
    #[error("migration store load failed: {message}")]
    Load { id: Option<String>, message: String },
    /// Persisting a generated migration failed.
    #[error("migration store save failed for '{id}': {message}")]
    Save { id: String, message: String },
    /// A migration already exists at the requested identity.
    #[error("migration already exists: {id}")]
    Conflict { id: String },
    /// Stored migration content could not be decoded or validated.
    #[error("invalid stored migration: {message}")]
    InvalidMigration { message: String },
    /// The backing migration store is unavailable.
    #[error("migration store unavailable: {message}")]
    Unavailable { message: String },
}

impl StoreError {
    /// Creates an unavailable-store failure when a narrower category is not known.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable {
            message: message.into(),
        }
    }
}

/// Errors returned by a caller-owned migration tracking store.
#[derive(Debug, Error)]
pub enum TrackingError {
    /// Installing tracking storage failed.
    #[error("tracking installation failed: {message}")]
    Install { message: String },
    /// Reading applied migration state failed.
    #[error("tracking read failed: {message}")]
    Read { message: String },
    /// Recording one applied migration failed.
    #[error("tracking record failed for '{id}': {message}")]
    Record { id: String, message: String },
    /// Removing one applied migration record failed.
    #[error("tracking removal failed for '{id}': {message}")]
    Unrecord { id: String, message: String },
    /// The tracking backend is unavailable.
    #[error("tracking store unavailable: {message}")]
    Unavailable { message: String },
}

impl TrackingError {
    /// Creates an unavailable-tracking failure when a narrower category is not known.
    pub fn unavailable(message: impl Into<String>) -> Self {
        Self::Unavailable {
            message: message.into(),
        }
    }
}

/// Errors returned by a caller-owned SQL executor.
#[derive(Debug, Error)]
pub enum ExecutorError {
    /// A target connection could not be established.
    #[error("connection failed: {0}")]
    Connect(String),
    /// A SQL statement could not be prepared without execution.
    #[error("prepare failed: {0}")]
    Prepare(String),
    /// A SQL statement could not be prepared and the database provided structured context.
    #[error("prepare failed: {0}")]
    PrepareDatabase(DatabaseFailure),
    /// A query needed for migration tracking could not be executed.
    #[error("query failed: {0}")]
    Fetch(String),
    /// A SQL statement could not be executed.
    #[error("execute failed: {0}")]
    Execute(String),
    /// A SQL statement could not be executed and the database provided structured context.
    #[error("execute failed: {0}")]
    ExecuteDatabase(DatabaseFailure),
    /// A transaction or migration lock operation failed.
    #[error("transaction failed: {0}")]
    Transaction(String),
    /// Acquiring or releasing the migration lock failed.
    #[error("migration lock failed: {0}")]
    Lock(String),
    /// The executor does not provide a required capability.
    #[error("unsupported executor capability: {0}")]
    Unsupported(String),
}

impl ExecutorError {
    /// Returns stable database context when the executor received it directly from the driver.
    pub(crate) fn database_failure(&self) -> Option<&DatabaseFailure> {
        match self {
            Self::PrepareDatabase(failure) | Self::ExecuteDatabase(failure) => Some(failure),
            _ => None,
        }
    }
}

/// Loads and persists migration definitions without prescribing a storage medium.
pub trait MigrationStore: Send + Sync {
    /// Loads every migration visible to this host.
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>>;

    /// Persists a newly generated migration atomically without replacing an existing identity.
    ///
    /// Implementations must not expose partial migration content. Concurrent saves for the same
    /// identity must allow at most one writer to succeed and return [`StoreError::Conflict`] for
    /// the others.
    fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>>;
}

impl<T> TrackingStore for &T
where
    T: TrackingStore + ?Sized,
{
    fn install<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        (**self).install(dialect, executor)
    }

    fn applied_ids<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>> {
        (**self).applied_ids(dialect, executor)
    }

    fn record<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        (**self).record(dialect, id, executor)
    }

    fn unrecord<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        (**self).unrecord(dialect, id, executor)
    }
}

/// Stores the applied migration IDs for a target environment.
pub trait TrackingStore: Send + Sync {
    /// Prepares the tracking backend before planning or applying migrations.
    fn install<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;

    /// Returns all applied migration IDs.
    fn applied_ids<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>>;

    /// Marks one migration as applied.
    fn record<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;

    /// Removes one migration from applied state.
    fn unrecord<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>>;
}

/// Executes SQL and transactional migration boundaries for a caller-owned target.
pub trait Executor: Send {
    /// Prepares one SQL statement without executing it when the host supports preparation.
    fn prepare<'a>(&'a mut self, _sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async {
            Err(ExecutorError::Prepare(
                "statement preparation is not supported by this executor".to_string(),
            ))
        })
    }

    /// Executes one SQL statement.
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Executes one checked DML statement and returns its affected-row count.
    fn execute_affected<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<u64, ExecutorError>> {
        Box::pin(async {
            Err(ExecutorError::Unsupported(
                "affected-row execution is not supported by this executor".to_string(),
            ))
        })
    }

    /// Returns the first text column from each result row for tracking adapters.
    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async {
            Err(ExecutorError::Fetch(
                "querying is not supported by this executor".to_string(),
            ))
        })
    }

    /// Starts one atomic migration transaction.
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Commits one atomic migration transaction.
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Rolls back one atomic migration transaction.
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Acquires an optional migration lock.
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    /// Releases an optional migration lock.
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

impl<T> Executor for &mut T
where
    T: Executor + ?Sized,
{
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).prepare(sql)
    }

    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).execute(sql)
    }

    fn execute_affected<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<u64, ExecutorError>> {
        (**self).execute_affected(sql)
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        (**self).fetch_strings(sql)
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).begin()
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).commit()
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).rollback()
    }

    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).acquire_lock()
    }

    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        (**self).release_lock()
    }
}

/// Name of the database table used by [`DatabaseTrackingStore`].
pub const TRACKING_TABLE: &str = "gaman_migrations";

/// Database-backed migration tracking implemented through the active executor.
#[derive(Debug, Default, Clone, Copy)]
pub struct DatabaseTrackingStore;

impl TrackingStore for DatabaseTrackingStore {
    fn install<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            let statements = dialect
                .tracking_install_sql(TRACKING_TABLE)
                .ok_or_else(|| TrackingError::Install {
                    message: format!(
                        "{} does not provide tracking-table installation SQL",
                        dialect.as_str()
                    ),
                })?;
            for statement in statements {
                executor
                    .execute(&statement)
                    .await
                    .map_err(|error| TrackingError::Install {
                        message: error.to_string(),
                    })?;
            }
            Ok(())
        })
    }

    fn applied_ids<'a>(
        &'a self,
        dialect: Dialect,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>> {
        Box::pin(async move {
            let sql =
                dialect
                    .tracking_list_sql(TRACKING_TABLE)
                    .ok_or_else(|| TrackingError::Read {
                        message: "dialect does not provide tracking read SQL".to_string(),
                    })?;
            executor
                .fetch_strings(&sql)
                .await
                .map(|ids| ids.into_iter().collect())
                .map_err(|error| TrackingError::Read {
                    message: error.to_string(),
                })
        })
    }

    fn record<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            let sql = dialect
                .tracking_record_sql(TRACKING_TABLE, id)
                .ok_or_else(|| TrackingError::Record {
                    id: id.to_string(),
                    message: "dialect does not provide tracking record SQL".to_string(),
                })?;
            executor
                .execute(&sql)
                .await
                .map_err(|error| TrackingError::Record {
                    id: id.to_string(),
                    message: error.to_string(),
                })
        })
    }

    fn unrecord<'a>(
        &'a self,
        dialect: Dialect,
        id: &'a str,
        executor: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async move {
            let sql = dialect
                .tracking_unrecord_sql(TRACKING_TABLE, id)
                .ok_or_else(|| TrackingError::Unrecord {
                    id: id.to_string(),
                    message: "dialect does not provide tracking removal SQL".to_string(),
                })?;
            executor
                .execute(&sql)
                .await
                .map_err(|error| TrackingError::Unrecord {
                    id: id.to_string(),
                    message: error.to_string(),
                })
        })
    }
}
