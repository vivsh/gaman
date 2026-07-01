#![cfg(feature = "sqlite")]

mod support;

use support::{
    SqliteCase, SqliteHarness, SqliteSpec, TestSupportError, assert_error_contains,
    assert_ops_match, assert_schema_matches_with_dialect, build_sqlite_migrator, case_label,
    case_name, read_case_file, selected_cases, sqlite_cases_root,
};

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let root = sqlite_cases_root();
    let files = match selected_cases(&root, &args) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if files.is_empty() {
        eprintln!("sqlite: no case files selected");
        std::process::exit(1);
    }

    let mut failures = Vec::new();
    for file in files {
        let name = match case_name(&file) {
            Ok(name) => name,
            Err(error) => {
                failures.push(format!("{}: {error}", file.display()));
                continue;
            }
        };
        let result = (|| async {
            let case: SqliteCase = read_case_file(&file)?;
            let label = case_label(&name, case.description.as_deref());
            run_sqlite_case(&name, &case).await?;
            Ok::<String, TestSupportError>(label)
        })()
        .await;
        match result {
            Ok(label) => println!("  ok: {label} ({})", file.display()),
            Err(error) => failures.push(format!("{}: {error}", file.display())),
        }
    }

    if !failures.is_empty() {
        eprintln!("sqlite cases failed:\n\n{}", failures.join("\n\n"));
        std::process::exit(1);
    }
}

async fn run_sqlite_case(name: &str, case: &SqliteCase) -> Result<(), TestSupportError> {
    let harness = SqliteHarness::new().await?;
    match &case.spec {
        SqliteSpec::Migrate {
            migrations,
            setup_sql,
            expect_schema,
            expect_error,
        } => {
            if let Some(sql) = setup_sql {
                harness.batch_execute(sql).await?;
            }
            let migrator = build_sqlite_migrator(name, &harness, migrations)?;
            let result = migrator.migrate(None, false).await;
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }
            result.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate failed unexpectedly: {error}"))
            })?;
            if let Some(expected) = expect_schema.clone() {
                let actual = harness.inspect_schema().await?;
                assert_schema_matches_with_dialect(
                    name,
                    "inspected schema",
                    actual,
                    expected,
                    gaman::core::Dialect::Sqlite,
                )?;
            }
            Ok(())
        }
        SqliteSpec::Inspect {
            setup_sql,
            expect_schema,
            expect_error,
        } => {
            if let Some(sql) = setup_sql {
                harness.batch_execute(sql).await?;
            }
            let result = harness.inspect_schema().await;
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }
            let actual = result?;
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!(
                    "{name}: inspect case requires expect_schema when expect_error is absent"
                ))
            })?;
            assert_schema_matches_with_dialect(
                name,
                "inspected schema",
                actual,
                expected,
                gaman::core::Dialect::Sqlite,
            )
        }
        SqliteSpec::Verify {
            migrations,
            setup_sql,
            mutate_sql,
            expect_verify,
            expect_error,
        } => {
            if let Some(sql) = setup_sql {
                harness.batch_execute(sql).await?;
            }
            let migrator = build_sqlite_migrator(name, &harness, migrations)?;
            if !migrations.is_empty() {
                migrator.migrate(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
            }
            if let Some(sql) = mutate_sql {
                harness.batch_execute(sql).await?;
            }
            let result = harness.verify(&migrator).await;
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }
            let actual = result?;
            let expected = expect_verify.clone().ok_or_else(|| {
                TestSupportError::message(format!(
                    "{name}: verify case requires expect_verify when expect_error is absent"
                ))
            })?;
            assert_ops_match(name, "verify operations", &actual, &expected)
        }
    }
}
