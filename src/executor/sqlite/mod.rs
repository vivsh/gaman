use sqlx::{Row, SqliteConnection};

use super::{BoxFuture, Executor, ExecutorError, InspectionError, SchemaInspector};
use gaman_core::migration_engine::DatabaseFailure;

/// Wraps a live SQLite connection and manages transaction boundaries explicitly.
pub struct SqliteExecutor {
    conn: SqliteConnection,
}

impl SqliteExecutor {
    pub fn new(conn: SqliteConnection) -> Self {
        Self { conn }
    }
}

impl Executor for SqliteExecutor {
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::Executor::prepare(&mut self.conn, sql)
                .await
                .map(|_| ())
                .map_err(|error| {
                    ExecutorError::PrepareDatabase(DatabaseFailure::message(error.to_string()))
                })
        })
    }

    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query(sql)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|error| {
                    ExecutorError::ExecuteDatabase(DatabaseFailure::message(error.to_string()))
                })
        })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async move {
            let rows = sqlx::query(sql)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
            rows.into_iter()
                .map(|r| {
                    r.try_get::<String, _>(0)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))
                })
                .collect()
        })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("BEGIN")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("COMMIT")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("ROLLBACK")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }
}

pub(super) fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

pub(super) fn synth_fk_name(table: &str, from_column: &str) -> String {
    format!("{table}_{from_column}_fkey")
}

type SqliteFkColumns = Vec<(i64, String, String)>;
pub(super) type SqliteFkGroups =
    std::collections::BTreeMap<i64, (String, SqliteFkColumns, Option<String>, Option<String>)>;

mod inspection;
