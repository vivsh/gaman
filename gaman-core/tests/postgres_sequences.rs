use gaman_core::Dialect;
use gaman_core::diff::{generate_diff, sort_operations};
use gaman_core::schema::{Operation, Schema, SchemaBuilder};

/// Verifies structured YAML preserves one canonical opaque sequence definition.
#[test]
fn sequence_yaml_round_trips() {
    let schema = Schema::from_yaml_str(
        "sequences:\n  audit.event_ids:\n    sql: CREATE SEQUENCE audit.event_ids START WITH 100 INCREMENT BY 5\n",
        Dialect::Postgres,
    )
    .expect("sequence YAML should prepare");
    let sequence = schema
        .sequences
        .get("audit.event_ids")
        .expect("qualified sequence should exist");
    assert_eq!(sequence.name, "event_ids");
    assert_eq!(sequence.schema.as_deref(), Some("audit"));

    let encoded = serde_yaml::to_string(&schema).expect("sequence schema should serialize");
    let decoded = Schema::from_yaml_str(&encoded, Dialect::Postgres)
        .expect("serialized sequence schema should reload");
    assert_eq!(decoded, schema);
}

/// Verifies JSON, map-key inference, and Rust opaque declarations converge on one identity.
#[test]
fn sequence_input_frontends_converge() {
    let yaml = Schema::from_yaml_str(
        "sequences:\n  audit.event_ids:\n    sql: CREATE SEQUENCE audit.event_ids\n",
        Dialect::Postgres,
    )
    .expect("YAML sequence should prepare");
    let json = Schema::from_json_str(
        r#"{"sequences":{"audit.event_ids":{"sql":"CREATE SEQUENCE audit.event_ids"}}}"#,
        Dialect::Postgres,
    )
    .expect("JSON sequence should prepare");
    let builder = SchemaBuilder::new(Dialect::Postgres)
        .opaque("CREATE SEQUENCE audit.event_ids")
        .build()
        .expect("Rust opaque sequence should prepare");

    assert_eq!(yaml.sequences, json.sequences);
    assert_eq!(json.sequences, builder.sequences);
}

/// Verifies formatting-only sequence source changes do not create lifecycle churn.
#[test]
fn sequence_source_formatting_is_a_noop() {
    let previous = Schema::from_sql_str(
        "CREATE SEQUENCE event_ids START WITH 100 INCREMENT BY 5",
        Dialect::Postgres,
    )
    .expect("baseline sequence should prepare");
    let desired = Schema::from_sql_str(
        "-- managed sequence\nCREATE   SEQUENCE event_ids START WITH 100 INCREMENT BY 5;",
        Dialect::Postgres,
    )
    .expect("reformatted sequence should prepare");

    assert!(generate_diff(&desired, &previous).is_empty());
}

/// Verifies sequence option changes are explicit drop-and-create replacements.
#[test]
fn sequence_definition_change_replaces_the_root() {
    let previous =
        Schema::from_sql_str("CREATE SEQUENCE event_ids START WITH 1", Dialect::Postgres)
            .expect("baseline sequence should prepare");
    let desired = Schema::from_sql_str(
        "CREATE SEQUENCE event_ids START WITH 100",
        Dialect::Postgres,
    )
    .expect("changed sequence should prepare");

    let operations = sort_operations(generate_diff(&desired, &previous))
        .expect("sequence replacement should order");
    assert!(matches!(
        operations.as_slice(),
        [
            Operation::DropSequence { .. },
            Operation::CreateSequence { .. }
        ]
    ));
}

/// Verifies create and drop operations replay exactly and invert one another.
#[test]
fn sequence_operations_replay_and_invert() {
    let desired =
        Schema::from_sql_str("CREATE SEQUENCE event_ids START WITH 10", Dialect::Postgres)
            .expect("sequence SQL should prepare");
    let operations = generate_diff(&desired, &Schema::default());
    assert_eq!(operations.len(), 1);
    assert!(matches!(operations[0], Operation::CreateSequence { .. }));

    let inverse = operations[0]
        .inverse()
        .expect("sequence create should invert");
    let mut replay = Schema::default();
    replay
        .apply(&operations[0])
        .expect("sequence create should replay");
    assert_eq!(replay.sequences, desired.sequences);
    replay.apply(&inverse).expect("sequence drop should replay");
    assert!(replay.sequences.is_empty());
}

