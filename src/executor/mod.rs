use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

use crate::conf::TlsMode;
use crate::environment::EnvironmentExecutor;
use gaman_core::dialects::Dialect;

/// Send boxed future used by object-safe live database traits.
///
/// Custom executors must return futures that can move across Tokio worker
/// threads while live migration, inspect, or verify work is in progress.
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Error)]
/// Errors returned by live SQL execution, transaction, and introspection calls.
pub enum ExecutorError {
    /// A statement could not be prepared by the configured database driver.
    #[error("prepare failed: {0}")]
    Prepare(String),
    /// A statement failed during execution.
    #[error("execute failed: {0}")]
    Execute(String),
    /// A query failed while fetching data needed by Gaman.
    #[error("fetch failed: {0}")]
    Fetch(String),
    /// A transaction boundary or rollback operation failed.
    #[error("transaction error: {0}")]
    Transaction(String),
}

/// Executes live SQL and migration lifecycle operations for a database backend.
///
/// Implementations are used only by native live migration paths. Offline
/// planning and `sql_migrate` render SQL without an executor.
pub trait Executor: Send {
    /// Prepares one statement without executing it.
    ///
    /// Implementations should delegate to their database driver's prepare
    /// operation. This is used by `MigrationEngine::check_sql_schema` and must
    /// not execute SQL, begin a transaction, or change migration state.
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Executes a statement that does not return rows to Gaman.
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Fetches a single string column from each returned row.
    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>>;

    /// Starts a migration transaction.
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Commits the current migration transaction.
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Rolls back the current migration transaction.
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;

    /// Acquires the database-level migration lock when the backend supports one.
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    /// Releases a previously acquired migration lock.
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Inspects a live database and returns Gaman's schema representation.
pub trait Introspectable: Send {
    /// Reads the requested schemas from the database catalog.
    fn inspect_db<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::states::Schema, ExecutorError>>;
}

#[derive(Debug, Error)]
/// Errors returned while creating a backend-specific live executor.
pub enum ConnectError {
    /// The requested connection cannot be created from the supplied configuration.
    #[error("{0}")]
    Config(String),
    /// The database client failed to establish a live connection.
    #[error("database connection failed: {0}")]
    Connect(String),
}

#[allow(dead_code)]
/// Opens the configured live executor for the selected dialect.
///
/// This is native-only connection plumbing for migration application,
/// `inspect_db`, and live verification. Offline SQL planning never calls it.
pub fn connect_environment_executor<'a>(
    dialect: Dialect,
    url: &'a str,
    tls: TlsMode,
) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, ConnectError>> {
    Box::pin(async move {
        match (dialect, tls) {
            #[cfg(feature = "postgres")]
            (Dialect::Postgres, TlsMode::NoTls) => {
                use sqlx::ConnectOptions;
                let opts = url
                    .parse::<sqlx::postgres::PgConnectOptions>()
                    .map_err(|e| ConnectError::Connect(e.to_string()))?
                    .ssl_mode(sqlx::postgres::PgSslMode::Disable);
                let conn = opts
                    .connect()
                    .await
                    .map_err(|e| ConnectError::Connect(e.to_string()))?;
                Ok(Box::new(PostgresExecutor::new(conn)) as Box<dyn EnvironmentExecutor + Send>)
            }
            #[cfg(not(feature = "postgres"))]
            (Dialect::Postgres, TlsMode::NoTls) => {
                let _ = url;
                Err(ConnectError::Config(
                    "postgres executor is not enabled; rebuild with the 'postgres' feature".into(),
                ))
            }
            #[cfg(feature = "sqlite")]
            (Dialect::Sqlite, TlsMode::NoTls) => {
                use sqlx::ConnectOptions;
                let opts = url
                    .parse::<sqlx::sqlite::SqliteConnectOptions>()
                    .map_err(|e| ConnectError::Connect(e.to_string()))?
                    .foreign_keys(true);
                let conn = opts
                    .connect()
                    .await
                    .map_err(|e| ConnectError::Connect(e.to_string()))?;
                Ok(Box::new(SqliteExecutor::new(conn)) as Box<dyn EnvironmentExecutor + Send>)
            }
            #[cfg(not(feature = "sqlite"))]
            (Dialect::Sqlite, TlsMode::NoTls) => {
                let _ = url;
                Err(ConnectError::Config(
                    "sqlite executor is not enabled; rebuild with the 'sqlite' feature".into(),
                ))
            }
            (Dialect::Mysql, _) => {
                let _ = url;
                Err(ConnectError::Config(
                    "mysql executor is not implemented".into(),
                ))
            }
        }
    })
}

#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(feature = "postgres")]
pub use postgres::PostgresExecutor;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteExecutor;
