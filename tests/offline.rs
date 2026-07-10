mod support;

use std::collections::BTreeSet;
use std::path::PathBuf;

use gaman::core::MigratorError;
use gaman::schema::Schema;

use support::{
    ExpectedDriftFinding, LoweringExpectation, OfflineCase, OfflineEvidence, OfflineFeatureCatalog,
    OfflineFeatureResult, OfflineResultStatus, OfflineSpec, OfflineSupportResults,
    ParseExpectation, ParserFixtureDialect, SqlDirection, TestSupportError,
    assert_clarifications_match, assert_error_contains, assert_ops_match,
    assert_schema_matches_with_dialect, assert_sql_matches, build_migrator, case_label,
    offline_cases_root, offline_features_path, ordered_migrations, read_case_file, replay_schema,
    selected_cases,
};

struct OfflineArgs {
    record: Option<PathBuf>,
    case_args: Vec<String>,
}

enum CaseStatus {
    Success,
    Failure(String),
    Skipped(String),
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let root = offline_cases_root();
    let result =
        selected_cases(&root, &args.case_args).and_then(|files| run_offline_cases(args, files));

    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<OfflineArgs, TestSupportError> {
    let mut record = None;
    let mut case_args = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--record" => {
                let value = raw
                    .next()
                    .ok_or_else(|| TestSupportError::message("--record requires a path"))?;
                record = Some(PathBuf::from(value));
            }
            value if value.starts_with("--") => {
                return Err(TestSupportError::message(format!(
                    "unsupported offline harness argument '{value}'"
                )));
            }
            _ => case_args.push(arg),
        }
    }
    Ok(OfflineArgs { record, case_args })
}

fn run_offline_cases(args: OfflineArgs, files: Vec<PathBuf>) -> Result<(), TestSupportError> {
    if files.is_empty() {
        return Err(TestSupportError::message("offline: no case files selected"));
    }

    let catalog: OfflineFeatureCatalog = read_case_file(&offline_features_path())?;
    let feature_ids = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut descriptions = BTreeSet::new();
    let mut results = initial_results(&catalog);
    let mut failures = Vec::new();

    for file in files {
        let name = support::case_name(&file)?;
        let case: OfflineCase = read_case_file(&file)?;
        case.validate(&name, &file)?;
        validate_case_metadata(&name, &case, &feature_ids, &mut descriptions)?;
        let label = case_label(&name, Some(&case.description));
        let status = run_case_with_status(&name, &case);
        record_case_status(&mut results, &name, &case, &status);
        match status {
            CaseStatus::Success => println!("  ok: {label} ({})", file.display()),
            CaseStatus::Skipped(reason) => {
                println!("  ok: {label} [skipped: {reason}] ({})", file.display());
            }
            CaseStatus::Failure(message) => failures.push(format!("{}: {message}", file.display())),
        }
    }

    if let Some(path) = args.record {
        write_results(&path, &results)?;
        println!("recorded offline support results: {}", path.display());
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(TestSupportError::message(format!(
            "offline cases failed:\n\n{}",
            failures.join("\n\n")
        )))
    }
}

fn validate_case_metadata(
    name: &str,
    case: &OfflineCase,
    feature_ids: &BTreeSet<&str>,
    descriptions: &mut BTreeSet<String>,
) -> Result<(), TestSupportError> {
    if !descriptions.insert(case.description.clone()) {
        return Err(TestSupportError::message(format!(
            "{name}: duplicate offline fixture description '{}'",
            case.description
        )));
    }
    for feature in &case.features {
        if !feature_ids.contains(feature.as_str()) {
            return Err(TestSupportError::message(format!(
                "{name}: unknown offline feature '{feature}'"
            )));
        }
    }
    Ok(())
}

fn run_case_with_status(name: &str, case: &OfflineCase) -> CaseStatus {
    if !case.dialect.is_available() {
        return CaseStatus::Skipped("feature not enabled".to_string());
    }
    run_offline_case(name, case).map_or_else(
        |error| CaseStatus::Failure(error.to_string()),
        |_| CaseStatus::Success,
    )
}

