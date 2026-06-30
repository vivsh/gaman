use sqlx::{Row, SqliteConnection};

use super::{BoxFuture, Executor, ExecutorError, Introspectable};
use crate::states::{Column, ForeignKey, Index, Schema, Table};

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
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn synth_fk_name(table: &str, from_column: &str) -> String {
    format!("{table}_{from_column}_fkey")
}

impl Introspectable for SqliteExecutor {
    fn inspect_db<'a>(
        &'a mut self,
        _schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, ExecutorError>> {
        Box::pin(async move {
            let mut state = Schema::default();

            let table_rows = sqlx::query(
                "SELECT name FROM sqlite_master \
                 WHERE type = 'table' \
                 AND name NOT LIKE 'sqlite_%' \
                 AND name != 'gaman_migrations' \
                 ORDER BY name",
            )
            .fetch_all(&mut self.conn)
            .await
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            for row in table_rows {
                let table_name: String = row
                    .try_get(0)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let quoted_table = quote_ident(&table_name);
                let mut table = Table {
                    name: table_name.clone(),
                    schema: None,
                    columns: vec![],
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                };

                let col_rows = sqlx::query(&format!("PRAGMA table_xinfo({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for cr in col_rows {
                    let hidden: i64 = cr
                        .try_get("hidden")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    if hidden != 0 {
                        continue;
                    }
                    let name: String = cr
                        .try_get("name")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let col_type: String = cr
                        .try_get("type")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let notnull: i64 = cr
                        .try_get("notnull")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let default: Option<String> = cr
                        .try_get("dflt_value")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let pk: i64 = cr
                        .try_get("pk")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    table.columns.push(Column {
                        name,
                        col_type,
                        nullable: notnull == 0 && pk == 0,
                        default,
                        primary_key: pk > 0,
                        references: None,
                        check: None,
                        generated: None,
                    });
                }

                let fk_rows = sqlx::query(&format!("PRAGMA foreign_key_list({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                for fkr in fk_rows {
                    let from_column: String = fkr
                        .try_get("from")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let to_table: String = fkr
                        .try_get("table")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let to_column: String = fkr
                        .try_get("to")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    table.foreign_keys.push(ForeignKey {
                        name: synth_fk_name(&table_name, &from_column),
                        from_column,
                        to_table,
                        to_column,
                    });
                }

                let idx_rows = sqlx::query(&format!("PRAGMA index_list({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                for idxr in idx_rows {
                    let idx_name: String = idxr
                        .try_get("name")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let origin: String = idxr
                        .try_get("origin")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    if origin != "c" {
                        continue;
                    }
                    let unique: i64 = idxr
                        .try_get("unique")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let quoted_idx = quote_ident(&idx_name);
                    let info_rows = sqlx::query(&format!("PRAGMA index_info({quoted_idx})"))
                        .fetch_all(&mut self.conn)
                        .await
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let mut columns = Vec::new();
                    for ir in info_rows {
                        columns.push(
                            ir.try_get::<String, _>("name")
                                .map_err(|e| ExecutorError::Fetch(e.to_string()))?,
                        );
                    }
                    table.indexes.push(Index {
                        name: idx_name,
                        columns,
                        unique: unique != 0,
                        predicate: None,
                    });
                }

                state.tables.insert(table_name, table);
            }

            Ok(state)
        })
    }
}
