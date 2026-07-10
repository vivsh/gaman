mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use support::{
    FeatureCatalog, OnlineCase, OnlineCheck, OnlineDialect, OnlineEvidence, OnlineFeatureResult,
    OnlineResultStatus, OnlineSupportResults, POSTGRES_DATABASE_URL_ENV, TestSupportError,
    assert_error_contains, assert_ops_match, case_label, case_name, features_path,
    online_cases_root, read_case_file, selected_cases,
};

struct OnlineArgs {
    dialect: Option<OnlineDialect>,
    record: PathBuf,
    explicit_record: bool,
    case_args: Vec<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    let root = online_cases_root();
    let result = match selected_cases(&root, &args.case_args) {
        Ok(files) => async_main(args, files).await,
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

async fn async_main(args: OnlineArgs, files: Vec<PathBuf>) -> Result<(), TestSupportError> {
    let catalog: FeatureCatalog = read_case_file(&features_path())?;
    let feature_ids = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut descriptions = BTreeSet::new();
    let mut results = initial_results(&catalog);
    let mut failures = Vec::new();

    for file in files {
        let name = case_name(&file)?;
        let case: OnlineCase = read_case_file(&file)?;
        validate_online_case(&name, &case, &feature_ids, &mut descriptions)?;
        let label = case_label(&name, Some(&case.description));
        let dialects = args
            .dialect
            .map(|dialect| vec![dialect])
            .unwrap_or_else(|| OnlineDialect::all().to_vec());

        for dialect in dialects {
            let status = run_online_dialect(&name, &case, dialect, args.dialect.is_some()).await;
            record_case_status(&mut results, &name, &case, dialect, &status);
            match status {
                CaseStatus::Success => println!("  ok: {label} [{}]", dialect.as_str()),
                CaseStatus::Unimplemented(reason) => {
                    println!("  skip: {label} [{}: {reason}]", dialect.as_str());
                }
                CaseStatus::Failure(message) => {
                    println!("  fail: {label} [{}]", dialect.as_str());
                    failures.push(format!("{name} [{}]: {message}", dialect.as_str()));
                }
            }
        }
    }

    write_results(&args.record, &results)?;
    if !failures.is_empty() {
        return Err(TestSupportError::message(format!(
            "online cases failed:\n\n{}",
            failures.join("\n\n")
        )));
    }
    if args.explicit_record {
        println!("recorded online support results: {}", args.record.display());
    }
    Ok(())
}

fn parse_args() -> Result<OnlineArgs, TestSupportError> {
    let mut dialect = None;
    let mut record = PathBuf::from("results/online-support-results.yaml");
    let mut explicit_record = false;
    let mut case_args = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--dialect" => {
                let value = raw.next().ok_or_else(|| {
                    TestSupportError::message("--dialect requires postgres, sqlite, or mysql")
                })?;
                dialect = Some(parse_online_dialect(&value)?);
            }
            "--record" => {
                let value = raw
                    .next()
                    .ok_or_else(|| TestSupportError::message("--record requires a path"))?;
                record = PathBuf::from(value);
                explicit_record = true;
            }
            value if value.starts_with("--") => {
                return Err(TestSupportError::message(format!(
                    "unsupported online harness argument '{value}'"
                )));
            }
            _ => case_args.push(arg),
        }
    }
    Ok(OnlineArgs {
        dialect,
        record,
        explicit_record,
        case_args,
    })
}

fn parse_online_dialect(value: &str) -> Result<OnlineDialect, TestSupportError> {
    match value {
        "postgres" | "postgresql" => Ok(OnlineDialect::Postgres),
        "sqlite" | "sqlite3" => Ok(OnlineDialect::Sqlite),
        "mysql" | "mariadb" => Ok(OnlineDialect::Mysql),
        _ => Err(TestSupportError::message(format!(
            "unsupported online dialect '{value}'"
        ))),
    }
}

