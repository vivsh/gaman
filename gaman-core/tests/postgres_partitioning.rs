use gaman_core::diff::{DiffEngine, DiffError};
use gaman_core::schema::{
    Operation, PostgresRangePartitioning, Schema, SchemaBuilder, SchemaValidationError,
    TableBuilder,
};
use gaman_core::{Dialect, Migration};

fn base_schema() -> Schema {
    SchemaBuilder::new(Dialect::Postgres)
        .table_def(
            TableBuilder::new("crawler_url")
                .column("url", "text", |column| column.not_null())
                .column("purge_at", "timestamp with time zone", |column| {
                    column.not_null()
                })
                .build(),
        )
        .build()
        .expect("base schema should prepare")
}

fn partitioned_schema() -> Schema {
    base_schema()
        .with_postgres_range_partitioning(
            "crawler_url",
            PostgresRangePartitioning::new("purge_at")
                .partition(
                    "crawler_url_2026_01",
                    "2026-01-01 00:00:00+00",
                    "2026-02-01 00:00:00+00",
                )
                .partition(
                    "crawler_url_2026_02",
                    "2026-02-01 00:00:00+00",
                    "2026-03-01 00:00:00+00",
                ),
        )
        .expect("partition metadata should be valid")
}

fn migration(operations: Vec<Operation>) -> Migration {
    Migration {
        id: "0001_partitioned_crawler_url".to_string(),
        dependencies: Vec::new(),
        operations,
        atomic: true,
    }
}

/// PostgreSQL renders the partitioned parent before two bounded monthly children.
#[test]
fn renders_range_partition_parent_and_children() {
    let schema = partitioned_schema();
    let operations = DiffEngine::new()
        .diff(&schema, &Schema::default(), &Dialect::Postgres)
        .expect("partition creation should diff");
    let sql = Dialect::Postgres
        .migration_to_sql(&migration(operations), &Schema::default())
        .expect("partition migration should render");

    assert_eq!(sql.len(), 3);
    assert!(sql[0].contains("PARTITION BY RANGE (\"purge_at\")"));
    assert!(sql[1].contains(
        "CREATE TABLE \"crawler_url_2026_01\" PARTITION OF \"crawler_url\" FOR VALUES FROM ('2026-01-01 00:00:00+00') TO ('2026-02-01 00:00:00+00')"
    ));
    assert!(sql[2].contains("CREATE TABLE \"crawler_url_2026_02\" PARTITION OF \"crawler_url\""));
}

/// Runtime schema serialization preserves parent and child partition identities and bounds.
#[test]
fn partition_metadata_round_trips_through_schema_serialization() {
    let schema = partitioned_schema();
    let yaml = serde_yaml::to_string(&schema).expect("schema should serialize");
    let decoded: Schema = serde_yaml::from_str(&yaml).expect("schema should deserialize");
    assert_eq!(decoded, schema);
}

/// Non-PostgreSQL dialects reject range partition metadata with a typed capability error.
#[test]
fn unsupported_dialects_reject_postgres_partition_metadata() {
    for dialect in [Dialect::Sqlite, Dialect::Mysql, Dialect::Mariadb] {
        let error = partitioned_schema()
            .prepare(dialect)
            .expect_err("non-PostgreSQL dialect should reject partitions");
        assert!(matches!(
            error,
            SchemaValidationError::UnsupportedDialectFeature { .. }
        ));
    }
}

/// Creation and removal diffs replay to the requested schema in dependency-safe order.
#[test]
fn partition_creation_and_removal_diff_and_replay() {
    let schema = partitioned_schema();
    let create = DiffEngine::new()
        .diff(&schema, &Schema::default(), &Dialect::Postgres)
        .expect("creation should diff");
    assert_eq!(create.len(), 3);
    let mut replayed = Schema::default();
    for operation in &create {
        replayed.apply(operation).expect("creation should replay");
    }
    assert_eq!(replayed, schema);

    let drop = DiffEngine::new()
        .diff(&Schema::default(), &schema, &Dialect::Postgres)
        .expect("removal should diff");
    assert_eq!(drop.len(), 3);
    assert!(drop[..2].iter().all(|operation| {
        matches!(operation, Operation::DropTable { table } if table.postgres_range_partition_child().is_some())
    }));
    assert!(matches!(&drop[2], Operation::DropTable { table } if table.name == "crawler_url"));
    for operation in &drop {
        replayed.apply(operation).expect("removal should replay");
    }
    assert_eq!(replayed, Schema::default());
}

/// Existing plain tables cannot be converted to partitioned tables by generated DDL.
#[test]
fn plain_to_partitioned_transition_requires_raw_sql() {
    let partitioned = partitioned_schema();
    let error = DiffEngine::new()
        .diff(&partitioned, &base_schema(), &Dialect::Postgres)
        .expect_err("plain table conversion must be rejected");
    assert!(matches!(error, DiffError::PostgresPartitionMutation(_)));
}

/// A downstream consumer can extend an already-built schema through public APIs only.
#[test]
fn existing_schema_can_be_extended_without_internal_metadata_access() {
    let schema = SchemaBuilder::new(Dialect::Postgres)
        .table_def(
            TableBuilder::new("events")
                .column("created_at", "timestamp", |column| column.not_null())
                .build(),
        )
        .build()
        .expect("model-derived schema");
    let schema = schema
        .with_postgres_range_partitioning(
            "events",
            PostgresRangePartitioning::new("created_at").partition(
                "events_2026_01",
                "2026-01-01",
                "2026-02-01",
            ),
        )
        .expect("public extension API");
    assert_eq!(schema.tables.len(), 2);
}
