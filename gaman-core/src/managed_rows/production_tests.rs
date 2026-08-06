use std::collections::{BTreeMap, VecDeque};
use std::future::Future;

use serde::Serialize;

use super::{ManagedRow, ManagedRows, ManagedValue, diff_schemas};
use crate::dialects::Dialect;
use crate::migration_engine::{BoxFuture, Executor, ExecutorError};
use crate::operations::Operation;
use crate::states::{EntityKind, Schema, SchemaBuilder};

fn row(values: &[(&str, serde_json::Value)]) -> ManagedRow {
    ManagedRow {
        values: values
            .iter()
            .map(|(name, value)| ((*name).to_string(), ManagedValue(value.clone())))
            .collect(),
    }
}

fn declaration(rows: Vec<ManagedRow>) -> ManagedRows {
    ManagedRows { rows }
}

/// Builds and validates one SQL-backed schema with a managed-row declaration.
fn schema_with_rows(sql: &str, table: &str, rows: Vec<ManagedRow>) -> Result<Schema, String> {
    let table_def = Schema::from_sql_str(sql, Dialect::Postgres)
        .map_err(|error| error.to_string())?
        .tables
        .get(table)
        .cloned()
        .ok_or_else(|| "table missing from test schema".to_string())?;
    let mut schema = SchemaBuilder::new(Dialect::Postgres)
        .table_def(table_def)
        .build()
        .map_err(|error| error.to_string())?;
    schema
        .managed_rows
        .insert(table.to_string(), declaration(rows));
    schema
        .prepare(Dialect::Postgres)
        .map_err(|error| error.to_string())
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime")
        .block_on(future)
}

#[derive(Serialize)]
struct SupportedValues {
    id: String,
    null_value: Option<String>,
    enabled: bool,
    signed: i64,
    unsigned: u64,
    finite: f64,
    text: String,
    bytes: Vec<u8>,
    sequence: Vec<i32>,
    object: BTreeMap<String, bool>,
}

/// Verifies every documented Serde value has a deterministic JSON-compatible
/// representation, including byte buffers as numeric sequences.
#[test]
fn serde_value_matrix_is_canonical() {
    let record = SupportedValues {
        id: "all-values".to_string(),
        null_value: None,
        enabled: true,
        signed: -9,
        unsigned: u64::MAX,
        finite: 1.25,
        text: "quoted ' value".to_string(),
        bytes: vec![0, 127, 255],
        sequence: vec![1, 2],
        object: BTreeMap::from([("nested".to_string(), true)]),
    };
    let first = ManagedRow::from_serializable(&record).expect("serialize supported values");
    let second = ManagedRow::from_serializable(&record).expect("serialize supported values again");

    assert_eq!(first, second);
    assert_eq!(first.values["bytes"].0, serde_json::json!([0, 127, 255]));
    assert_eq!(first.values["unsigned"].0, serde_json::json!(u64::MAX));
    assert_eq!(
        first.values["object"].0,
        serde_json::json!({"nested": true})
    );
}

/// Verifies non-finite floats and non-string map keys fail during Serde
/// conversion instead of entering managed schema state.
#[test]
fn serde_rejects_unsupported_values() {
    #[derive(Serialize)]
    struct InvalidFloat {
        id: &'static str,
        value: f64,
    }
    #[derive(Serialize)]
    struct InvalidMap {
        id: &'static str,
        value: BTreeMap<(u8, u8), bool>,
    }

    assert!(
        ManagedRow::from_serializable(&InvalidFloat {
            id: "invalid",
            value: f64::NAN,
        })
        .is_err()
    );
    assert!(
        ManagedRow::from_serializable(&InvalidMap {
            id: "invalid",
            value: BTreeMap::from([((1, 2), true)]),
        })
        .is_err()
    );
}

