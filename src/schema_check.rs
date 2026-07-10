//! Storage-neutral live validation of authored SQL schema statements.

use crate::executor::Executor;
use gaman_core::dialects::Dialect;
use gaman_core::parsers::segment_sql;

/// One authored SQL schema input supplied by a host application.
///
/// `label` is shown in diagnostics and may be a filesystem path, browser name,
/// or another host-defined identifier. The engine never interprets it as a
/// path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqlSchemaInput {
    /// Stable display label supplied by the caller.
    pub label: String,
    /// Exact authored SQL source to validate.
    pub source: String,
}

impl SqlSchemaInput {
    /// Creates an in-memory SQL schema input with a caller-defined label.
    pub fn new(label: impl Into<String>, source: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            source: source.into(),
        }
    }
}

/// Aggregate result of validating ordered SQL schema inputs.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SchemaCheckReport {
    /// Results in the same order as the supplied inputs.
    pub files: Vec<SchemaCheckFileReport>,
}

impl SchemaCheckReport {
    /// Returns true when any checked input has one or more failures.
    pub fn has_failures(&self) -> bool {
        self.files.iter().any(SchemaCheckFileReport::has_failures)
    }
}

/// Validation result for one authored schema input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaCheckFileReport {
    /// Caller-provided display label for the input.
    pub label: String,
    /// Structured status for the checked or ignored input.
    pub status: SchemaCheckFileStatus,
}

impl SchemaCheckFileReport {
    /// Creates a report for an input intentionally ignored by this check.
    pub fn ignored(label: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            status: SchemaCheckFileStatus::Ignored {
                reason: reason.into(),
            },
        }
    }

    /// Returns true when this checked input contains at least one failure.
    pub fn has_failures(&self) -> bool {
        matches!(
            &self.status,
            SchemaCheckFileStatus::Checked { failures, .. } if !failures.is_empty()
        )
    }
}

/// Outcome for one schema input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCheckFileStatus {
    /// An SQL input was segmented and each statement was prepared.
    Checked {
        /// Number of statements prepared successfully.
        passed: usize,
        /// Segmentation or prepare failures collected without stopping later inputs.
        failures: Vec<SchemaCheckFailure>,
    },
    /// A non-SQL schema input was deliberately not sent to the database.
    Ignored {
        /// Human-readable reason the input was ignored.
        reason: String,
    },
}

/// One failure found while validating an SQL schema input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SchemaCheckFailure {
    /// The source could not be deterministically segmented into statements.
    Segmentation {
        /// Segmentation diagnostic.
        message: String,
    },
    /// A segmented statement could not be prepared by the selected database.
    Statement {
        /// One-based statement ordinal within the source file.
        ordinal: usize,
        /// One-based source line where the statement begins.
        line: usize,
        /// Driver-provided prepare diagnostic.
        message: String,
    },
}

/// Validates SQL inputs through a connected executor without executing them.
///
/// This deliberately does not begin transactions, acquire locks, inspect
/// migration state, or read a migration source. Each statement is prepared in
/// source order, and statement failures are accumulated so callers receive a
/// complete report for the run.
pub(crate) async fn check_sql_schema_with_executor<E: Executor + ?Sized>(
    executor: &mut E,
    dialect: Dialect,
    files: impl IntoIterator<Item = SqlSchemaInput>,
) -> SchemaCheckReport {
    let mut reports = Vec::new();
    for input in files {
        reports.push(check_one_sql_input(executor, dialect, input).await);
    }
    SchemaCheckReport { files: reports }
}

/// Segments and prepares one SQL input, retaining every prepare failure.
async fn check_one_sql_input<E: Executor + ?Sized>(
    executor: &mut E,
    dialect: Dialect,
    input: SqlSchemaInput,
) -> SchemaCheckFileReport {
    let segments = match segment_sql(&input.source, dialect) {
        Ok(segments) => segments,
        Err(error) => {
            return SchemaCheckFileReport {
                label: input.label,
                status: SchemaCheckFileStatus::Checked {
                    passed: 0,
                    failures: vec![SchemaCheckFailure::Segmentation {
                        message: error.to_string(),
                    }],
                },
            };
        }
    };

    let mut passed = 0;
    let mut failures = Vec::new();
    for segment in segments {
        match executor.prepare(&segment.sql).await {
            Ok(()) => passed += 1,
            Err(error) => failures.push(SchemaCheckFailure::Statement {
                ordinal: segment.ordinal,
                line: segment.start_line,
                message: error.to_string(),
            }),
        }
    }
    SchemaCheckFileReport {
        label: input.label,
        status: SchemaCheckFileStatus::Checked { passed, failures },
    }
}

