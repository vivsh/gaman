use thiserror::Error;

use crate::conf::TlsMode;
use crate::environment::EnvironmentExecutor;
use gaman_core::dialects::Dialect;

/// Core lifecycle traits implemented by native SQLx executors.
pub use gaman_core::{BoxFuture, Executor, ExecutorError, InspectionError, SchemaInspector};

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
/// `inspect`, and live verification. Offline SQL planning never calls it.
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
            #[cfg(feature = "mysql")]
            (dialect @ Dialect::Mysql, TlsMode::NoTls) => {
                MysqlFamilyExecutor::connect(url, dialect)
                    .await
                    .map(|executor| Box::new(executor) as Box<dyn EnvironmentExecutor + Send>)
            }
            #[cfg(not(feature = "mysql"))]
            (Dialect::Mysql, _) => Err(ConnectError::Config(
                "mysql executor is not enabled; rebuild with the 'mysql' feature".into(),
            )),
            #[cfg(feature = "mariadb")]
            (dialect @ Dialect::Mariadb, TlsMode::NoTls) => {
                MysqlFamilyExecutor::connect(url, dialect)
                    .await
                    .map(|executor| Box::new(executor) as Box<dyn EnvironmentExecutor + Send>)
            }
            #[cfg(not(feature = "mariadb"))]
            (Dialect::Mariadb, _) => Err(ConnectError::Config(
                "mariadb executor is not enabled; rebuild with the 'mariadb' feature".into(),
            )),
        }
    })
}

#[cfg(any(feature = "mysql", feature = "mariadb"))]
pub mod mysql_family;
#[cfg(feature = "postgres")]
pub mod postgres;
#[cfg(feature = "sqlite")]
pub mod sqlite;
#[cfg(any(feature = "mysql", feature = "mariadb"))]
pub use mysql_family::MysqlFamilyExecutor;
#[cfg(feature = "postgres")]
pub use postgres::PostgresExecutor;
#[cfg(feature = "sqlite")]
pub use sqlite::SqliteExecutor;
