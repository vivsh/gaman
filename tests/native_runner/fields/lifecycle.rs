//! Forward, reverse, and ownership-only execution with real PostgreSQL constraints.

use super::support::*;

/// A generated field addition applies, rolls back, and reapplies without disturbing references.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn generated_field_addition_roundtrip() {
    with_database(|config, mut observer| async move {
        let (mut runner, next) = generated(&config).await;
        apply(&mut runner, &next.id).await.expect("apply generated add-field migration");
        preserved(&mut observer).await;
        rows(&mut observer, "SELECT id || ':' || auxiliary_output_limit::text FROM model_profiles ORDER BY id",
            &["external:2048", "preset:3072"]).await;
        verified(&mut runner).await;
        records(&mut observer, &["0001_profiles", &next.id]).await;
        apply(&mut runner, "0001_profiles").await.expect("rollback ownership before column removal");
        preserved(&mut observer).await;
        rows(&mut observer, "SELECT count(*)::text FROM information_schema.columns WHERE table_name = 'model_profiles' AND column_name = 'auxiliary_output_limit'", &["0"]).await;
        records(&mut observer, &["0001_profiles"]).await;
        apply(&mut runner, &next.id).await.expect("reapply unchanged migration");
        preserved(&mut observer).await;
        verified(&mut runner).await;
        records(&mut observer, &["0001_profiles", &next.id]).await;
    }).await;
}

/// Relinquishing ownership leaves the value and column intact and never fires an UPDATE trigger.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn ownership_guard_does_not_mutate_database_values() {
    with_database(|config, mut observer| async move {
        let (mut runner, next) = generated(&config).await;
        apply(&mut runner, &next.id).await.unwrap();
        sql(&mut observer, "CREATE FUNCTION reject_profile_update() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'unexpected physical update'; END $$; CREATE TRIGGER reject_profile_update BEFORE UPDATE ON model_profiles FOR EACH ROW EXECUTE FUNCTION reject_profile_update()").await;
        let release = release(&next);
        save(&config, &release).await;
        apply(&mut runner, &release.id).await.expect("checked read, not dummy UPDATE");
        rows(&mut observer, "SELECT id || ':' || auxiliary_output_limit::text FROM model_profiles ORDER BY id",
            &["external:2048", "preset:3072"]).await;
        preserved(&mut observer).await;
        records(&mut observer, &["0001_profiles", &next.id, &release.id]).await;
    }).await;
}
