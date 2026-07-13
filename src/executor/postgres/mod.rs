use sqlx::PgConnection;
use sqlx::Row;

use super::{BoxFuture, Executor, ExecutorError, InspectionError, SchemaInspector};

const GAMAN_LOCK_KEY: i64 = 7242068691819328000;

/// Wraps a live Postgres connection and manages transaction boundaries explicitly.
/// Call `begin()` before a migration and `commit()` or `rollback()` after.
pub struct PostgresExecutor {
    conn: PgConnection,
}

impl PostgresExecutor {
    pub fn new(conn: PgConnection) -> Self {
        Self { conn }
    }
}

impl Executor for PostgresExecutor {
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::Executor::prepare(&mut self.conn, sql)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Prepare(format!("{e}\n  SQL: {sql}")))
        })
    }

    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query(sql)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Execute(format!("{e}\n  SQL: {sql}")))
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

    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("SET lock_timeout = '30s'")
                .execute(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Execute(e.to_string()))?;
            sqlx::query("SELECT pg_advisory_lock($1)")
                .bind(GAMAN_LOCK_KEY)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| {
                    ExecutorError::Execute(format!("could not acquire migration lock: {e}"))
                })
        })
    }

    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(GAMAN_LOCK_KEY)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| {
                    ExecutorError::Execute(format!("could not release migration lock: {e}"))
                })
        })
    }
}

mod inspection;