#[cfg(test)]
mod tests {
    use super::{
        SchemaCheckFailure, SchemaCheckFileStatus, SqlSchemaInput, check_sql_schema_with_executor,
    };
    use crate::executor::{BoxFuture, Executor, ExecutorError};
    use gaman_core::dialects::Dialect;

    #[derive(Default)]
    struct RecordingExecutor {
        prepared: Vec<String>,
        executed: Vec<String>,
        transactions: usize,
        locks: usize,
    }

    impl Executor for RecordingExecutor {
        fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.executed.push(sql.to_string());
            Box::pin(async { Ok(()) })
        }

        fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.prepared.push(sql.to_string());
            let fails = sql.contains("BROKEN");
            Box::pin(async move {
                if fails {
                    Err(ExecutorError::Prepare("forced failure".to_string()))
                } else {
                    Ok(())
                }
            })
        }

        fn fetch_strings<'a>(
            &'a mut self,
            _sql: &'a str,
        ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
            Box::pin(async { Ok(Vec::new()) })
        }

        fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.transactions += 1;
            Box::pin(async { Ok(()) })
        }

        fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.transactions += 1;
            Box::pin(async { Ok(()) })
        }

        fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.transactions += 1;
            Box::pin(async { Ok(()) })
        }

        fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.locks += 1;
            Box::pin(async { Ok(()) })
        }
    }

    /// Verifies schema checking prepares every segment without migration lifecycle calls.
    #[tokio::test]
    async fn schema_check_prepares_segments_without_execution_or_lifecycle_calls() {
        let mut executor = RecordingExecutor::default();
        let report = check_sql_schema_with_executor(
            &mut executor,
            Dialect::Postgres,
            [SqlSchemaInput::new(
                "schema.sql",
                "CREATE TABLE users (id integer); CREATE TABLE posts (id integer);",
            )],
        )
        .await;

        assert_eq!(executor.prepared.len(), 2);
        assert!(executor.executed.is_empty());
        assert_eq!(executor.transactions, 0);
        assert_eq!(executor.locks, 0);
        assert!(!report.has_failures());
    }

    /// Verifies later statements are prepared after an earlier prepare failure.
    #[tokio::test]
    async fn schema_check_collects_prepare_failures_without_stopping_later_statements() {
        let mut executor = RecordingExecutor::default();
        let report = check_sql_schema_with_executor(
            &mut executor,
            Dialect::Postgres,
            [SqlSchemaInput::new(
                "schema.sql",
                "CREATE TABLE users (id integer); BROKEN; CREATE TABLE posts (id integer);",
            )],
        )
        .await;

        assert_eq!(executor.prepared.len(), 3);
        let SchemaCheckFileStatus::Checked { passed, failures } = &report.files[0].status else {
            panic!("SQL input should be checked");
        };
        assert_eq!(*passed, 2);
        assert_eq!(failures.len(), 1);
        assert!(matches!(
            failures[0],
            SchemaCheckFailure::Statement {
                ordinal: 2,
                line: 1,
                ..
            }
        ));
    }

    /// Verifies segmentation failures do not send partial source to the executor.
    #[tokio::test]
    async fn schema_check_reports_segmentation_failures_without_prepare_calls() {
        let mut executor = RecordingExecutor::default();
        let report = check_sql_schema_with_executor(
            &mut executor,
            Dialect::Postgres,
            [SqlSchemaInput::new(
                "schema.sql",
                "CREATE TABLE users (id integer); /*",
            )],
        )
        .await;

        assert!(executor.prepared.is_empty());
        assert!(matches!(
            report.files[0].status,
            SchemaCheckFileStatus::Checked {
                ref failures,
                ..
            } if matches!(failures.as_slice(), [SchemaCheckFailure::Segmentation { .. }])
        ));
    }
}