fn initial_results(catalog: &OfflineFeatureCatalog) -> OfflineSupportResults {
    let mut results = OfflineSupportResults::default();
    for feature in &catalog.features {
        results.features.insert(
            feature.id.clone(),
            OfflineFeatureResult {
                status: OfflineResultStatus::Skipped,
                evidence: Vec::new(),
                reason: Some("no offline evidence recorded".to_string()),
            },
        );
    }
    results
}

fn record_case_status(
    results: &mut OfflineSupportResults,
    name: &str,
    case: &OfflineCase,
    status: &CaseStatus,
) {
    for feature in &case.features {
        let Some(current) = results.features.get_mut(feature) else {
            continue;
        };
        match status {
            CaseStatus::Success => {
                if current.status != OfflineResultStatus::Failure {
                    current.status = OfflineResultStatus::Success;
                    current.reason = None;
                }
                current.evidence.push(offline_evidence(name, case));
            }
            CaseStatus::Failure(message) => {
                current.status = OfflineResultStatus::Failure;
                current.reason = Some(message.clone());
                current.evidence.push(offline_evidence(name, case));
            }
            CaseStatus::Skipped(reason) => {
                if current.status == OfflineResultStatus::Skipped {
                    current.reason = Some(reason.clone());
                    current.evidence.push(offline_evidence(name, case));
                }
            }
        }
    }
}

fn offline_evidence(name: &str, case: &OfflineCase) -> OfflineEvidence {
    OfflineEvidence {
        case: name.to_string(),
        description: case.description.clone(),
        group: case.group.clone(),
        kind: offline_kind(&case.spec).to_string(),
        dialect: offline_dialect(case),
    }
}

fn offline_kind(spec: &OfflineSpec) -> &'static str {
    match spec {
        OfflineSpec::Parser { .. } => "parser",
        OfflineSpec::SqlToSchema { .. } => "sql_to_schema",
        OfflineSpec::SchemaToMigration { .. } => "schema_to_migration",
        OfflineSpec::MigrationToReplay { .. } => "migration_to_replay",
        OfflineSpec::MigrationToSql { .. } => "migration_to_sql",
        OfflineSpec::Verify { .. } => "verify",
        OfflineSpec::EndToEnd { .. } => "end_to_end",
    }
}

fn offline_dialect(case: &OfflineCase) -> Option<String> {
    match &case.spec {
        OfflineSpec::Parser { parser_dialect, .. } => Some(parser_dialect_name(*parser_dialect)),
        _ => Some(fixture_dialect_name(case.dialect)),
    }
}

fn parser_dialect_name(dialect: ParserFixtureDialect) -> String {
    match dialect {
        ParserFixtureDialect::Postgres => "postgres",
        ParserFixtureDialect::Sqlite => "sqlite",
        ParserFixtureDialect::Mysql => "mysql",
    }
    .to_string()
}

fn fixture_dialect_name(dialect: support::FixtureDialect) -> String {
    match dialect {
        support::FixtureDialect::Postgres => "postgres",
        support::FixtureDialect::Sqlite => "sqlite",
    }
    .to_string()
}

