use std::collections::BTreeMap;

use serde::Serialize;

use super::{ManagedRow, ManagedRows, ManagedValue, apply_operation, diff_schemas};
use crate::dialects::Dialect;
use crate::operations::Operation;
use crate::states::{Schema, SchemaBuilder};

fn row(values: &[(&str, serde_json::Value)]) -> ManagedRow {
    ManagedRow {
        values: values
            .iter()
            .map(|(name, value)| ((*name).to_string(), ManagedValue(value.clone())))
            .collect::<BTreeMap<_, _>>(),
    }
}

/// Verifies Serde records become canonical, string-keyed managed rows.
#[test]
fn serializable_records_have_stable_identity() {
    #[derive(Serialize)]
    struct Lane<'a> {
        id: &'a str,
        enabled: bool,
        properties: serde_json::Value,
    }

    let rows = ManagedRows::from_serializable([Lane {
        id: "approval",
        enabled: true,
        properties: serde_json::json!({"manager": true}),
    }])
    .expect("record should serialize");

    assert_eq!(
        rows.rows[0].identity(&["id".to_string()]).as_deref(),
        Ok("id=\"approval\"")
    );
    assert_eq!(
        rows.rows[0].values.keys().cloned().collect::<Vec<_>>(),
        vec!["enabled", "id", "properties"]
    );
}

/// Verifies row operations are deterministic and have exact inverses.
#[test]
fn diff_and_replay_are_reversible() {
    let old = row(&[
        ("id", serde_json::json!("approval")),
        ("name", serde_json::json!("review")),
    ]);
    let new = row(&[
        ("id", serde_json::json!("approval")),
        ("name", serde_json::json!("senior_review")),
    ]);
    let mut previous = Schema::from_sql_str(
        "CREATE TABLE task_lanes (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)",
        Dialect::Sqlite,
    )
    .expect("table schema");
    previous.managed_rows.insert(
        "task_lanes".to_string(),
        ManagedRows {
            rows: vec![old.clone()],
        },
    );
    let mut desired = previous.clone();
    desired
        .managed_rows
        .get_mut("task_lanes")
        .expect("declaration")
        .rows[0] = new.clone();

    let operations = diff_schemas(&desired, &previous);
    assert_eq!(operations.len(), 1);
    assert!(matches!(operations[0], Operation::UpdateRow { .. }));
    apply_operation(&mut previous, &operations[0]).expect("update should replay");
    assert_eq!(previous.managed_rows, desired.managed_rows);
    apply_operation(&mut previous, &operations[0].inverse().expect("inverse"))
        .expect("inverse should replay");
    assert_eq!(previous.managed_rows["task_lanes"].rows, vec![old]);
}

/// Verifies every supported dialect renders strict old-state predicates.
#[test]
fn sql_rendering_is_exact_for_all_dialects() {
    let old = row(&[
        ("id", serde_json::json!("approval")),
        ("name", serde_json::Value::Null),
    ]);
    let new = row(&[
        ("id", serde_json::json!("approval")),
        ("name", serde_json::json!("review")),
    ]);
    let operation = Operation::UpdateRow {
        table_name: "vyuh.task_lanes".to_string(),
        key: vec!["id".to_string()],
        old,
        new,
    };

    let postgres = super::sql::render(Dialect::Postgres, &operation).expect("postgres");
    let sqlite = super::sql::render(Dialect::Sqlite, &operation).expect("sqlite");
    let mysql = super::sql::render(Dialect::Mysql, &operation).expect("mysql");
    let mariadb = super::sql::render(Dialect::Mariadb, &operation).expect("mariadb");
    assert_eq!(
        postgres[0],
        "UPDATE \"vyuh\".\"task_lanes\" SET \"name\" = 'review' WHERE \"id\" IS NOT DISTINCT FROM 'approval' AND \"name\" IS NOT DISTINCT FROM NULL"
    );
    assert_eq!(
        sqlite[0],
        "UPDATE \"vyuh\".\"task_lanes\" SET \"name\" = 'review' WHERE \"id\" IS 'approval' AND \"name\" IS NULL"
    );
    assert_eq!(
        mysql[0],
        "UPDATE `vyuh`.`task_lanes` SET `name` = 'review' WHERE `id` <=> 'approval' AND `name` <=> NULL"
    );
    assert_eq!(mariadb, mysql);
}

/// Verifies Rust declarations may precede or follow a table contribution.
#[test]
fn builder_resolves_managed_rows_after_complete_composition() {
    #[derive(Clone, Serialize)]
    struct Lane<'a> {
        id: &'a str,
        name: &'a str,
    }
    let table_schema = Schema::from_sql_str(
        "CREATE TABLE task_lanes (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)",
        Dialect::Sqlite,
    )
    .expect("table SQL");
    let table = table_schema.tables["task_lanes"].clone();
    let lane = Lane {
        id: "approval",
        name: "review",
    };

    let before = SchemaBuilder::new(Dialect::Sqlite)
        .managed_rows("task_lanes", [lane.clone()])
        .table_def(table.clone())
        .build()
        .expect("rows before table");
    let after = SchemaBuilder::new(Dialect::Sqlite)
        .table_def(table)
        .managed_rows("task_lanes", [lane])
        .build()
        .expect("rows after table");
    assert_eq!(before, after);
}

