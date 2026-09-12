//! Regression for defaulted persona references to newly inserted managed presets.

use super::support::*;
use gaman_core::operations::Operation;
use gaman_core::{Command, CommandResult, ExecutorError};
use sqlx::PgConnection;

/// Verifies the native directory runner installs presets before validating existing persona FKs.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn directory_model_profiles_regression() {
    with_database(|config, observer| async move {
        profiles(directory(&config, &PROFILES, 2).await, observer).await;
    })
    .await;
}

/// Verifies compiled application migrations support the same model-profile installation and rollback.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn embedded_model_profiles_regression() {
    with_database(|config, observer| async move {
        profiles(embedded(config, &PROFILES), observer).await;
    })
    .await;
}

/// Checks statement-three ordering, persisted presets, FK enforcement, and inverse schema changes.
async fn profiles(mut runner: Runner, mut observer: PgConnection) {
    apply(&mut runner, "0007_personas")
        .await
        .expect("existing personas");
    assert_profile_sql(&mut runner).await;
    apply(&mut runner, "0008_model_profiles")
        .await
        .expect("managed insert at statement 3");
    rows(
        &mut observer,
        "SELECT name || ':' || model_profile_id FROM personas ORDER BY id",
        &["Alice:standard", "Bob:standard"],
    )
    .await;
    rows(
        &mut observer,
        "SELECT id || ':' || name FROM model_profiles ORDER BY id",
        &["focused:Focused", "standard:Standard"],
    )
    .await;
    verified(&mut runner).await;
    let error = sqlx::query("INSERT INTO personas VALUES (3, 'Invalid', 'missing')")
        .execute(&mut observer)
        .await
        .expect_err("foreign key is enforced");
    assert_eq!(
        error.as_database_error().and_then(|e| e.code()).as_deref(),
        Some("23503")
    );
    let repeat = apply(&mut runner, "0008_model_profiles")
        .await
        .expect("already applied");
    assert!(matches!(repeat, CommandResult::Movement(m) if m.applied == 0 && m.reverted == 0));
    records(&mut observer, &["0007_personas", "0008_model_profiles"]).await;
    apply(&mut runner, "0007_personas")
        .await
        .expect("rollback profile migration");
    original_personas(&mut observer).await;
    records(&mut observer, &["0007_personas"]).await;
}

/// Proves the original failing insert occupies statement three in native SQL output.
async fn assert_profile_sql(runner: &mut Runner) {
    let result = runner
        .run_command(&Command::Sql {
            id: Some("0008_model_profiles".into()),
            backwards: false,
        })
        .await
        .expect("migration SQL");
    let CommandResult::Sql(statements) = result else {
        panic!("expected SQL")
    };
    assert_eq!(statements.len(), 5);
    assert!(statements[2].starts_with("INSERT INTO \"model_profiles\""));
}

/// Verifies a later failed statement removes the new table, reference column, and inserted preset.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn profile_failure_rolls_back_schema_and_presets() {
    with_database(|config, mut observer| async move {
        let mut runner = directory(&config, &PROFILES, 1).await;
        apply(&mut runner, "0007_personas").await.expect("existing personas");
        let mut migration = fixture(&PROFILES, 1);
        migration.operations.insert(3, Operation::Statement {
            up: "SELECT 1 / 0".into(), down: Some("SELECT 1".into()),
        });
        save(&config, &migration).await;
        let error = apply(&mut runner, "0008_model_profiles").await.expect_err("failure after preset");
        let cause = execution(&error, "0008_model_profiles", "apply", 4, "SELECT");
        assert!(matches!(cause, ExecutorError::ExecuteDatabase(failure) if failure.code.as_deref() == Some("22012")));
        original_personas(&mut observer).await;
        records(&mut observer, &["0007_personas"]).await;
        lock_released(&config, "0007_personas").await;
    }).await;
}

/// Checks externally that rollback preserved preexisting data and removed all profile schema.
async fn original_personas(observer: &mut PgConnection) {
    rows(
        observer,
        "SELECT name FROM personas ORDER BY id",
        &["Alice", "Bob"],
    )
    .await;
    rows(
        observer,
        "SELECT COALESCE(to_regclass('public.model_profiles')::text, 'absent')",
        &["absent"],
    )
    .await;
    rows(observer, "SELECT column_name::text FROM information_schema.columns WHERE table_schema = 'public' AND table_name = 'personas' ORDER BY ordinal_position", &["id", "name"]).await;
}
