//! Ownership-only checks refuse missing, stale, and nonunique expected rows.

use super::super::support::execution;
use super::support::*;

/// Stale values block rollback, retain tracking, and leave the new column intact until corrected.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn rollback_guard_rejects_stale_expected_values() {
    with_database(|config, mut observer| async move {
        let (mut runner, next) = generated(&config).await;
        apply(&mut runner, &next.id).await.unwrap();
        sql(
            &mut observer,
            "UPDATE model_profiles SET auxiliary_output_limit = 999 WHERE id = 'preset'",
        )
        .await;
        let error = apply(&mut runner, "0001_profiles")
            .await
            .expect_err("stale rollback must fail");
        let cause = execution(&error, &next.id, "rollback", 1, "SELECT");
        assert!(cause.to_string().contains("precondition"), "{cause}");
        records(&mut observer, &["0001_profiles", &next.id]).await;
        rows(
            &mut observer,
            "SELECT auxiliary_output_limit::text FROM model_profiles WHERE id = 'preset'",
            &["999"],
        )
        .await;
        preserved(&mut observer).await;
        sql(
            &mut observer,
            "UPDATE model_profiles SET auxiliary_output_limit = 3072 WHERE id = 'preset'",
        )
        .await;
        apply(&mut runner, "0001_profiles")
            .await
            .expect("retry after correcting isolated drift");
        records(&mut observer, &["0001_profiles"]).await;
    })
    .await;
}

/// Zero or multiple matching rows fail the guard and roll back earlier DDL in the same migration.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn ownership_guard_requires_exactly_one_row() {
    for duplicate in [false, true] {
        with_database(move |config, mut observer| async move {
            let (mut runner, next) = generated(&config).await;
            apply(&mut runner, &next.id).await.unwrap();
            sql(&mut observer, "ALTER TABLE personas DROP CONSTRAINT personas_profile_fk").await;
            if duplicate {
                sql(&mut observer, "ALTER TABLE model_profiles DROP CONSTRAINT model_profiles_pkey; INSERT INTO model_profiles SELECT * FROM model_profiles WHERE id = 'preset'").await;
            } else {
                sql(&mut observer, "DELETE FROM model_profiles WHERE id = 'preset'").await;
            }
            let mut release = release(&next);
            release.operations.insert(0, serde_yaml::from_str(
                "type: statement\nup: CREATE TABLE must_rollback (id integer)\ndown: DROP TABLE must_rollback"
            ).unwrap());
            save(&config, &release).await;
            let error = apply(&mut runner, &release.id).await.expect_err("invalid cardinality");
            let cause = execution(&error, &release.id, "apply", 2, "SELECT");
            assert!(cause.to_string().contains(if duplicate { "integrity" } else { "precondition" }), "{cause}");
            records(&mut observer, &["0001_profiles", &next.id]).await;
            rows(&mut observer, "SELECT count(*)::text FROM information_schema.tables WHERE table_name = 'must_rollback'", &["0"]).await;
        }).await;
    }
}
