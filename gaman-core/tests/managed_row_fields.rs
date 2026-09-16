//! Ownership-only managed-row updates remain checked and reversible.

use gaman_core::managed_rows::{ManagedRow, ManagedValue};
use gaman_core::operations::Operation;
use gaman_core::sql_plan::SqlPlanRenderer;
use gaman_core::{Dialect, Migration, OfflinePlanner};
use serde_json::json;

const DIALECTS: [Dialect; 4] = [
    Dialect::Postgres,
    Dialect::Sqlite,
    Dialect::Mysql,
    Dialect::Mariadb,
];

fn baseline() -> Migration {
    let mut migration = Migration::from_yaml_str(include_str!("fixtures/managed_row_fields.yaml"))
        .expect("baseline");
    migration.id = "0001_profiles".into();
    migration
}

fn row(value: serde_json::Value) -> ManagedRow {
    ManagedRow::from_serializable(&value).expect("row")
}

fn update(old: serde_json::Value, new: serde_json::Value) -> Operation {
    Operation::UpdateRow {
        table_name: "model_profiles".into(),
        key: vec!["id".into()],
        old: row(old),
        new: row(new),
    }
}

/// Generates an actual add-column diff and plans both directions for every renderer.
#[test]
fn generated_field_addition_plans_forward_and_rollback() {
    for dialect in DIALECTS {
        let mut baseline = baseline();
        // SQLite deliberately refuses table rebuilds with inbound foreign keys.
        if dialect == Dialect::Sqlite {
            baseline.operations.truncate(4);
        }
        let planner = OfflinePlanner::new(dialect).from_migrations(vec![baseline.clone()]);
        let original = planner.replay().expect("baseline replay");
        let desired = with_field(original.clone());
        let migration = planner
            .make_named_migration(desired.clone(), Some("add_field"), &[])
            .expect("generate")
            .expect("changes");
        assert!(matches!(
            migration.operations[0],
            Operation::AddColumn { .. }
        ));
        assert!(matches!(
            migration.operations[1],
            Operation::UpdateRow { .. }
        ));
        let renderer = SqlPlanRenderer::new(dialect, vec![baseline, migration.clone()]).unwrap();
        let forward = renderer
            .render_migrations(&[migration.clone()])
            .expect("forward SQL");
        let reverse = renderer
            .render_rollback_migrations(&[migration.clone()])
            .expect("rollback SQL");
        assert!(forward.iter().any(|sql| sql.starts_with("UPDATE ")));
        assert!(reverse[0].starts_with("SELECT '1' FROM "), "{reverse:?}");
        assert_replay(dialect, original, desired, &migration);
    }
}

/// Adds a defaulted column and its desired managed value without modifying the old declaration.
fn with_field(mut schema: gaman_core::schema::Schema) -> gaman_core::schema::Schema {
    schema
        .tables
        .get_mut("model_profiles")
        .unwrap()
        .columns
        .push(
            serde_yaml::from_str("{name: auxiliary_output_limit, type: integer, default: '2048'}")
                .expect("defaulted column"),
        );
    schema.managed_rows.get_mut("model_profiles").unwrap().rows[0]
        .values
        .insert("auxiliary_output_limit".into(), ManagedValue(json!(2048)));
    schema
}

/// Compares both replay directions with the canonical desired and original states.
fn assert_replay(
    dialect: Dialect,
    original: gaman_core::schema::Schema,
    desired: gaman_core::schema::Schema,
    migration: &Migration,
) {
    let mut replay = original.clone();
    for op in &migration.operations {
        replay.apply(op).expect("forward replay");
    }
    assert_eq!(
        replay.clone().prepare(dialect).unwrap(),
        desired.prepare(dialect).unwrap()
    );
    for op in migration.operations.iter().rev() {
        replay.apply(&op.inverse().unwrap()).unwrap();
    }
    assert_eq!(replay.prepare(dialect).unwrap(), original);
}