fn expected_inspect_schema(
    name: &str,
    section: &support::OnlineDialectCase,
) -> Result<gaman::schema::Schema, TestSupportError> {
    section.expect_schema.clone().ok_or_else(|| {
        TestSupportError::message(format!(
            "{name}: inspect check succeeded, but the fixture has no expect_schema"
        ))
    })
}

fn assert_inspected_extensions(
    name: &str,
    schema: &gaman::schema::Schema,
    expected: &[String],
) -> Result<(), TestSupportError> {
    for extension_name in expected {
        let extension = schema.extensions.get(extension_name).ok_or_else(|| {
            TestSupportError::message(format!(
                "{name}: inspected schema is missing extension '{extension_name}'"
            ))
        })?;
        if extension.version.as_deref().is_none_or(str::is_empty) {
            return Err(TestSupportError::message(format!(
                "{name}: inspected extension '{extension_name}' has no catalog version"
            )));
        }
    }
    Ok(())
}

fn expected_verify_ops<'a>(
    name: &str,
    section: &'a support::OnlineDialectCase,
) -> Result<&'a [gaman::schema::Operation], TestSupportError> {
    section.expect_verify.as_deref().ok_or_else(|| {
        TestSupportError::message(format!(
            "{name}: verify check succeeded, but the fixture has no expect_verify"
        ))
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ExpectedErrorAction {
    Migrate,
    MigrateTo,
    Rollback,
    Inspect,
    Verify,
}

fn expected_error_action(section: &support::OnlineDialectCase) -> Option<ExpectedErrorAction> {
    if !section.checks.contains(&OnlineCheck::Error) {
        return None;
    }
    if section.checks.contains(&OnlineCheck::Migrate) {
        Some(ExpectedErrorAction::Migrate)
    } else if section.checks.contains(&OnlineCheck::MigrateTo) {
        Some(ExpectedErrorAction::MigrateTo)
    } else if section.checks.contains(&OnlineCheck::Rollback) {
        Some(ExpectedErrorAction::Rollback)
    } else if section.checks.contains(&OnlineCheck::Inspect) {
        Some(ExpectedErrorAction::Inspect)
    } else if section.checks.contains(&OnlineCheck::Verify) {
        Some(ExpectedErrorAction::Verify)
    } else {
        None
    }
}

fn required_target<'a>(
    name: &str,
    section: &'a support::OnlineDialectCase,
    check: &str,
) -> Result<&'a str, TestSupportError> {
    section
        .target
        .as_deref()
        .ok_or_else(|| TestSupportError::message(format!("{name}: {check} check requires target")))
}

fn expected_error<'a>(
    name: &str,
    section: &'a support::OnlineDialectCase,
) -> Result<&'a str, TestSupportError> {
    section.expect_error.as_deref().ok_or_else(|| {
        TestSupportError::message(format!("{name}: error check requires expect_error"))
    })
}

fn assert_records_match(
    name: &str,
    actual: Vec<String>,
    expected: &[String],
) -> Result<(), TestSupportError> {
    if actual == expected {
        Ok(())
    } else {
        Err(TestSupportError::message(format!(
            "{name}: migration records mismatch\nexpected: {:?}\nactual: {:?}",
            expected, actual
        )))
    }
}