/// Verifies composite primary and unique identities are inferred only when
/// every non-null key column is represented.
#[test]
fn composite_identity_matrix_is_enforced() {
    let primary_sql = "CREATE TABLE composite_items (tenant_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT, PRIMARY KEY (tenant_id, id))";
    let primary = schema_with_rows(
        primary_sql,
        "composite_items",
        vec![row(&[
            ("tenant_id", serde_json::json!("acme")),
            ("id", serde_json::json!("one")),
        ])],
    )
    .expect("composite primary key");
    assert_eq!(
        super::validation::resolve_key(
            &primary.tables["composite_items"],
            &primary.managed_rows["composite_items"]
        ),
        Some(vec!["tenant_id".to_string(), "id".to_string()])
    );

    let unique_sql = "CREATE TABLE composite_items (tenant_id TEXT NOT NULL, slug TEXT NOT NULL, note TEXT, UNIQUE (tenant_id, slug))";
    assert!(
        schema_with_rows(
            unique_sql,
            "composite_items",
            vec![row(&[
                ("tenant_id", serde_json::json!("acme")),
                ("slug", serde_json::json!("first")),
            ])],
        )
        .is_ok()
    );
    assert!(
        schema_with_rows(
            unique_sql,
            "composite_items",
            vec![row(&[("tenant_id", serde_json::json!("acme"))])],
        )
        .is_err()
    );
}

/// Verifies nullable and partial unique definitions cannot establish managed
/// identity, while an eligible non-partial unique index can.
#[test]
fn unique_identity_eligibility_is_strict() {
    let nullable = "CREATE TABLE items (slug TEXT UNIQUE, note TEXT)";
    assert!(
        schema_with_rows(
            nullable,
            "items",
            vec![row(&[("slug", serde_json::json!("first"))])],
        )
        .is_err()
    );

    let partial = "CREATE TABLE items (slug TEXT NOT NULL, active BOOLEAN NOT NULL); CREATE UNIQUE INDEX items_slug_active ON items (slug) WHERE active";
    assert!(
        schema_with_rows(
            partial,
            "items",
            vec![row(&[
                ("slug", serde_json::json!("first")),
                ("active", serde_json::json!(true)),
            ])],
        )
        .is_err()
    );

    let eligible = "CREATE TABLE items (slug TEXT NOT NULL, note TEXT); CREATE UNIQUE INDEX items_slug_unique ON items (slug)";
    assert!(
        schema_with_rows(
            eligible,
            "items",
            vec![row(&[("slug", serde_json::json!("first"))])],
        )
        .is_ok()
    );
}

/// Verifies empty, null-key, duplicate, shape-mismatched, and unknown-table
/// declarations are rejected at final schema validation.
#[test]
fn invalid_declaration_matrix_is_rejected() {
    let sql = "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT)";
    assert!(schema_with_rows(sql, "items", Vec::new()).is_err());
    assert!(
        schema_with_rows(sql, "items", vec![row(&[("id", serde_json::Value::Null)])],).is_err()
    );
    let duplicate = row(&[("id", serde_json::json!("same"))]);
    assert!(schema_with_rows(sql, "items", vec![duplicate.clone(), duplicate]).is_err());
    assert!(
        schema_with_rows(
            sql,
            "items",
            vec![
                row(&[("id", serde_json::json!("one"))]),
                row(&[
                    ("id", serde_json::json!("two")),
                    ("name", serde_json::json!("different-shape")),
                ]),
            ],
        )
        .is_err()
    );
    let mut unknown = Schema::default();
    unknown.managed_rows.insert(
        "missing".to_string(),
        declaration(vec![row(&[("id", serde_json::json!("one"))])]),
    );
    assert!(unknown.prepare(Dialect::Postgres).is_err());
}

/// Verifies unknown and generated columns and omitted required columns fail,
/// while omitted nullable/defaulted columns remain outside ownership.
#[test]
fn managed_column_boundary_is_validated() {
    let required = "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, external_note TEXT, revision INTEGER NOT NULL DEFAULT 0)";
    assert!(
        schema_with_rows(
            required,
            "items",
            vec![row(&[("id", serde_json::json!("one"))])],
        )
        .is_err()
    );
    assert!(
        schema_with_rows(
            required,
            "items",
            vec![row(&[
                ("id", serde_json::json!("one")),
                ("name", serde_json::json!("managed")),
            ])],
        )
        .is_ok()
    );
    assert!(
        schema_with_rows(
            required,
            "items",
            vec![row(&[
                ("id", serde_json::json!("one")),
                ("name", serde_json::json!("managed")),
                ("missing", serde_json::json!(true)),
            ])],
        )
        .is_err()
    );
    let generated = "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, slug TEXT GENERATED ALWAYS AS (lower(name)) STORED)";
    assert!(
        schema_with_rows(
            generated,
            "items",
            vec![row(&[
                ("id", serde_json::json!("one")),
                ("name", serde_json::json!("managed")),
                ("slug", serde_json::json!("managed")),
            ])],
        )
        .is_err()
    );
}

