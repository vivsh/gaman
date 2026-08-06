use crate::dialects::Dialect;
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::Schema;

const MANAGED_SCHEMA: &str = r#"
tables:
  items:
    name: items
    columns:
      - name: tenant_id
        type: text
      - name: id
        type: text
      - name: label
        type: text
      - name: external_note
        type: text
        nullable: true
    primary_key:
      columns: [tenant_id, id]
managed_rows:
  items:
    rows:
      - tenant_id: acme
        id: approval
        label: manager_review
      - tenant_id: acme
        id: triage
        label: intake
"#;

/// Verifies managed rows survive canonical YAML and JSON schema serialization.
#[test]
fn schema_yaml_and_json_round_trip_managed_rows() {
    let schema =
        Schema::from_yaml_str(MANAGED_SCHEMA, Dialect::Postgres).expect("load managed schema");
    let yaml = serde_yaml::to_string(&schema).expect("serialize schema YAML");
    let from_yaml = Schema::from_yaml_str(&yaml, Dialect::Postgres).expect("reload schema YAML");
    let json = serde_json::to_string(&schema).expect("serialize schema JSON");
    let from_json = Schema::from_json_str(&json, Dialect::Postgres).expect("reload schema JSON");

    assert_eq!(from_yaml, schema);
    assert_eq!(from_json, schema);
}

/// Verifies every managed-row operation survives canonical migration YAML.
#[test]
fn migration_yaml_round_trips_all_managed_row_operations() {
    let schema =
        Schema::from_yaml_str(MANAGED_SCHEMA, Dialect::Postgres).expect("load managed schema");
    let rows = &schema.managed_rows["items"].rows;
    let migration = Migration {
        id: "0002_managed_items".to_string(),
        dependencies: vec!["0001_items".to_string()],
        operations: vec![
            Operation::InsertRow {
                table_name: "items".to_string(),
                key: vec!["tenant_id".to_string(), "id".to_string()],
                row: rows[0].clone(),
            },
            Operation::UpdateRow {
                table_name: "items".to_string(),
                key: vec!["tenant_id".to_string(), "id".to_string()],
                old: rows[0].clone(),
                new: rows[1].clone(),
            },
            Operation::DeleteRow {
                table_name: "items".to_string(),
                key: vec!["tenant_id".to_string(), "id".to_string()],
                row: rows[1].clone(),
            },
        ],
        atomic: true,
    };

    let yaml = migration.to_yaml_string().expect("serialize migration");
    let mut reloaded = Migration::from_yaml_str(&yaml).expect("reload migration");
    reloaded.id = migration.id.clone();

    assert_eq!(reloaded.dependencies, migration.dependencies);
    assert_eq!(reloaded.operations, migration.operations);
    assert_eq!(reloaded.atomic, migration.atomic);
}