fn validate_online_case(
    name: &str,
    case: &OnlineCase,
    known_features: &BTreeSet<&str>,
    descriptions: &mut BTreeSet<String>,
) -> Result<(), TestSupportError> {
    if case.description.trim().is_empty() {
        return Err(TestSupportError::message(format!(
            "{name}: online fixture description must not be empty"
        )));
    }
    if !descriptions.insert(case.description.clone()) {
        return Err(TestSupportError::message(format!(
            "{name}: duplicate online fixture description '{}'",
            case.description
        )));
    }
    if case.features.is_empty() {
        return Err(TestSupportError::message(format!(
            "{name}: online fixture must list at least one feature"
        )));
    }
    for feature in &case.features {
        if !known_features.contains(feature.as_str()) {
            return Err(TestSupportError::message(format!(
                "{name}: unknown online feature '{feature}'"
            )));
        }
    }
    for (dialect, section) in &case.dialects {
        if section.checks.is_empty() {
            return Err(TestSupportError::message(format!(
                "{name}: {} section must list at least one check",
                dialect.as_str()
            )));
        }
        if !section.requires_extensions.is_empty() && *dialect != OnlineDialect::Postgres {
            return Err(TestSupportError::message(format!(
                "{name}: requires_extensions is currently PostgreSQL-only"
            )));
        }
        if section.checks.contains(&OnlineCheck::Inspect)
            && section.expect_schema.is_none()
            && section.expect_extensions.is_empty()
            && section.expect_error.is_none()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} inspect checks require expect_schema",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::Verify)
            && section.expect_verify.is_none()
            && section.expect_error.is_none()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} verify checks require expect_verify",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::Error) && section.expect_error.is_none() {
            return Err(TestSupportError::message(format!(
                "{name}: {} error checks require expect_error",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::Error) && expected_error_action(section).is_none()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} error checks must pair with migrate, migrate_to, rollback, inspect, or verify",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::MigrateTo) && section.target.is_none() {
            return Err(TestSupportError::message(format!(
                "{name}: {} migrate_to checks require target",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::Rollback) && section.target.is_none() {
            return Err(TestSupportError::message(format!(
                "{name}: {} rollback checks require target",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::MigrationRecords)
            && section.expect_records.is_empty()
            && !section.checks.contains(&OnlineCheck::Error)
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} migration_records checks require expect_records",
                dialect.as_str()
            )));
        }
    }
    Ok(())
}

fn initial_results(catalog: &FeatureCatalog) -> OnlineSupportResults {
    let mut results = OnlineSupportResults::default();
    for feature in &catalog.features {
        let mut dialects = BTreeMap::new();
        for dialect in OnlineDialect::all() {
            dialects.insert(
                dialect.as_str().to_string(),
                OnlineFeatureResult {
                    status: OnlineResultStatus::Unimplemented,
                    evidence: Vec::new(),
                    reason: Some("no online evidence recorded".to_string()),
                },
            );
        }
        results.features.insert(feature.id.clone(), dialects);
    }
    results
}

enum CaseStatus {
    Success,
    Failure(String),
    Unimplemented(String),
}

async fn run_online_dialect(
    name: &str,
    case: &OnlineCase,
    dialect: OnlineDialect,
    explicit_dialect: bool,
) -> CaseStatus {
    let Some(section) = case.dialects.get(&dialect) else {
        return CaseStatus::Unimplemented("dialect section missing".to_string());
    };
    match dialect {
        OnlineDialect::Postgres => {
            if !cfg!(feature = "postgres") {
                if explicit_dialect {
                    return CaseStatus::Failure(
                        "postgres feature is not enabled for online harness".to_string(),
                    );
                }
                return CaseStatus::Unimplemented(
                    "postgres feature is not enabled for online harness".to_string(),
                );
            }
            if std::env::var(POSTGRES_DATABASE_URL_ENV).is_err() {
                if explicit_dialect {
                    return CaseStatus::Failure(format!(
                        "{POSTGRES_DATABASE_URL_ENV} must be set to run PostgreSQL online cases"
                    ));
                }
                return CaseStatus::Unimplemented(format!(
                    "{POSTGRES_DATABASE_URL_ENV} is not set"
                ));
            }
            match support::missing_postgres_extensions(&section.requires_extensions).await {
                Ok(missing) if !missing.is_empty() => {
                    return CaseStatus::Unimplemented(format!(
                        "required PostgreSQL extensions unavailable: {}",
                        missing.join(", ")
                    ));
                }
                Ok(_) => {}
                Err(error) => return CaseStatus::Failure(error.to_string()),
            }
            run_postgres_online_case(name, case, section)
                .await
                .map_or_else(
                    |error| CaseStatus::Failure(error.to_string()),
                    |_| CaseStatus::Success,
                )
        }
        OnlineDialect::Sqlite => {
            if !cfg!(feature = "sqlite") {
                if explicit_dialect {
                    return CaseStatus::Failure(
                        "sqlite feature is not enabled for online harness".to_string(),
                    );
                }
                return CaseStatus::Unimplemented(
                    "sqlite feature is not enabled for online harness".to_string(),
                );
            }
            run_sqlite_online_case(name, case, section)
                .await
                .map_or_else(
                    |error| CaseStatus::Failure(error.to_string()),
                    |_| CaseStatus::Success,
                )
        }
        OnlineDialect::Mysql => {
            let _ = section;
            CaseStatus::Unimplemented("mysql dialect is not implemented".to_string())
        }
    }
}

