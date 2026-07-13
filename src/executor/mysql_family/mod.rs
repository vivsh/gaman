//! Live MySQL and MariaDB execution and catalog inspection.

use gaman_core::dialects::Dialect;
use sqlx::{ConnectOptions, MySqlConnection};

use super::{BoxFuture, ConnectError, Executor, ExecutorError, InspectionError, SchemaInspector};

/// SQLx-backed executor shared by MySQL and MariaDB after product validation.
pub struct MysqlFamilyExecutor {
    conn: MySqlConnection,
    dialect: Dialect,
    database: String,
    lock_name: String,
}

impl MysqlFamilyExecutor {
    /// Connects to and validates the selected MySQL-family server.
    pub async fn connect(url: &str, dialect: Dialect) -> Result<Self, ConnectError> {
        let options = url
            .parse::<sqlx::mysql::MySqlConnectOptions>()
            .map_err(|error| ConnectError::Connect(error.to_string()))?;
        let database = options.get_database().unwrap_or("").to_string();
        if database.is_empty() {
            return Err(ConnectError::Config(
                "MySQL and MariaDB URLs must select a database".to_string(),
            ));
        }
        let mut conn = options
            .connect()
            .await
            .map_err(|error| ConnectError::Connect(error.to_string()))?;
        let version: String = sqlx::query_scalar("SELECT VERSION()")
            .fetch_one(&mut conn)
            .await
            .map_err(|error| ConnectError::Connect(error.to_string()))?;
        validate_server(dialect, &version)?;
        let lock_name = format!("gaman:{}", database.chars().take(48).collect::<String>());
        Ok(Self {
            conn,
            dialect,
            database,
            lock_name,
        })
    }
}

/// Validates server identity and the minimum product version Gaman can operate against.
fn validate_server(dialect: Dialect, version: &str) -> Result<(), ConnectError> {
    let maria = version.to_ascii_lowercase().contains("mariadb");
    match dialect {
        Dialect::Mysql if maria => Err(ConnectError::Config(format!(
            "mysql URL reached MariaDB server {version}; use a mariadb URL"
        ))),
        Dialect::Mariadb if !maria => Err(ConnectError::Config(format!(
            "mariadb URL reached MySQL server {version}; use a mysql URL"
        ))),
        Dialect::Mysql if mysql_version(version)? < (8, 4) => Err(ConnectError::Config(format!(
            "unsupported MySQL server {version}; Gaman requires MySQL 8.4 or newer"
        ))),
        Dialect::Mariadb if !(version.contains("11.4.") || version.contains("11.8.")) => {
            Err(ConnectError::Config(format!(
                "unsupported MariaDB server {version}; Gaman requires MariaDB 11.4 or 11.8"
            )))
        }
        Dialect::Mysql | Dialect::Mariadb => Ok(()),
        _ => Err(ConnectError::Config(
            "MySQL-family executor received a non-family dialect".to_string(),
        )),
    }
}

/// Extracts the MySQL major and minor version used for minimum-version validation.
fn mysql_version(version: &str) -> Result<(u64, u64), ConnectError> {
    let mut parts = version.split('.');
    let major = parts.next().and_then(|part| part.parse().ok());
    let minor = parts.next().and_then(|part| part.parse().ok());
    major.zip(minor).ok_or_else(|| {
        ConnectError::Config(format!(
            "could not determine MySQL server version from {version:?}"
        ))
    })
}

impl Executor for MysqlFamilyExecutor {
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::Executor::prepare(&mut self.conn, sql)
                .await
                .map(|_| ())
                .map_err(|error| ExecutorError::Prepare(format!("{error}\n  SQL: {sql}")))
        })
    }
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::Executor::execute(&mut self.conn, sql)
                .await
                .map(|_| ())
                .map_err(|error| ExecutorError::Execute(format!("{error}\n  SQL: {sql}")))
        })
    }
    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async move {
            sqlx::query_scalar(sql)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|error| ExecutorError::Fetch(error.to_string()))
        })
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("START TRANSACTION")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|error| ExecutorError::Transaction(error.to_string()))
        })
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("COMMIT")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|error| ExecutorError::Transaction(error.to_string()))
        })
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("ROLLBACK")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|error| ExecutorError::Transaction(error.to_string()))
        })
    }
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            let acquired: Option<i64> = sqlx::query_scalar("SELECT GET_LOCK(?, 30)")
                .bind(&self.lock_name)
                .fetch_one(&mut self.conn)
                .await
                .map_err(|error| ExecutorError::Execute(error.to_string()))?;
            if acquired == Some(1) {
                Ok(())
            } else {
                Err(ExecutorError::Execute(
                    "could not acquire migration lock within 30 seconds".to_string(),
                ))
            }
        })
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            let released: Option<i64> = sqlx::query_scalar("SELECT RELEASE_LOCK(?)")
                .bind(&self.lock_name)
                .fetch_one(&mut self.conn)
                .await
                .map_err(|error| ExecutorError::Execute(error.to_string()))?;
            if released == Some(1) {
                Ok(())
            } else {
                Err(ExecutorError::Execute(
                    "migration lock was not held by this connection".to_string(),
                ))
            }
        })
    }
}

mod inspection;
