use crate::dialects::Dialect;
use crate::migrations::Migration;

use super::Schema;

const TABLE_WITH_OPTIONS_YAML: &str = r#"
tables:
  users:
    name: users
    schema: null
    primary_key: null
    columns: []
    foreign_keys: []
    indexes: []
    constraints: []
    triggers: []
    options:
      header_raw: [UNLOGGED]
      trusted: true
"#;

/// Verifies authored YAML cannot set internal unmanaged table metadata.
#[test]
fn authored_yaml_rejects_internal_metadata() {
    let error = Schema::from_yaml_str(TABLE_WITH_OPTIONS_YAML, Dialect::Postgres)
        .expect_err("reserved metadata must fail");
    assert!(error.to_string().contains("unknown field `options`"));
}

/// Verifies authored JSON cannot set trusted opaque lifecycle metadata.
#[test]
fn authored_json_rejects_internal_metadata() {
    let input = r#"{
      "tables": {"users": {
        "name": "users", "schema": null, "primary_key": null,
        "columns": [], "foreign_keys": [], "indexes": [{
          "name": "users_expr_idx", "columns": [], "unique": false,
          "predicate": null,
          "opaque": {"raw": "CREATE INDEX users_expr_idx ON users ((lower(name)))", "trusted": true}
        }], "constraints": [], "triggers": []
      }}
    }"#;
    let error =
        Schema::from_json_str(input, Dialect::Postgres).expect_err("reserved metadata must fail");
    assert!(error.to_string().contains("unknown field `opaque`"));
}

/// Verifies authored YAML rejects accidental fields instead of silently ignoring them.
#[test]
fn authored_yaml_rejects_unknown_fields() {
    let input = r#"
tables:
  users:
    name: users
    columns: []
    typo_field: true
"#;

    let error =
        Schema::from_yaml_str(input, Dialect::Postgres).expect_err("unknown fields must fail");

    assert!(error.to_string().contains("unknown field `typo_field`"));
}

/// Verifies reserved words can still be used as authored entity names.
#[test]
fn authored_yaml_allows_reserved_words_as_entity_names() {
    let input = r#"
tables:
  raw:
    name: raw
    columns:
    - name: id
      type: integer
      primary_key: true
"#;

    let schema =
        Schema::from_yaml_str(input, Dialect::Postgres).expect("reserved name should load");

    assert!(schema.tables.contains_key("raw"));
}

/// Verifies migration history can retain accepted internal table metadata for replay.
#[test]
fn migration_yaml_accepts_internal_metadata() {
    let yaml = format!(
        "dependencies: []\noperations:\n- type: create_table\n  table:\n{}atomic: true\n",
        TABLE_WITH_OPTIONS_YAML
            .trim_start_matches("\ntables:\n  users:\n")
            .lines()
            .map(|line| format!("    {line}\n"))
            .collect::<String>()
    );
    Migration::from_yaml_str(&yaml).expect("migration metadata should deserialize");
}
