//! Checked row lifecycle through both native migration sources.

use super::support::*;
use gaman::Config;
use gaman_core::CommandResult;
use sqlx::PgConnection;

/// Verifies directory-backed insert/update/delete and inverses preserve unrelated data and tracking.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn directory_managed_row_lifecycle() {
    with_database(|config, observer| async move {
        let runner = directory(&config, &ITEMS, 4).await;
        lifecycle(runner, observer).await;
    })
    .await;
}

/// Verifies application-style embedded migrations traverse the same checked native executor.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn embedded_managed_row_lifecycle() {
    with_database(|config, observer| async move {
        lifecycle(embedded(config, &ITEMS), observer).await;
    })
    .await;
}

/// Exercises all three row operations and their exact inverse values through a real runner.
async fn lifecycle(mut runner: Runner, mut observer: PgConnection) {
    apply(&mut runner, "0002_insert")
        .await
        .expect("insert managed row");
    assert_items(
        &mut observer,
        &["external:outside:keep-external", "managed:first:"],
    )
    .await;
    records(&mut observer, &["0001_items", "0002_insert"]).await;
    sql(
        &mut observer,
        "UPDATE items SET external_note = 'keep-owned' WHERE id = 'managed'",
    )
    .await;
    apply(&mut runner, "0003_update")
        .await
        .expect("update managed row");
    assert_items(
        &mut observer,
        &[
            "external:outside:keep-external",
            "managed:second:keep-owned",
        ],
    )
    .await;
    apply(&mut runner, "0002_insert")
        .await
        .expect("reverse checked update");
    assert_items(
        &mut observer,
        &["external:outside:keep-external", "managed:first:keep-owned"],
    )
    .await;
    apply(&mut runner, "0004_delete")
        .await
        .expect("update and delete");
    assert_items(&mut observer, &["external:outside:keep-external"]).await;
    records(
        &mut observer,
        &["0001_items", "0002_insert", "0003_update", "0004_delete"],
    )
    .await;
    reverse_deletion(runner, observer).await;
}

/// Restores managed values in reverse order and proves the same history can be reapplied.
async fn reverse_deletion(mut runner: Runner, mut observer: PgConnection) {
    apply(&mut runner, "0003_update")
        .await
        .expect("inverse delete reinserts managed values");
    assert_items(
        &mut observer,
        &["external:outside:keep-external", "managed:second:"],
    )
    .await;
    records(&mut observer, &["0001_items", "0002_insert", "0003_update"]).await;
    apply(&mut runner, "0001_items")
        .await
        .expect("inverse update and insert");
    assert_items(&mut observer, &["external:outside:keep-external"]).await;
    records(&mut observer, &["0001_items"]).await;
    let result = apply(&mut runner, "0004_delete")
        .await
        .expect("reapply lifecycle");
    assert!(matches!(result, CommandResult::Movement(m) if m.applied == 3 && m.reverted == 0));
}

/// Checks both ownership boundaries with the same deterministic observation query.
pub(super) async fn assert_items(observer: &mut PgConnection, expected: &[&str]) {
    rows(
        observer,
        "SELECT id || ':' || name || ':' || COALESCE(external_note, '') FROM items ORDER BY id",
        expected,
    )
    .await;
}

/// Establishes a recorded managed row before deliberate drift in failure scenarios.
pub(super) async fn inserted(config: &Config) -> Runner {
    let mut runner = directory(config, &ITEMS, 2).await;
    apply(&mut runner, "0002_insert")
        .await
        .expect("baseline managed insert");
    runner
}