#[cfg(feature = "postgres")]
async fn run_postgres_online_case(
    name: &str,
    case: &OnlineCase,
    section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    let mut harness = support::PgHarness::new().await?;
    harness.reset().await?;
    let result = run_postgres_checks(name, &mut harness, case, section).await;
    let cleanup = harness.cleanup().await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Err(error), Err(cleanup)) => Err(TestSupportError::message(format!(
            "{error}\ncleanup also failed: {cleanup}"
        ))),
    }
}

#[cfg(not(feature = "postgres"))]
async fn run_postgres_online_case(
    _name: &str,
    _case: &OnlineCase,
    _section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    Err(TestSupportError::message(
        "postgres feature is not enabled for online harness",
    ))
}

#[cfg(feature = "postgres")]
async fn run_postgres_checks(
    name: &str,
    harness: &mut support::PgHarness,
    case: &OnlineCase,
    section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    async {
        if let Some(sql) = section.setup_sql(case) {
            let sql = support::postgres_placeholder_text(sql, harness.schema_name());
            harness.batch_execute(&sql).await?;
        }
        let migrations = section.migrations(case);
        let migrator = support::build_postgres_migrator(name, harness, migrations)?;
        let mut migrated = false;
        let mut migration_attempted = false;
        let error_action = expected_error_action(section);
        if section.checks.contains(&OnlineCheck::Migrate) {
            let result = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate failed unexpectedly: {error}"))
            });
            migration_attempted = true;
            if error_action == Some(ExpectedErrorAction::Migrate) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::MigrateTwice) {
            let first = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: first migrate failed: {error}"))
            })?;
            let second = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: second migrate failed: {error}"))
            })?;
            if second.applied != 0 || second.reverted != 0 {
                return Err(TestSupportError::message(format!(
                    "{name}: second migrate should be idempotent but changed {second:?}"
                )));
            }
            migration_attempted = true;
            migrated = first.applied > 0 || first.reverted > 0 || migrated;
        }
        if section.checks.contains(&OnlineCheck::MigrateTo) {
            let target = required_target(name, section, "migrate_to")?;
            let result = migrator.apply(Some(target), false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate_to failed: {error}"))
            });
            migration_attempted = true;
            if error_action == Some(ExpectedErrorAction::MigrateTo) {
                assert_error_contains(name, result.map(|_| ()), expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::Rollback) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
            }
            let target = required_target(name, section, "rollback")?;
            let result = migrator.apply(Some(target), false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: rollback failed: {error}"))
            });
            if error_action == Some(ExpectedErrorAction::Rollback) {
                assert_error_contains(name, result.map(|_| ()), expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::LockBehavior) {
            harness.assert_lock_released().await?;
        }
        if section.checks.contains(&OnlineCheck::MigrationRecords) {
            let actual = harness.migration_records().await?;
            assert_records_match(name, actual, &section.expect_records)?;
        }
        if section.checks.contains(&OnlineCheck::Inspect) {
            let result = async {
                let mut actual = harness.inspect_schema().await?;
                support::scope_schema_for_compare(&mut actual, harness.schema_name());
                if let Some(expected) = &section.expect_schema {
                    support::assert_schema_matches(
                        name,
                        "inspected schema",
                        actual.clone(),
                        expected.clone(),
                    )?;
                }
                assert_inspected_extensions(name, &actual, &section.expect_extensions)
            }
            .await;
            if error_action == Some(ExpectedErrorAction::Inspect) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::Verify) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
                migration_attempted = true;
                migrated = true;
            }
            if let Some(sql) = section.mutate_sql(case) {
                let sql = support::postgres_placeholder_text(sql, harness.schema_name());
                harness.batch_execute(&sql).await?;
            }
            let result = async {
                let actual = harness.verify(&migrator).await?;
                let expected = expected_verify_ops(name, section)?;
                assert_ops_match(name, "verify operations", &actual, expected)
            }
            .await;
            if error_action == Some(ExpectedErrorAction::Verify) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::Data) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
            }
            for check in &section.data {
                let sql = support::postgres_placeholder_text(&check.sql, harness.schema_name());
                if let Some(expected) = &check.expect_error {
                    assert_error_contains(name, harness.batch_execute(&sql).await, expected)?;
                } else {
                    let actual = harness.fetch_strings(&sql).await?;
                    if actual != check.expect {
                        return Err(TestSupportError::message(format!(
                            "{name}: data check mismatch\nexpected: {:?}\nactual: {:?}",
                            check.expect, actual
                        )));
                    }
                }
            }
        }
        Ok(())
    }
    .await
}