/// Verifies checked writes distinguish stale state from broken uniqueness.
#[test]
fn affected_row_contract_requires_exactly_one() {
    assert!(super::ensure_one_affected(1).is_ok());
    assert!(
        super::ensure_one_affected(0)
            .expect_err("zero must fail")
            .to_string()
            .contains("precondition")
    );
    assert!(
        super::ensure_one_affected(2)
            .expect_err("many must fail")
            .to_string()
            .contains("integrity")
    );
}

/// Verifies duplicate module contributions fail at the terminal builder boundary.
#[test]
fn duplicate_declarations_are_rejected() {
    #[derive(Clone, Serialize)]
    struct Lane<'a> {
        id: &'a str,
        name: &'a str,
    }
    let table = Schema::from_sql_str(
        "CREATE TABLE task_lanes (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)",
        Dialect::Sqlite,
    )
    .expect("table SQL")
    .tables["task_lanes"]
        .clone();
    let lane = Lane {
        id: "approval",
        name: "review",
    };
    let result = SchemaBuilder::new(Dialect::Sqlite)
        .managed_rows("task_lanes", [lane.clone()])
        .managed_rows("task_lanes", [lane])
        .table_def(table)
        .build();

    assert!(result.is_err());
}

fn insert(table: &str, id: &str) -> Operation {
    Operation::InsertRow {
        table_name: table.to_string(),
        key: vec!["id".to_string()],
        row: row(&[("id", serde_json::json!(id))]),
    }
}

/// Verifies parent inserts precede children and child deletes precede parents.
#[test]
fn foreign_keys_order_managed_row_writes() {
    let schema = Schema::from_sql_str(
        "CREATE TABLE parents (id TEXT PRIMARY KEY NOT NULL);\n\
         CREATE TABLE children (id TEXT PRIMARY KEY NOT NULL, parent_id TEXT NOT NULL,\
         CONSTRAINT children_parent_fk FOREIGN KEY (parent_id) REFERENCES parents(id));",
        Dialect::Postgres,
    )
    .expect("foreign-key schema");
    let inserts = super::order_operations(
        vec![insert("children", "child"), insert("parents", "parent")],
        &schema,
        &Schema::default(),
    )
    .expect("insert order");
    assert_eq!(inserts[0].table_name(), Some("parents"));

    let deletes = super::order_operations(
        inserts
            .iter()
            .rev()
            .filter_map(Operation::inverse)
            .collect(),
        &Schema::default(),
        &schema,
    )
    .expect("delete order");
    assert_eq!(deletes[0].table_name(), Some("children"));
}

/// Verifies cyclic row dependencies fail instead of selecting an unsafe order.
#[test]
fn foreign_key_cycles_are_rejected() {
    let schema = Schema::from_sql_str(
        "CREATE TABLE first_rows (id TEXT PRIMARY KEY NOT NULL, second_id TEXT NOT NULL,\
         CONSTRAINT first_second_fk FOREIGN KEY (second_id) REFERENCES second_rows(id));\n\
         CREATE TABLE second_rows (id TEXT PRIMARY KEY NOT NULL, first_id TEXT NOT NULL,\
         CONSTRAINT second_first_fk FOREIGN KEY (first_id) REFERENCES first_rows(id));",
        Dialect::Postgres,
    )
    .expect("cyclic schema");
    let result = super::order_operations(
        vec![
            insert("first_rows", "first"),
            insert("second_rows", "second"),
        ],
        &schema,
        &Schema::default(),
    );

    assert!(matches!(
        result,
        Err(crate::diff::DiffError::DependencyCycle)
    ));
}

/// Verifies declaration-level key metadata is rejected rather than ignored.
#[test]
fn authored_key_field_is_rejected() {
    let schema = r#"{
        "tables": {
            "items": {
                "columns": [
                    {"name": "id", "type": "text", "primary_key": true, "nullable": false}
                ]
            }
        },
        "managed_rows": {
            "items": {"key": ["id"], "rows": [{"id": "managed"}]}
        }
    }"#;

    assert!(Schema::from_json_str(schema, Dialect::Postgres).is_err());
}

