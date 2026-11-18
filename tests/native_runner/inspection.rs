//! Catalog inspection regressions for modeled and opaque PostgreSQL constraints.

use gaman::runner_factory::{LazyExecutor, NativeRunnerFactory};
use gaman_core::{Command, CommandResult, SchemaInspector};

use super::support::{sql, with_database};

/// Authored export models column nullability and does not duplicate ordinary catalog constraints.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn ordinary_not_null_exports_as_column_nullability() {
    with_database(|config, mut observer| async move {
        sql(
            &mut observer,
            "CREATE TABLE items (id integer PRIMARY KEY, required text NOT NULL, \
             named text CONSTRAINT named_not_null NOT NULL, optional text); \
             ALTER TABLE items RENAME COLUMN required TO label;",
        )
        .await;
        let result = NativeRunnerFactory::from_directory(config)
            .build()
            .run_command(&Command::Inspect {
                schemas: vec!["public".into()],
                filters: vec![],
                table: None,
            })
            .await
            .expect("ordinary NOT NULL is exportable");
        let CommandResult::Inspect(schema) = result else {
            panic!("expected inspected schema")
        };
        let table = &schema.tables["items"];
        assert!(table.constraints.is_empty());
        assert_eq!(
            table
                .columns
                .iter()
                .map(|column| column.nullable)
                .collect::<Vec<_>>(),
            [false, false, false, true]
        );
    })
    .await;
}

/// Exclusion constraints and nonstandard NOT NULL constraints remain opaque and block export.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn unsupported_constraint_metadata_remains_opaque() {
    with_database(|config, mut observer| async move {
        let version: i32 = sqlx::query_scalar("SELECT current_setting('server_version_num')::int")
            .fetch_one(&mut observer)
            .await
            .expect("server version");
        sql(&mut observer, "CREATE TABLE items (span int4range, CONSTRAINT no_overlap EXCLUDE USING gist (span WITH &&), ignored int, local_only int); ").await;
        let mut expected = vec!["no_overlap"];
        if version >= 180000 {
            sql(&mut observer, "ALTER TABLE items ADD CONSTRAINT unchecked_nn NOT NULL ignored NOT VALID; ALTER TABLE items ADD CONSTRAINT local_nn NOT NULL local_only NO INHERIT;").await;
            expected.extend(["local_nn", "unchecked_nn"]);
        }
        expected.sort_unstable();
        let schema = LazyExecutor::new(config.clone()).inspect(&["public"]).await.expect("raw catalog inspection");
        let constraints = &schema.tables["items"].constraints;
        assert!(constraints.iter().all(|constraint| constraint.is_opaque()));
        assert_eq!(constraints.iter().map(|constraint| constraint.name()).collect::<Vec<_>>(), expected);
        let result = NativeRunnerFactory::from_directory(config).build().run_command(&Command::Inspect {
            schemas: vec!["public".into()], filters: vec![], table: None,
        }).await;
        assert!(result.expect_err("opaque constraints cannot be exported").to_string().contains("opaque owned entity"));
    }).await;
}
