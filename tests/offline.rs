mod support;

use gaman::schema::Schema;

use support::{
    OfflineCase, OfflineSpec, TestSupportError, assert_error_contains, assert_ops_match,
    assert_schema_matches_with_dialect, assert_sql_matches, build_migrator, case_label, case_name,
    discover_cases, offline_cases_root, ordered_migrations, read_case_file, replay_schema,
};

/// Runs offline transform cases for SQL->schema, schema->migration, migration->replay, and migration->SQL.
#[test]
fn offline_cases() {
    let files = discover_cases(&offline_cases_root()).expect("failed to discover offline cases");
    let mut failures = Vec::new();

    for file in files {
        let name = match case_name(&file) {
            Ok(name) => name,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };

        let result = (|| -> Result<String, TestSupportError> {
            let case: OfflineCase = read_case_file(&file)?;
            let label = case_label(&name, case.description.as_deref());
            run_offline_case(&name, &case)?;
            Ok(label)
        })();

        match result {
            Ok(label) => println!("  ok: {label}"),
            Err(error) => failures.push(error.to_string()),
        }
    }

    if !failures.is_empty() {
        panic!("offline cases failed:\n\n{}", failures.join("\n\n"));
    }
}

fn run_offline_case(name: &str, case: &OfflineCase) -> Result<(), TestSupportError> {
    let dialect = case.dialect.to_dialect();
    match &case.spec {
        OfflineSpec::SqlToSchema { sql, expect_schema, expect_error } => {
            let result = Schema::from_sql_str(sql);
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let actual = result.map_err(|error| {
                TestSupportError::message(format!("{name}: sql_to_schema failed unexpectedly: {error}"))
            })?;
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!("{name}: sql_to_schema requires expect_schema when expect_error is absent"))
            })?;
            assert_schema_matches_with_dialect(name, "parsed schema", actual, expected, dialect)
        }
        OfflineSpec::SchemaToMigration {
            name: migration_name,
            migrations,
            current,
            decisions,
            expect_no_changes,
            expect_operations,
            expect_sql,
            expect_error,
        } => {
            let migrator = build_migrator(name, case.dialect, migrations)?;
            let result = migrator.make_migrations(Some(migration_name.clone()), current.clone(), true, decisions);
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let generated = result.map_err(|error| {
                TestSupportError::message(format!("{name}: schema_to_migration failed unexpectedly: {error}"))
            })?;

            if *expect_no_changes {
                if generated.is_some() {
                    return Err(TestSupportError::message(format!(
                        "{name}: expected no migration, but one was generated",
                    )));
                }
                return Ok(());
            }

            let generated = generated.ok_or_else(|| {
                TestSupportError::message(format!("{name}: expected a generated migration, but diff returned no changes"))
            })?;

            if let Some(expected) = expect_operations {
                assert_ops_match(name, "generated operations", &generated.operations, expected)?;
            }
            if let Some(expected) = expect_sql {
                let actual = migrator.sql_migrate(&[generated]).map_err(|error| {
                    TestSupportError::message(format!("{name}: failed to render generated SQL: {error}"))
                })?;
                assert_sql_matches(name, &actual, expected)?;
            }
            Ok(())
        }
        OfflineSpec::MigrationToReplay { migrations, expect_schema, expect_error } => {
            let result = build_migrator(name, case.dialect, migrations)
                .and_then(|migrator| replay_schema(name, &migrator));
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let actual = result?;
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!("{name}: migration_to_replay requires expect_schema when expect_error is absent"))
            })?;
            assert_schema_matches_with_dialect(name, "replayed schema", actual, expected, dialect)
        }
        OfflineSpec::MigrationToSql { migrations, expect_sql, expect_error } => {
            let result = build_migrator(name, case.dialect, migrations)
                .and_then(|migrator| {
                    let ordered = ordered_migrations(name, &migrator)?;
                    migrator.sql_migrate(&ordered).map_err(|error| {
                        TestSupportError::message(format!("{name}: migration_to_sql failed: {error}"))
                    })
                });
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let actual = result?;
            let expected = expect_sql.as_deref().ok_or_else(|| {
                TestSupportError::message(format!("{name}: migration_to_sql requires expect_sql when expect_error is absent"))
            })?;
            assert_sql_matches(name, &actual, expected)
        }
    }
}
