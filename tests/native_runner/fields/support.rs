//! Small generated fixtures shared by field-ownership integration scenarios.

use gaman::Config;
use gaman_core::managed_rows::ManagedValue;
use gaman_core::{Dialect, Migration, OfflinePlanner};
use sqlx::PgConnection;

pub(super) use super::super::support::{
    Runner, apply, records, rows, save, sql, verified, with_database,
};

/// Generates an add-column plus managed-row update, saved through the native directory store.
pub(super) async fn generated(config: &Config) -> (Runner, Migration) {
    let mut baseline = Migration::from_yaml_str(include_str!(
        "../../../gaman-core/tests/fixtures/managed_row_fields.yaml"
    ))
    .expect("baseline fixture");
    baseline.id = "0001_profiles".into();
    let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![baseline.clone()]);
    let mut desired = planner.replay().expect("baseline replay");
    desired
        .tables
        .get_mut("model_profiles")
        .unwrap()
        .columns
        .push(
            serde_yaml::from_str("{name: auxiliary_output_limit, type: integer, default: '2048'}")
                .unwrap(),
        );
    desired.managed_rows.get_mut("model_profiles").unwrap().rows[0]
        .values
        .insert("auxiliary_output_limit".into(), ManagedValue(3072.into()));
    let next = planner
        .make_named_migration(desired, Some("add_field"), &[])
        .expect("generate migration")
        .expect("changed schema");
    save(config, &baseline).await;
    save(config, &next).await;
    let runner = gaman::runner_factory::NativeRunnerFactory::from_directory(config.clone()).build();
    (runner, next)
}

/// Asserts both managed and never-managed records and their referencing personas survive.
pub(super) async fn preserved(observer: &mut PgConnection) {
    rows(observer,
        "SELECT p.id::text || ':' || m.id || ':' || m.name || ':' || m.output_limit::text || ':' || m.external_note FROM personas p JOIN model_profiles m ON m.id = p.model_profile_id ORDER BY p.id",
        &["1:preset:standard:1024:keep-managed", "2:external:outside:512:keep-external"],
    ).await;
}

/// Constructs the authored ownership-only inverse without dropping its database column.
pub(super) fn release(next: &Migration) -> Migration {
    let mut migration =
        Migration::from_yaml_str("id: 0003_release\natomic: true\noperations: []").unwrap();
    migration.id = "0003_release".into();
    migration.dependencies = vec![next.id.clone()];
    migration.operations = next
        .operations
        .iter()
        .filter(|op| matches!(op, gaman_core::operations::Operation::UpdateRow { .. }))
        .map(|op| op.inverse().unwrap())
        .collect();
    migration
}