/// Ownership-only SQL checks removed values and composite keys without issuing a write.
#[test]
fn field_removal_renders_a_complete_null_safe_guard() {
    for dialect in DIALECTS {
        let mut operation = update(
            json!({"id": "preset", "tenant": "acme", "limit": null}),
            json!({"id": "preset", "tenant": "acme"}),
        );
        if let Operation::UpdateRow {
            table_name, key, ..
        } = &mut operation
        {
            *table_name = "app.model_profiles".into();
            *key = vec!["tenant".into(), "id".into()];
        }
        let (quote, eq, lock) = match dialect {
            Dialect::Postgres => ('"', "IS NOT DISTINCT FROM", " FOR UPDATE"),
            Dialect::Sqlite => ('"', "IS", ""),
            Dialect::Mysql | Dialect::Mariadb => ('`', "<=>", " FOR UPDATE"),
        };
        let expected = format!(
            "SELECT '1' FROM {quote}app{quote}.{quote}model_profiles{quote} WHERE {quote}tenant{quote} {eq} 'acme' AND {quote}id{quote} {eq} 'preset' AND {quote}limit{quote} {eq} NULL{lock}"
        );
        assert_eq!(
            gaman_core::managed_rows::sql::render(dialect, &operation).unwrap(),
            vec![expected]
        );
    }
}

/// Empty updates and invalid or changed identities never become successful ownership guards.
#[test]
fn noops_and_invalid_keys_are_rejected() {
    for dialect in DIALECTS {
        let valid = json!({"id": "preset", "limit": 2048});
        let cases = [
            update(valid.clone(), valid.clone()),
            update(valid.clone(), json!({"limit": 2048})),
            update(json!({"limit": 2048}), json!({})),
            update(json!({"id": null, "limit": 2048}), json!({"id": null})),
            update(valid.clone(), json!({"id": "other"})),
        ];
        for operation in cases {
            assert!(
                gaman_core::managed_rows::sql::render(dialect, &operation).is_err(),
                "{operation:?}"
            );
        }
        let mut operation = update(valid, json!({"id": "preset"}));
        if let Operation::UpdateRow { key, .. } = &mut operation {
            key.clear();
        }
        assert!(gaman_core::managed_rows::sql::render(dialect, &operation).is_err());
    }
}

/// Explicit null assignments remain writes, and mixed removal/value changes set only changed fields.
#[test]
fn null_and_mixed_updates_keep_checked_dml() {
    for dialect in DIALECTS {
        let old = json!({"id": "preset", "name": "before", "limit": 2048});
        for new in [
            json!({"id": "preset", "name": "after"}),
            json!({"id": "preset", "name": "before", "limit": null}),
        ] {
            let sql =
                gaman_core::managed_rows::sql::render(dialect, &update(old.clone(), new)).unwrap();
            let (assignment, predicate) = sql[0].split_once(" WHERE ").unwrap();
            assert!(assignment.starts_with("UPDATE "));
            assert_eq!(assignment.matches(" = ").count(), 1);
            assert!(predicate.contains("2048"));
            assert!(predicate.contains("before"));
        }
    }
}

/// Replay requires the exact old declaration, and does not drop a column when ownership shrinks.
#[test]
fn replay_removes_ownership_only_and_rejects_stale_state() {
    let mut schema = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![baseline()])
        .replay()
        .unwrap();
    let operation = update(
        json!({"id": "preset", "name": "standard", "output_limit": 1024}),
        json!({"id": "preset", "name": "standard"}),
    );
    let mut stale = operation.clone();
    if let Operation::UpdateRow { old, .. } = &mut stale {
        old.values
            .insert("output_limit".into(), ManagedValue(json!(9)));
    }
    assert!(
        schema
            .apply(&stale)
            .unwrap_err()
            .to_string()
            .contains("expected old state")
    );
    let tables = schema.tables.clone();
    schema.apply(&operation).unwrap();
    assert_eq!(schema.tables, tables);
    assert!(
        !schema.managed_rows["model_profiles"].rows[0]
            .values
            .contains_key("output_limit")
    );
    schema.apply(&operation.inverse().unwrap()).unwrap();
    assert_eq!(
        schema.managed_rows["model_profiles"].rows[0].values["output_limit"],
        ManagedValue(json!(1024))
    );
}
