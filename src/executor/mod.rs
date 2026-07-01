use std::future::Future;
use std::pin::Pin;
use thiserror::Error;

use crate::conf::TlsMode;
use crate::environment::EnvironmentExecutor;
use gaman_core::dialects::Dialect;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + 'a>>;

#[derive(Debug, Error)]
pub enum ExecutorError {
    #[error("execute failed: {0}")]
    Execute(String),
    #[error("fetch failed: {0}")]
    Fetch(String),
    #[error("transaction error: {0}")]
    Transaction(String),
}

pub trait Executor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>>;
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>>;
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

pub trait Introspectable {
    fn inspect_db<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::states::Schema, ExecutorError>>;
}

#[derive(Debug, Error)]
pub enum ConnectError {
    #[error("{0}")]
    Config(String),
    #[error("database connection failed: {0}")]
    Connect(String),
}

#[allow(dead_code)]
pub fn connect_environment_executor<'a>(
    dialect: Dialect,
    url: &'a str,
    tls: TlsMode,
) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, ConnectError>> {
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
                Ok(Box::new(PostgresExecutor::new(conn)) as Box<dyn EnvironmentExecutor>)
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
                Ok(Box::new(SqliteExecutor::new(conn)) as Box<dyn EnvironmentExecutor>)
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
