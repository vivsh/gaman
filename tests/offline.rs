mod support;

use gaman::schema::Schema;
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use support::{
    LoweringExpectation, OfflineCase, OfflineSpec, ParseExpectation, ParserFixtureDialect,
    TestSupportError, assert_error_contains, assert_ops_match, assert_schema_matches_with_dialect,
    assert_sql_matches, build_migrator, case_label, offline_cases_root, ordered_migrations,
    read_case_file, replay_schema, run_case_set, selected_cases,
};

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let root = offline_cases_root();
    let result = selected_cases(&root, &args).and_then(|files| {
        run_case_set("offline", files, |file, name| {
            let case: OfflineCase = read_case_file(file)?;
            let label = case_label(name, case.description.as_deref());
            run_offline_case(name, &case)?;
            Ok(label)
        })
    });

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run_offline_case(name: &str, case: &OfflineCase) -> Result<(), TestSupportError> {
    let dialect = case.dialect.to_dialect();
    match &case.spec {
        OfflineSpec::SqlParse {
            parser_dialect,
            sql,
            expect_parse,
            expect_lowering,
            expect_schema,
            expect_error,
        } => run_sql_parse_case(
            name,
            *parser_dialect,
            sql,
            *expect_parse,
            *expect_lowering,
            expect_schema,
            expect_error.as_deref(),
        ),
        OfflineSpec::SqlToSchema {
            sql,
            expect_schema,
            expect_error,
        } => {
            let result = Schema::from_sql_str(sql);
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let actual = result.map_err(|error| {
                TestSupportError::message(format!(
                    "{name}: sql_to_schema failed unexpectedly: {error}"
                ))
            })?;
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!(
                    "{name}: sql_to_schema requires expect_schema when expect_error is absent"
                ))
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
            let result = migrator.make_migrations(
                Some(migration_name.clone()),
                current.clone(),
                true,
                decisions,
            );
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let generated = result.map_err(|error| {
                TestSupportError::message(format!(
                    "{name}: schema_to_migration failed unexpectedly: {error}"
                ))
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
                TestSupportError::message(format!(
                    "{name}: expected a generated migration, but diff returned no changes"
                ))
            })?;

            if let Some(expected) = expect_operations {
                assert_ops_match(
                    name,
                    "generated operations",
                    &generated.operations,
                    expected,
                )?;
            }
            if let Some(expected) = expect_sql {
                let actual = migrator.sql_migrate(&[generated]).map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: failed to render generated SQL: {error}"
                    ))
                })?;
                assert_sql_matches(name, &actual, expected)?;
            }
            Ok(())
        }
        OfflineSpec::MigrationToReplay {
            migrations,
            expect_schema,
            expect_error,
        } => {
            let result = build_migrator(name, case.dialect, migrations)
                .and_then(|migrator| replay_schema(name, &migrator));
            if let Some(expected) = expect_error {
                return assert_error_contains(name, result.map(|_| ()), expected);
            }

            let actual = result?;
            let expected = expect_schema.clone().ok_or_else(|| {
                TestSupportError::message(format!(
                    "{name}: migration_to_replay requires expect_schema when expect_error is absent"
                ))
            })?;
            assert_schema_matches_with_dialect(name, "replayed schema", actual, expected, dialect)
        }
        OfflineSpec::MigrationToSql {
            migrations,
            expect_sql,
            expect_error,
        } => {
            let result = build_migrator(name, case.dialect, migrations).and_then(|migrator| {
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
                TestSupportError::message(format!(
                    "{name}: migration_to_sql requires expect_sql when expect_error is absent"
                ))
            })?;
            assert_sql_matches(name, &actual, expected)
        }
    }
}

fn run_sql_parse_case(
    name: &str,
    dialect: ParserFixtureDialect,
    sql: &str,
    expect_parse: ParseExpectation,
    expect_lowering: LoweringExpectation,
    expect_schema: &Option<Schema>,
    expect_error: Option<&str>,
) -> Result<(), TestSupportError> {
    let parse = parse_sql_only(dialect, sql);
    match (expect_parse, parse) {
        (ParseExpectation::Ok, Ok(_)) => {}
        (ParseExpectation::Ok, Err(error)) => {
            return Err(TestSupportError::message(format!(
                "{name}: expected parser success but got {error}"
            )));
        }
        (ParseExpectation::Error, Ok(_)) => {
            return Err(TestSupportError::message(format!(
                "{name}: expected parser failure but parse succeeded"
            )));
        }
        (ParseExpectation::Error, Err(error)) => {
            if let Some(expected) = expect_error {
                let actual = error.to_string();
                if !actual.contains(expected) {
                    return Err(TestSupportError::message(format!(
                        "{name}: expected parse error containing '{expected}' but got '{actual}'"
                    )));
                }
            }
            return Ok(());
        }
    }

    let lowering = lower_sql_to_schema(dialect, sql);
    match expect_lowering {
        LoweringExpectation::Ok => {
            let actual = lowering.map_err(|error| {
                TestSupportError::message(format!(
                    "{name}: expected lowering success but got {error}"
                ))
            })?;
            if let Some(expected) = expect_schema.clone() {
                assert_schema_matches_with_dialect(
                    name,
                    "lowered schema",
                    actual,
                    expected,
                    gaman::core::Dialect::Postgres,
                )?;
            }
        }
        LoweringExpectation::Unsupported => match lowering {
            Ok(_) => {
                return Err(TestSupportError::message(format!(
                    "{name}: expected unsupported lowering but lowering succeeded"
                )));
            }
            Err(error) => {
                let actual = error.to_string();
                let expected = expect_error.unwrap_or("unsupported");
                if !actual.contains(expected) {
                    return Err(TestSupportError::message(format!(
                        "{name}: expected unsupported lowering error containing '{expected}' but got '{actual}'"
                    )));
                }
            }
        },
        LoweringExpectation::Error => {
            if let Some(expected) = expect_error {
                assert_error_contains(name, lowering.map(|_| ()), expected)?;
            } else if lowering.is_ok() {
                return Err(TestSupportError::message(format!(
                    "{name}: expected lowering error but lowering succeeded"
                )));
            }
        }
    }
    Ok(())
}

fn parse_sql_only(dialect: ParserFixtureDialect, sql: &str) -> Result<usize, String> {
    match dialect {
        ParserFixtureDialect::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, sql),
        ParserFixtureDialect::Sqlite => Parser::parse_sql(&SQLiteDialect {}, sql),
        ParserFixtureDialect::Mysql => Parser::parse_sql(&MySqlDialect {}, sql),
    }
    .map(|statements| statements.len())
    .map_err(|error| error.to_string())
}

fn lower_sql_to_schema(dialect: ParserFixtureDialect, sql: &str) -> Result<Schema, String> {
    match dialect {
        ParserFixtureDialect::Postgres => {
            Schema::from_sql_str(sql).map_err(|error| error.to_string())
        }
        ParserFixtureDialect::Sqlite => {
            Err("unsupported SQL lowering for sqlite parser fixtures".to_string())
        }
        ParserFixtureDialect::Mysql => {
            Err("unsupported SQL lowering for mysql parser fixtures".to_string())
        }
    }
}