fn write_results(path: &PathBuf, results: &OfflineSupportResults) -> Result<(), TestSupportError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| TestSupportError::Io {
            path: parent.display().to_string(),
            message: error.to_string(),
        })?;
    }
    let yaml = serde_yaml::to_string(results).map_err(|error| {
        TestSupportError::message(format!("failed to serialize offline results: {error}"))
    })?;
    std::fs::write(path, yaml).map_err(|error| TestSupportError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn run_offline_case(name: &str, case: &OfflineCase) -> Result<(), TestSupportError> {
    let dialect = case.dialect.to_dialect()?;
    match &case.spec {
        OfflineSpec::Parser {
            parser_dialect,
            sql,
            expect_parse,
            expect_lowering,
            expect_schema,
            expect_error,
        } => run_parser_case(
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
            let result = Schema::from_sql_str(sql, gaman::core::Dialect::Postgres);
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
            expect_clarifications,
            expect_pending_clarifications,
            expect_operations,
            expect_sql,
            expect_error,
        } => {
            if expect_clarifications.is_some() && expect_pending_clarifications.is_some() {
                return Err(TestSupportError::message(format!(
                    "{name}: use either expect_clarifications or expect_pending_clarifications, not both"
                )));
            }

            let migrator = build_migrator(name, case.dialect, migrations)?;
            let result = migrator.make_migrations(
                Some(migration_name.clone()),
                current.clone(),
                true,
                decisions,
            );
            if let Some(expected) = expect_clarifications
                .as_ref()
                .or(expect_pending_clarifications.as_ref())
            {
                return assert_schema_to_migration_clarifications(name, result, expected);
            }
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
            direction,
            ids,
            migrations,
            expect_sql,
            expect_error,
        } => {
            let result = build_migrator(name, case.dialect, migrations).and_then(|migrator| {
                let ordered = selected_migrations(name, &migrator, ids)?;
                render_migration_sql_case(name, &migrator, &ordered, *direction)
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
        OfflineSpec::Verify {
            schema,
            replayed,
            inspected,
            expect_findings,
            expect_operations,
            expect_report,
            expect_error,
        } => run_verify_case(
            name,
            dialect,
            schema,
            replayed,
            inspected,
            expect_findings,
            expect_operations,
            expect_report,
            expect_error.as_deref(),
        ),
        OfflineSpec::EndToEnd {
            name: migration_name,
            migrations,
            current,
            decisions,
            expect_operations,
            expect_schema,
            expect_sql,
            expect_error,
        } => run_end_to_end_case(
            name,
            case.dialect,
            migration_name,
            migrations,
            current,
            decisions,
            expect_operations,
            expect_schema,
            expect_sql,
            expect_error.as_deref(),
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_verify_case(
    name: &str,
    dialect: gaman::core::Dialect,
    schema: &str,
    replayed: &Schema,
    inspected: &Schema,
    expect_findings: &Option<Vec<ExpectedDriftFinding>>,
    expect_operations: &Option<Vec<gaman::schema::Operation>>,
    expect_report: &Option<Vec<String>>,
    expect_error: Option<&str>,
) -> Result<(), TestSupportError> {
    let result = prepare_verify_inputs(dialect, replayed.clone(), inspected.clone())
        .map(|(replayed, inspected)| gaman::drift::diff(replayed, inspected, schema, dialect));

    if let Some(expected) = expect_error {
        return assert_error_contains(name, result.map(|_| ()), expected);
    }

    let report = result.map_err(|error| {
        TestSupportError::message(format!("{name}: verify setup failed unexpectedly: {error}"))
    })?;
    if let Some(expected) = expect_findings {
        assert_drift_findings_match(name, &report.findings, expected)?;
    }
    if let Some(expected) = expect_operations {
        assert_ops_match(name, "verify operations", &report.operations, expected)?;
    }
    if let Some(expected) = expect_report {
        let actual = gaman::drift::format_report(&report);
        if &actual != expected {
            return Err(TestSupportError::message(format!(
                "{name}: verify report mismatch\nexpected:\n{}\nactual:\n{}",
                expected.join("\n"),
                actual.join("\n")
            )));
        }
    }
    Ok(())
}

fn prepare_verify_inputs(
    dialect: gaman::core::Dialect,
    replayed: Schema,
    inspected: Schema,
) -> Result<(Schema, Schema), String> {
    let replayed = replayed
        .prepare(dialect)
        .map_err(|error| format!("failed to prepare replayed schema: {error}"))?;
    let inspected = dialect
        .normalize_inspected_schema(inspected)
        .map_err(|error| format!("failed to normalize inspected schema: {error}"))?;
    Ok((replayed, inspected))
}

fn assert_drift_findings_match(
    name: &str,
    actual: &[gaman::drift::DriftFinding],
    expected: &[ExpectedDriftFinding],
) -> Result<(), TestSupportError> {
    let actual: Vec<ExpectedDriftFinding> = actual
        .iter()
        .map(|finding| ExpectedDriftFinding {
            operation: finding.operation.to_string(),
            entity_kind: drift_entity_kind_name(finding.entity_kind).to_string(),
            entity_name: finding.entity_name.clone(),
            property: finding.property.to_string(),
            expected: finding.expected.clone(),
            observed: finding.observed.clone(),
            note: finding.note.clone(),
        })
        .collect();
    if actual == expected {
        return Ok(());
    }

    let actual_yaml = serde_yaml::to_string(&actual).map_err(|error| {
        TestSupportError::message(format!(
            "{name}: failed to serialize actual findings: {error}"
        ))
    })?;
    let expected_yaml = serde_yaml::to_string(expected).map_err(|error| {
        TestSupportError::message(format!(
            "{name}: failed to serialize expected findings: {error}"
        ))
    })?;
    Err(TestSupportError::message(format!(
        "{name}: verify findings mismatch\nexpected:\n{expected_yaml}\nactual:\n{actual_yaml}",
    )))
}

fn drift_entity_kind_name(kind: impl std::fmt::Debug) -> String {
    let debug = format!("{kind:?}");
    let mut name = String::new();
    for (index, ch) in debug.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                name.push('_');
            }
            name.push(ch.to_ascii_lowercase());
        } else {
            name.push(ch);
        }
    }
    name
}

fn selected_migrations(
    name: &str,
    migrator: &gaman::core::Migrator,
    ids: &[String],
) -> Result<Vec<gaman::Migration>, TestSupportError> {
    if ids.is_empty() {
        return ordered_migrations(name, migrator);
    }

    ids.iter()
        .map(|id| {
            migrator.graph.get(id).cloned().ok_or_else(|| {
                TestSupportError::message(format!("{name}: graph is missing migration '{id}'"))
            })
        })
        .collect()
}

fn render_migration_sql_case(
    name: &str,
    migrator: &gaman::core::Migrator,
    ordered: &[gaman::Migration],
    direction: SqlDirection,
) -> Result<Vec<String>, TestSupportError> {
    match direction {
        SqlDirection::Forward => migrator.sql_migrate(ordered).map_err(|error| {
            TestSupportError::message(format!("{name}: migration_to_sql failed: {error}"))
        }),
        SqlDirection::Rollback => migrator.sql_rollback(ordered).map_err(|error| {
            TestSupportError::message(format!("{name}: migration_to_sql rollback failed: {error}"))
        }),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_end_to_end_case(
    name: &str,
    fixture_dialect: support::FixtureDialect,
    migration_name: &str,
    migrations: &[support::InlineMigration],
    current: &Schema,
    decisions: &[gaman::core::Decision],
    expect_operations: &Option<Vec<gaman::schema::Operation>>,
    expect_schema: &Option<Schema>,
    expect_sql: &Option<String>,
    expect_error: Option<&str>,
) -> Result<(), TestSupportError> {
    let dialect = fixture_dialect.to_dialect()?;
    let migrator = build_migrator(name, fixture_dialect, migrations)?;
    let result = migrator.make_migrations(
        Some(migration_name.to_string()),
        current.clone(),
        true,
        decisions,
    );
    if let Some(expected) = expect_error {
        return assert_error_contains(name, result.map(|_| ()), expected);
    }

    let generated = result
        .map_err(|error| {
            TestSupportError::message(format!("{name}: end_to_end diff failed: {error}"))
        })?
        .ok_or_else(|| {
            TestSupportError::message(format!(
                "{name}: expected generated migration, but diff returned no changes"
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
    if let Some(expected) = expect_schema {
        let final_schema = replay_with_generated(name, &migrator, &generated)?;
        assert_schema_matches_with_dialect(
            name,
            "final schema",
            final_schema,
            expected.clone(),
            dialect,
        )?;
    }
    if let Some(expected) = expect_sql {
        let actual = migrator
            .sql_migrate(std::slice::from_ref(&generated))
            .map_err(|error| {
                TestSupportError::message(format!(
                    "{name}: failed to render generated SQL: {error}"
                ))
            })?;
        assert_sql_matches(name, &actual, expected)?;
    }
    Ok(())
}

fn replay_with_generated(
    name: &str,
    migrator: &gaman::core::Migrator,
    generated: &gaman::Migration,
) -> Result<Schema, TestSupportError> {
    let mut schema = replay_schema(name, migrator)?;
    for (index, op) in generated.operations.iter().enumerate() {
        schema.apply(op).map_err(|error| {
            TestSupportError::message(format!(
                "{name}: generated replay failed at operation {} ({}): {}",
                index + 1,
                op.type_name(),
                error
            ))
        })?;
    }
    Ok(schema)
}

fn assert_schema_to_migration_clarifications(
    name: &str,
    result: Result<Option<gaman::Migration>, MigratorError>,
    expected: &[gaman::core::Clarification],
) -> Result<(), TestSupportError> {
    match result {
        Err(MigratorError::NeedsInput(actual)) => {
            assert_clarifications_match(name, "clarifications", &actual, expected)
        }
        Ok(Some(migration)) => Err(TestSupportError::message(format!(
            "{name}: expected clarifications, but generated migration '{}'",
            migration.id
        ))),
        Ok(None) => Err(TestSupportError::message(format!(
            "{name}: expected clarifications, but diff returned no changes"
        ))),
        Err(error) => Err(TestSupportError::message(format!(
            "{name}: expected clarification input, but got error: {error}"
        ))),
    }
}

fn run_parser_case(
    name: &str,
    dialect: ParserFixtureDialect,
    sql: &str,
    expect_parse: ParseExpectation,
    expect_lowering: LoweringExpectation,
    expect_schema: &Option<Schema>,
    expect_error: Option<&str>,
) -> Result<(), TestSupportError> {
    let lowering = lower_sql_to_schema(dialect, sql);
    let parse_failed = lowering.as_ref().err().is_some_and(|error| {
        error.contains("SQL parse error") || error.contains("unsupported SQL dialect")
    });
    match (expect_parse, parse_failed) {
        (ParseExpectation::Ok, false) => {}
        (ParseExpectation::Ok, true) => {
            return Err(TestSupportError::message(format!(
                "{name}: expected parser success but got {}",
                lowering.err().unwrap()
            )));
        }
        (ParseExpectation::Error, false) => {
            return Err(TestSupportError::message(format!(
                "{name}: expected parser failure but parse succeeded"
            )));
        }
        (ParseExpectation::Error, true) => {
            if let Some(expected) = expect_error {
                let actual = lowering.err().unwrap();
                if !actual.contains(expected) {
                    return Err(TestSupportError::message(format!(
                        "{name}: expected parse error containing '{expected}' but got '{actual}'"
                    )));
                }
            }
            return Ok(());
        }
    }

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
                    parser_dialect_to_schema_dialect(dialect)?,
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

fn lower_sql_to_schema(dialect: ParserFixtureDialect, sql: &str) -> Result<Schema, String> {
    match dialect {
        ParserFixtureDialect::Postgres => {
            gaman::parsers::parse_sql(sql, gaman::core::Dialect::Postgres)
        }
        ParserFixtureDialect::Sqlite => {
            gaman::parsers::parse_sql(sql, gaman::core::Dialect::Sqlite)
        }
        ParserFixtureDialect::Mysql => Err(gaman::parsers::ParseError::UnsupportedDialect(
            "mysql".to_string(),
        )),
    }
    .map_err(|error| error.to_string())
}

fn parser_dialect_to_schema_dialect(
    dialect: ParserFixtureDialect,
) -> Result<gaman::core::Dialect, TestSupportError> {
    match dialect {
        ParserFixtureDialect::Postgres => Ok(gaman::core::Dialect::Postgres),
        ParserFixtureDialect::Sqlite => Ok(gaman::core::Dialect::Sqlite),
        ParserFixtureDialect::Mysql => Err(TestSupportError::message(
            "mysql parser fixtures cannot compare schema because MySQL schema lowering is unsupported",
        )),
    }
}