/// Verifies primary keys are preferred and unique keys are used when the PK is absent.
#[test]
fn identity_is_inferred_from_table_constraints() {
    #[derive(Serialize)]
    struct Full<'a> {
        id: &'a str,
        slug: &'a str,
    }
    #[derive(Serialize)]
    struct BySlug<'a> {
        slug: &'a str,
    }
    let table = Schema::from_sql_str(
        "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL DEFAULT 'generated', \
         slug TEXT NOT NULL UNIQUE, external_note TEXT);",
        Dialect::Postgres,
    )
    .expect("table schema")
    .tables["items"]
        .clone();
    let primary = SchemaBuilder::new(Dialect::Postgres)
        .table_def(table.clone())
        .managed_rows(
            "items",
            [Full {
                id: "one",
                slug: "first",
            }],
        )
        .build()
        .expect("primary identity");
    let unique = SchemaBuilder::new(Dialect::Postgres)
        .table_def(table)
        .managed_rows("items", [BySlug { slug: "second" }])
        .build()
        .expect("unique identity");

    assert_eq!(
        super::validation::resolve_key(&primary.tables["items"], &primary.managed_rows["items"]),
        Some(vec!["id".to_string()])
    );
    assert_eq!(
        super::validation::resolve_key(&unique.tables["items"], &unique.managed_rows["items"]),
        Some(vec!["slug".to_string()])
    );
}

/// Verifies a declaration without any represented PK or unique key is invalid.
#[test]
fn declaration_without_table_identity_is_rejected() {
    #[derive(Serialize)]
    struct Invalid<'a> {
        name: &'a str,
    }
    let table = Schema::from_sql_str(
        "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL);",
        Dialect::Postgres,
    )
    .expect("table schema")
    .tables["items"]
        .clone();

    assert!(
        SchemaBuilder::new(Dialect::Postgres)
            .table_def(table)
            .managed_rows("items", [Invalid { name: "missing-id" }])
            .build()
            .is_err()
    );
}

/// Verifies managed rows own only declared identities and declared columns.
#[test]
fn managed_and_unmanaged_data_coexist_through_insert_update_and_delete() {
    #[derive(Clone, Serialize)]
    struct Desired<'a> {
        id: &'a str,
        name: &'a str,
    }
    let table = Schema::from_sql_str(
        "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, \
         external_note TEXT, revision INTEGER NOT NULL DEFAULT 0);",
        Dialect::Sqlite,
    )
    .expect("table schema")
    .tables["items"]
        .clone();
    let table_only = SchemaBuilder::new(Dialect::Sqlite)
        .table_def(table.clone())
        .build()
        .expect("table-only state");
    let inserted = SchemaBuilder::new(Dialect::Sqlite)
        .table_def(table.clone())
        .managed_rows(
            "items",
            [Desired {
                id: "managed",
                name: "first",
            }],
        )
        .build()
        .expect("subset declaration");
    let updated = SchemaBuilder::new(Dialect::Sqlite)
        .table_def(table)
        .managed_rows(
            "items",
            [Desired {
                id: "managed",
                name: "second",
            }],
        )
        .build()
        .expect("updated declaration");
    let mut database = BTreeMap::from([(
        "external".to_string(),
        BTreeMap::from([
            ("id".to_string(), serde_json::json!("external")),
            ("name".to_string(), serde_json::json!("outside")),
            (
                "external_note".to_string(),
                serde_json::json!("keep-external"),
            ),
            ("revision".to_string(), serde_json::json!(7)),
        ]),
    )]);

    apply_data_operations(&mut database, &diff_schemas(&inserted, &table_only));
    database.get_mut("managed").expect("managed insert").insert(
        "external_note".to_string(),
        serde_json::json!("keep-managed"),
    );
    apply_data_operations(&mut database, &diff_schemas(&updated, &inserted));
    assert_eq!(database["managed"]["name"], serde_json::json!("second"));
    assert_eq!(
        database["managed"]["external_note"],
        serde_json::json!("keep-managed")
    );
    assert_eq!(database["external"]["name"], serde_json::json!("outside"));

    apply_data_operations(&mut database, &diff_schemas(&table_only, &updated));
    assert!(!database.contains_key("managed"));
    assert_eq!(database.len(), 1);
    assert_eq!(
        database["external"]["external_note"],
        serde_json::json!("keep-external")
    );
}

fn apply_data_operations(
    database: &mut BTreeMap<String, BTreeMap<String, serde_json::Value>>,
    operations: &[Operation],
) {
    for operation in operations {
        match operation {
            Operation::InsertRow { row, .. } => {
                let id = row.values["id"].0.as_str().expect("string id").to_string();
                database.insert(
                    id,
                    row.values
                        .iter()
                        .map(|(name, value)| (name.clone(), value.0.clone()))
                        .collect(),
                );
            }
            Operation::UpdateRow { old, new, .. } => {
                let id = old.values["id"].0.as_str().expect("string id");
                let stored = database.get_mut(id).expect("managed update target");
                for (name, value) in &new.values {
                    stored.insert(name.clone(), value.0.clone());
                }
            }
            Operation::DeleteRow { row, .. } => {
                let id = row.values["id"].0.as_str().expect("string id");
                database.remove(id);
            }
            _ => {}
        }
    }
}