#[cfg(feature = "sqlite")]
async fn run_sqlite_online_case(
    name: &str,
    case: &OnlineCase,
    section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    let harness = support::SqliteHarness::new().await?;
    async {
        if let Some(sql) = section.setup_sql(case) {
            harness.batch_execute(sql).await?;
        }
        let migrations = section.migrations(case);
        let migrator = support::build_sqlite_migrator(name, &harness, migrations)?;
        let mut migrated = false;
        let mut migration_attempted = false;
        let error_action = expected_error_action(section);
        if section.checks.contains(&OnlineCheck::Migrate) {
            let result = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate failed unexpectedly: {error}"))
            });
            migration_attempted = true;
            if error_action == Some(ExpectedErrorAction::Migrate) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::MigrateTwice) {
            let first = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: first migrate failed: {error}"))
            })?;
            let second = migrator.apply(None, false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: second migrate failed: {error}"))
            })?;
            if second.applied != 0 || second.reverted != 0 {
                return Err(TestSupportError::message(format!(
                    "{name}: second migrate should be idempotent but changed {second:?}"
                )));
            }
            migration_attempted = true;
            migrated = first.applied > 0 || first.reverted > 0 || migrated;
        }
        if section.checks.contains(&OnlineCheck::MigrateTo) {
            let target = required_target(name, section, "migrate_to")?;
            let result = migrator.apply(Some(target), false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: migrate_to failed: {error}"))
            });
            migration_attempted = true;
            if error_action == Some(ExpectedErrorAction::MigrateTo) {
                assert_error_contains(name, result.map(|_| ()), expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::Rollback) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
            }
            let target = required_target(name, section, "rollback")?;
            let result = migrator.apply(Some(target), false).await.map_err(|error| {
                TestSupportError::message(format!("{name}: rollback failed: {error}"))
            });
            if error_action == Some(ExpectedErrorAction::Rollback) {
                assert_error_contains(name, result.map(|_| ()), expected_error(name, section)?)?;
            } else {
                result?;
                migrated = true;
            }
        }
        if section.checks.contains(&OnlineCheck::LockBehavior) {
            harness.assert_lock_released().await?;
        }
        if section.checks.contains(&OnlineCheck::MigrationRecords) {
            let actual = harness.migration_records().await?;
            assert_records_match(name, actual, &section.expect_records)?;
        }
        if section.checks.contains(&OnlineCheck::Inspect) {
            let result = async {
                let actual = harness.inspect_schema().await?;
                let expected = expected_inspect_schema(name, section)?;
                support::assert_schema_matches_with_dialect(
                    name,
                    "inspected schema",
                    actual,
                    expected,
                    gaman::core::Dialect::Sqlite,
                )
            }
            .await;
            if error_action == Some(ExpectedErrorAction::Inspect) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::Verify) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
                migration_attempted = true;
                migrated = true;
            }
            if let Some(sql) = section.mutate_sql(case) {
                harness.batch_execute(sql).await?;
            }
            let result = async {
                let actual = harness.verify(&migrator).await?;
                let expected = expected_verify_ops(name, section)?;
                assert_ops_match(name, "verify operations", &actual, expected)
            }
            .await;
            if error_action == Some(ExpectedErrorAction::Verify) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::Data) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                migrator.apply(None, false).await.map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: setup migrate failed unexpectedly: {error}"
                    ))
                })?;
            }
            for check in &section.data {
                if let Some(expected) = &check.expect_error {
                    assert_error_contains(name, harness.batch_execute(&check.sql).await, expected)?;
                } else {
                    let actual = harness.fetch_strings(&check.sql).await?;
                    if actual != check.expect {
                        return Err(TestSupportError::message(format!(
                            "{name}: data check mismatch\nexpected: {:?}\nactual: {:?}",
                            check.expect, actual
                        )));
                    }
                }
            }
        }
        Ok(())
    }
    .await
}

