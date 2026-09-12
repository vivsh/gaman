//! Failed checked writes must preserve database state, diagnostics, and migration tracking.

use super::lifecycle::{assert_items, inserted};
use super::support::*;
use gaman_core::{Command, CommandResult, ExecutorError};

/// Verifies stale updates and deletes affect zero rows and retain prior tracking and live data.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn stale_updates_and_deletes_fail_checked() {
    with_database(|config, mut observer| async move {
        let mut runner = inserted(&config).await;
        save(&config, &fixture(&ITEMS, 2)).await;
        sql(
            &mut observer,
            "UPDATE items SET name = 'external-change' WHERE id = 'managed'",
        )
        .await;
        let error = apply(&mut runner, "0003_update")
            .await
            .expect_err("stale update");
        precondition(&error, "0003_update", "apply", "UPDATE");
        records(&mut observer, &["0001_items", "0002_insert"]).await;
        assert_items(
            &mut observer,
            &["external:outside:keep-external", "managed:external-change:"],
        )
        .await;
        lock_released(&config, "0002_insert").await;
        sql(
            &mut observer,
            "UPDATE items SET name = 'first' WHERE id = 'managed'",
        )
        .await;
        apply(&mut runner, "0003_update")
            .await
            .expect("reuse original connection");
        save(&config, &fixture(&ITEMS, 3)).await;
        sql(
            &mut observer,
            "UPDATE items SET name = 'external-change' WHERE id = 'managed'",
        )
        .await;
        let error = apply(&mut runner, "0004_delete")
            .await
            .expect_err("stale delete");
        precondition(&error, "0004_delete", "apply", "DELETE");
        records(&mut observer, &["0001_items", "0002_insert", "0003_update"]).await;
        assert_items(
            &mut observer,
            &["external:outside:keep-external", "managed:external-change:"],
        )
        .await;
    })
    .await;
}

/// Checks the typed zero-row cause together with the enclosing migration failure context.
fn precondition(error: &gaman_core::CommandError, id: &str, direction: &str, signature: &str) {
    let cause = execution(error, id, direction, 1, signature);
    assert!(matches!(cause, ExecutorError::Execute(message) if message.contains("precondition")));
}

/// Verifies two matched identities cause an integrity failure and both updates are rolled back.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn multiple_affected_rows_roll_back() {
    with_database(|config, mut observer| async move {
        let mut runner = inserted(&config).await;
        save(&config, &fixture(&ITEMS, 2)).await;
        sql(&mut observer, "ALTER TABLE items DROP CONSTRAINT items_pkey; INSERT INTO items (id, name) VALUES ('managed', 'first')").await;
        let error = apply(&mut runner, "0003_update").await.expect_err("duplicate identity");
        let cause = execution(&error, "0003_update", "apply", 1, "UPDATE");
        assert!(matches!(cause, ExecutorError::Execute(message) if message.contains("integrity failure") && message.contains("got 2")));
        rows(&mut observer, "SELECT name FROM items WHERE id = 'managed' ORDER BY name", &["first", "first"]).await;
        records(&mut observer, &["0001_items", "0002_insert"]).await;
        lock_released(&config, "0002_insert").await;
    }).await;
}

/// Verifies a late zero-row write atomically rolls back earlier DDL and a successful managed insert.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn checked_failure_rolls_back_prior_schema_and_data() {
    with_database(|config, mut observer| async move {
        let mut runner = inserted(&config).await;
        let mut migration = gaman::Migration::from_yaml_str(include_str!("fixtures/atomic_failure.yaml"))
            .expect("atomic failure fixture");
        migration.id = "0003_atomic_failure".into();
        save(&config, &migration).await;
        sql(&mut observer, "UPDATE items SET name = 'external-change' WHERE id = 'managed'").await;
        let error = apply(&mut runner, "0003_atomic_failure").await.expect_err("late checked failure");
        execution(&error, "0003_atomic_failure", "apply", 3, "UPDATE");
        rows(&mut observer, "SELECT count(*)::text FROM information_schema.columns WHERE table_name = 'items' AND column_name = 'added'", &["0"]).await;
        assert_items(&mut observer, &["external:outside:keep-external", "managed:external-change:"]).await;
        records(&mut observer, &["0001_items", "0002_insert"]).await;
        let pending = runner.run_command(&Command::Apply(gaman_core::ApplyCommand::Plan)).await.expect("pending plan");
        assert!(matches!(pending, CommandResult::Pending(ids) if ids == ["0003_atomic_failure"]));
        lock_released(&config, "0002_insert").await;
    }).await;
}

/// Verifies inverse checked updates fail on changed old values and do not unrecord the migration.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn stale_rollback_preserves_tracking() {
    with_database(|config, mut observer| async move {
        let mut runner = directory(&config, &ITEMS, 3).await;
        apply(&mut runner, "0003_update")
            .await
            .expect("baseline update");
        sql(
            &mut observer,
            "UPDATE items SET name = 'external-change' WHERE id = 'managed'",
        )
        .await;
        let error = apply(&mut runner, "0002_insert")
            .await
            .expect_err("stale inverse update");
        let cause = execution(&error, "0003_update", "rollback", 1, "UPDATE");
        assert!(
            matches!(cause, ExecutorError::Execute(message) if message.contains("precondition"))
        );
        records(&mut observer, &["0001_items", "0002_insert", "0003_update"]).await;
        assert_items(
            &mut observer,
            &["external:outside:keep-external", "managed:external-change:"],
        )
        .await;
        lock_released(&config, "0003_update").await;
    })
    .await;
}

/// Verifies plain inserts preserve PostgreSQL duplicate-key diagnostics without adopting live rows.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn insert_collision_retains_sqlstate() {
    with_database(|config, mut observer| async move {
        let mut runner = directory(&config, &ITEMS, 2).await;
        apply(&mut runner, "0001_items").await.expect("create table");
        sql(&mut observer, "INSERT INTO items (id, name) VALUES ('managed', 'collision')").await;
        let error = apply(&mut runner, "0002_insert").await.expect_err("normal insert collision");
        let cause = execution(&error, "0002_insert", "apply", 1, "INSERT");
        assert!(matches!(cause, ExecutorError::ExecuteDatabase(failure) if failure.code.as_deref() == Some("23505")));
        records(&mut observer, &["0001_items"]).await;
        assert_items(&mut observer, &["external:outside:keep-external", "managed:collision:"]).await;
        lock_released(&config, "0001_items").await;
    }).await;
}