/// Verifies key changes are represented as insert plus delete and dropping the
/// represented table absorbs its managed rows without redundant deletes.
#[test]
fn key_changes_and_table_drops_have_exact_diff_semantics() {
    let sql = "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)";
    let previous = schema_with_rows(
        sql,
        "items",
        vec![row(&[
            ("id", serde_json::json!("old")),
            ("name", serde_json::json!("value")),
        ])],
    )
    .expect("previous schema");
    let desired = schema_with_rows(
        sql,
        "items",
        vec![row(&[
            ("id", serde_json::json!("new")),
            ("name", serde_json::json!("value")),
        ])],
    )
    .expect("desired schema");
    let operations = diff_schemas(&desired, &previous);
    assert_eq!(operations.len(), 2);
    assert!(
        operations
            .iter()
            .any(|operation| matches!(operation, Operation::InsertRow { .. }))
    );
    assert!(
        operations
            .iter()
            .any(|operation| matches!(operation, Operation::DeleteRow { .. }))
    );
    assert!(diff_schemas(&Schema::default(), &previous).is_empty());
}

/// Verifies compatible declaration merging is deterministic and conflicting
/// identities are rejected by final validation.
#[test]
fn declaration_composition_is_validated_after_merge() {
    let mut existing = declaration(vec![row(&[
        ("id", serde_json::json!("one")),
        ("name", serde_json::json!("first")),
    ])]);
    super::validation::merge_declaration(
        "items",
        &mut existing,
        declaration(vec![row(&[
            ("id", serde_json::json!("two")),
            ("name", serde_json::json!("second")),
        ])]),
    )
    .expect("compatible merge");
    let sql = "CREATE TABLE items (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL)";
    assert!(schema_with_rows(sql, "items", existing.rows).is_ok());

    let mut conflicting = declaration(vec![row(&[
        ("id", serde_json::json!("same")),
        ("name", serde_json::json!("first")),
    ])]);
    super::validation::merge_declaration(
        "items",
        &mut conflicting,
        declaration(vec![row(&[
            ("id", serde_json::json!("same")),
            ("name", serde_json::json!("second")),
        ])]),
    )
    .expect("shape-compatible merge");
    assert!(schema_with_rows(sql, "items", conflicting.rows).is_err());
}

#[derive(Default)]
struct RowQueryExecutor {
    responses: VecDeque<Result<Vec<String>, ExecutorError>>,
    queries: Vec<String>,
}