/// Verifies source order is irrelevant and sequence lifecycle surrounds table lifecycle.
#[test]
fn sequence_operations_are_dependency_ordered() {
    let desired = Schema::from_sql_str(
        "CREATE TABLE events (id bigint PRIMARY KEY);\nCREATE SEQUENCE event_ids;",
        Dialect::Postgres,
    )
    .expect("table and sequence SQL should prepare");
    let creates = sort_operations(generate_diff(&desired, &Schema::default()))
        .expect("create graph should sort");
    assert_eq!(creates[0].type_name(), "create_sequence");
    assert_eq!(creates[1].type_name(), "create_table");

    let drops = sort_operations(generate_diff(&Schema::default(), &desired))
        .expect("drop graph should sort");
    assert_eq!(drops[0].type_name(), "drop_table");
    assert_eq!(drops[1].type_name(), "drop_sequence");
}

/// Verifies sequences precede source-later functions and views without body parsing.
#[test]
fn sequence_precedes_functions_and_views_without_inference() {
    let desired = Schema::from_sql_str(
        "CREATE VIEW recent_events AS SELECT nextval('event_ids');\n\
         CREATE FUNCTION next_event() RETURNS bigint LANGUAGE sql AS $$ SELECT nextval('event_ids') $$;\n\
         CREATE SEQUENCE event_ids;",
        Dialect::Postgres,
    )
    .expect("sequence, function, and view should prepare");
    let operations = sort_operations(generate_diff(&desired, &Schema::default()))
        .expect("create graph should sort");
    let sequence = operations
        .iter()
        .position(|operation| matches!(operation, Operation::CreateSequence { .. }))
        .expect("sequence create should exist");
    let function = operations
        .iter()
        .position(|operation| matches!(operation, Operation::CreateFunction { .. }))
        .expect("function create should exist");
    let view = operations
        .iter()
        .position(|operation| matches!(operation, Operation::CreateView { .. }))
        .expect("view create should exist");

    assert!(sequence < function);
    assert!(sequence < view);
}

/// Verifies non-PostgreSQL and reverse-owned sequence definitions fail closed.
#[test]
fn unsupported_sequence_ownership_is_rejected() {
    let owned = Schema::from_sql_str(
        "CREATE SEQUENCE event_ids OWNED BY events.id",
        Dialect::Postgres,
    )
    .expect_err("OWNED BY must not enter opaque root state");
    assert!(owned.to_string().contains("OWNED BY"));

    let sqlite = Schema::from_sql_str("CREATE SEQUENCE event_ids", Dialect::Sqlite)
        .expect_err("SQLite sequence input must be rejected");
    assert!(sqlite.to_string().contains("PostgreSQL"));
}

/// Verifies temporary, replacement, and caller-owned lifecycle modifiers never enter state.
#[test]
fn unsupported_sequence_lifecycle_modifiers_are_rejected() {
    for sql in [
        "CREATE TEMP SEQUENCE event_ids",
        "CREATE SEQUENCE event_ids OWNED BY events.id",
        "CREATE SEQUENCE IF NOT EXISTS event_ids",
    ] {
        assert!(
            Schema::from_sql_str(sql, Dialect::Postgres).is_err(),
            "{sql}"
        );
    }
}

/// Verifies SQL composition rejects duplicate sequence identities instead of overwriting source.
#[test]
fn duplicate_sequence_identity_is_rejected() {
    let result = Schema::from_sql_str(
        "CREATE SEQUENCE event_ids; CREATE SEQUENCE event_ids START WITH 100;",
        Dialect::Postgres,
    );
    let error = result.expect_err("duplicate sequence identity must fail");
    assert!(error.to_string().contains("duplicate sequence 'event_ids'"));
}
