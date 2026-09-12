//! Transparent native executor forwarding, including lazy connection errors.

use super::lifecycle::inserted;
use super::support::*;
use gaman::runner_factory::LazyExecutor;
use gaman::{Config, Executor, ExecutorError};
use gaman_core::Dialect;

/// Verifies the new lazy method classifies connection setup failures like ordinary execution.
#[tokio::test]
async fn affected_execution_preserves_connection_error_category() {
    let config = Config::new(
        "not-a-database-url".into(),
        "unused".into(),
        "unused".into(),
        Dialect::Postgres,
    );
    let mut lazy = LazyExecutor::new(config);
    let affected = lazy
        .execute_affected("SELECT 1")
        .await
        .expect_err("invalid URL");
    let ordinary = lazy.execute("SELECT 1").await.expect_err("invalid URL");
    assert!(matches!(&affected, ExecutorError::Execute(message) if !message.is_empty()));
    assert_eq!(affected.to_string(), ordinary.to_string());
}

/// Verifies a cold lazy connection returns exact zero, one, and multiple affected-row counts.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn affected_execution_preserves_counts_and_connection() {
    with_database(|config, mut observer| async move {
        let _runner = inserted(&config).await;
        let mut lazy = LazyExecutor::new(config);
        assert_eq!(
            lazy.execute_affected("UPDATE items SET external_note = 'all'")
                .await
                .expect("two rows"),
            2
        );
        lazy.begin().await.expect("begin on same connection");
        assert_eq!(
            lazy.execute_affected("UPDATE items SET external_note = 'one' WHERE id = 'managed'")
                .await
                .expect("one row"),
            1
        );
        assert_eq!(
            lazy.execute_affected("DELETE FROM items WHERE id = 'absent'")
                .await
                .expect("zero rows"),
            0
        );
        lazy.rollback().await.expect("rollback same connection");
        rows(
            &mut observer,
            "SELECT external_note FROM items ORDER BY id",
            &["all", "all"],
        )
        .await;
    })
    .await;
}
