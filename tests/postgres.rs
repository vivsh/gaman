#![cfg(feature = "postgres")]

mod support;

use support::{
    PgHarness, PostgresCase, PostgresSpec, TestSupportError, assert_error_contains,
    assert_ops_match, assert_schema_matches, build_postgres_migrator, case_label, case_name,
    postgres_cases_root, read_case_file, scope_schema_for_compare, selected_cases,
};

/// Runs PostgreSQL-backed cases for migrate, verify, and inspect.
#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let root = postgres_cases_root();
    let explicit = !args.is_empty();
    let files = match selected_cases(&root, &args) {
        Ok(files) => files,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if files.is_empty() {
        eprintln!("postgres: no case files selected");
        std::process::exit(1);
    }

    if std::env::var("TEST_DATABASE_URL").is_err() {
        let message = "postgres cases skipped: TEST_DATABASE_URL is not set";
        if explicit {
            eprintln!("{message}");
            std::process::exit(1);
        }
        println!("{message}");
        return;
    }

    let mut failures = Vec::new();

    for file in files {
        let name = match case_name(&file) {
            Ok(name) => name,
            Err(error) => {
                failures.push(error.to_string());
                continue;
            }
        };

        let result = (|| async {
            let case: PostgresCase = read_case_file(&file)?;
            let label = case_label(&name, case.description.as_deref());
            run_postgres_case(&name, &case).await?;
            Ok::<String, TestSupportError>(label)
        })()
        .await;

        match result {
            Ok(label) => println!("  ok: {label}"),
            Err(error) => failures.push(error.to_string()),
        }
    }

    if !failures.is_empty() {
        eprintln!("postgres cases failed:\n\n{}", failures.join("\n\n"));
        std::process::exit(1);
    }
}

async fn run_postgres_case(name: &str, case: &PostgresCase) -> Result<(), TestSupportError> {
    let mut harness = PgHarness::new().await?;
    harness.reset().await?;
    let result = match &case.spec {
        PostgresSpec::Migrate {
            migrations,
            setup_sql,
            target,
            fake,
            expect_schema,
            expect_error,
        } => {
            if let Some(sql) = setup_sql {
                harness.batch_execute(sql).await?;
            }
            let migrator = build_postgres_migrator(name, &harness, migrations)?;
            let result = migrator.migrate(target.as_deref(), *fake).await;
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }
            result.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate failed unexpectedly: {error}"))
            })?;
            if let Some(expected) = expect_schema.clone() {
                let mut actual = harness.inspect_schema().await?;
                scope_schema_for_compare(&mut actual, harness.schema_name());
                assert_schema_matches(name, "inspected schema", actual, expected)?;
            }
            Ok(())
        }
        PostgresSpec::Verify {
            migrations,
            setup_sql,
            mutate_sql,
            expect_verify,
            expect_error,
        } => {
            if let Some(sql) = setup_sql {
                harness.batch_execute(sql).await?;
            }
            let migrator = build_postgres_migrator(name, &harness, migrations)?;
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
        PostgresSpec::Inspect {
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
            let mut actual = result?;
            scope_schema_for_compare(&mut actual, harness.schema_name());
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!(
                    "{name}: inspect case requires expect_schema when expect_error is absent"
                ))
            })?;
            assert_schema_matches(name, "inspected schema", actual, expected)
        }
    };

    let cleanup = harness.cleanup().await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(cleanup)) => Err(cleanup),
        (Err(error), Err(cleanup)) => Err(TestSupportError::message(format!(
            "{error}\ncleanup also failed: {cleanup}"
        ))),
    }
}