impl Executor for RowQueryExecutor {
    fn execute<'a>(&'a mut self, _: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        self.queries.push(sql.to_string());
        let response = self.responses.pop_front().unwrap_or_else(|| Ok(Vec::new()));
        Box::pin(async move { response })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

/// Returns the composite-key managed schema used by targeted drift tests.
fn expected_drift_schema() -> Schema {
    schema_with_rows(
        "CREATE TABLE composite_items (tenant_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, external_note TEXT, PRIMARY KEY (tenant_id, id))",
        "composite_items",
        vec![row(&[
            ("tenant_id", serde_json::json!("acme")),
            ("id", serde_json::json!("approval")),
            ("name", serde_json::json!("review")),
        ])],
    )
    .expect("expected drift schema")
}

/// Verifies matching managed values produce no drift and row reads target only
/// the declared composite identity and managed columns.
#[test]
fn targeted_verification_ignores_unmanaged_data() {
    block_on(async {
        let expected = expected_drift_schema();
        let mut executor = RowQueryExecutor {
            responses: VecDeque::from([Ok(vec![
                serde_json::json!({
                    "tenant_id": "acme",
                    "id": "approval",
                    "name": "review"
                })
                .to_string(),
            ])]),
            queries: Vec::new(),
        };
        let report = super::drift::verify(&expected, Dialect::Postgres, &mut executor)
            .await
            .expect("verify matching row");

        assert!(report.findings.is_empty());
        assert!(report.operations.is_empty());
        assert_eq!(executor.queries.len(), 1);
        assert!(executor.queries[0].contains("tenant_id"));
        assert!(executor.queries[0].contains("id"));
        assert!(!executor.queries[0].contains("external_note"));
    });
}

/// Verifies missing and changed observations produce deterministic row
/// identities and repairs based on observed old values.
#[test]
fn verification_projects_checked_repairs() {
    block_on(async {
        let expected = expected_drift_schema();
        let mut missing = RowQueryExecutor {
            responses: VecDeque::from([Ok(Vec::new())]),
            queries: Vec::new(),
        };
        let missing_report = super::drift::verify(&expected, Dialect::Postgres, &mut missing)
            .await
            .expect("verify missing row");
        assert_eq!(missing_report.findings[0].entity_kind, EntityKind::Row);
        assert!(
            missing_report.findings[0]
                .entity_name
                .contains("tenant_id=\"acme\",id=\"approval\"")
        );
        assert!(matches!(
            missing_report.operations[0],
            Operation::InsertRow { .. }
        ));

        let observed = serde_json::json!({
            "tenant_id": "acme",
            "id": "approval",
            "name": "tampered"
        });
        let mut changed = RowQueryExecutor {
            responses: VecDeque::from([Ok(vec![observed.to_string()])]),
            queries: Vec::new(),
        };
        let changed_report = super::drift::verify(&expected, Dialect::Postgres, &mut changed)
            .await
            .expect("verify changed row");
        let Operation::UpdateRow { old, new, .. } = &changed_report.operations[0] else {
            panic!("expected checked update repair");
        };
        assert_eq!(old.values["name"].0, serde_json::json!("tampered"));
        assert_eq!(new.values["name"].0, serde_json::json!("review"));
    });
}

/// Verifies malformed, duplicate, and failed targeted reads are reported as
/// executor failures instead of being converted into repair operations.
#[test]
fn verification_rejects_invalid_observations() {
    block_on(async {
        let expected = expected_drift_schema();
        for response in [
            Ok(vec!["not-json".to_string()]),
            Ok(vec!["{}".to_string(), "{}".to_string()]),
            Err(ExecutorError::Fetch("query failed".to_string())),
        ] {
            let mut executor = RowQueryExecutor {
                responses: VecDeque::from([response]),
                queries: Vec::new(),
            };
            assert!(
                super::drift::verify(&expected, Dialect::Postgres, &mut executor)
                    .await
                    .is_err()
            );
        }
    });
}

/// Verifies SQL rendering updates only changed columns while retaining the
/// complete expected old managed state in the predicate.
#[test]
fn update_sql_owns_only_changed_columns() {
    let old = row(&[
        ("tenant_id", serde_json::json!("acme")),
        ("id", serde_json::json!("approval")),
        ("name", serde_json::json!("review")),
        ("properties", serde_json::json!({"manager": true})),
    ]);
    let mut new = old.clone();
    new.values.insert(
        "name".to_string(),
        ManagedValue(serde_json::json!("senior_review")),
    );
    let operation = Operation::UpdateRow {
        table_name: "vyuh.task_lanes".to_string(),
        key: vec!["tenant_id".to_string(), "id".to_string()],
        old,
        new,
    };

    for dialect in [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ] {
        let sql = super::sql::render(dialect, &operation).expect("render checked update");
        let set = sql[0].split(" WHERE ").next().expect("update assignment");
        let predicate = sql[0].split(" WHERE ").nth(1).expect("update predicate");
        assert!(set.contains("name"));
        assert!(!set.contains("properties"));
        assert!(predicate.contains("tenant_id"));
        assert!(predicate.contains("id"));
        assert!(predicate.contains("properties"));
    }
}