#[cfg(not(feature = "sqlite"))]
async fn run_sqlite_online_case(
    _name: &str,
    _case: &OnlineCase,
    _section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    Err(TestSupportError::message(
        "sqlite feature is not enabled for online harness",
    ))
}

fn record_case_status(
    results: &mut OnlineSupportResults,
    name: &str,
    case: &OnlineCase,
    dialect: OnlineDialect,
    status: &CaseStatus,
) {
    for feature in &case.features {
        let dialects = results
            .features
            .get_mut(feature)
            .expect("online case features should be validated");
        let current = dialects
            .get_mut(dialect.as_str())
            .expect("online result dialect should exist");
        match status {
            CaseStatus::Success => {
                if current.status != OnlineResultStatus::Failure {
                    current.status = OnlineResultStatus::Success;
                    current.reason = None;
                }
                current.evidence.push(OnlineEvidence {
                    case: name.to_string(),
                    description: case.description.clone(),
                    checks: case
                        .dialects
                        .get(&dialect)
                        .map(|section| section.checks.clone())
                        .unwrap_or_default(),
                });
            }
            CaseStatus::Failure(message) => {
                current.status = OnlineResultStatus::Failure;
                current.reason = Some(message.clone());
                current.evidence.push(OnlineEvidence {
                    case: name.to_string(),
                    description: case.description.clone(),
                    checks: case
                        .dialects
                        .get(&dialect)
                        .map(|section| section.checks.clone())
                        .unwrap_or_default(),
                });
            }
            CaseStatus::Unimplemented(reason) => {
                if current.status == OnlineResultStatus::Unimplemented {
                    current.reason = Some(reason.clone());
                }
            }
        }
    }
}

fn write_results(path: &PathBuf, results: &OnlineSupportResults) -> Result<(), TestSupportError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| TestSupportError::Io {
            path: parent.display().to_string(),
            message: error.to_string(),
        })?;
    }
    let yaml = serde_yaml::to_string(results).map_err(|error| {
        TestSupportError::message(format!("failed to serialize results: {error}"))
    })?;
    std::fs::write(path, yaml).map_err(|error| TestSupportError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}
