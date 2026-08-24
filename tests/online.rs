mod support;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use support::{
    FeatureCatalog, MARIADB_DATABASE_URL_ENV, MYSQL_DATABASE_URL_ENV, OnlineCase, OnlineCheck,
    OnlineDialect, OnlineEvidence, OnlineFeatureResult, OnlineResultStatus, OnlineSupportResults,
    POSTGRES_DATABASE_URL_ENV, TestSupportError, assert_error_contains, case_label, case_name,
    features_path, online_cases_root, read_case_file, selected_cases,
};

struct OnlineArgs {
    dialect: Option<OnlineDialect>,
    record: PathBuf,
    explicit_record: bool,
    failure_output: Option<PathBuf>,
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
    let mut names = BTreeSet::new();
    let mut results = initial_results(&catalog);
    let mut failures = Vec::new();

    for file in files {
        let name = case_name(&file)?;
        if !names.insert(name.clone()) {
            return Err(TestSupportError::message(format!(
                "online case file stem '{name}' is duplicated"
            )));
        }
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

    if !failures.is_empty() {
        if let Some(path) = args.failure_output {
            write_results(&path, &results)?;
        }
        return Err(TestSupportError::message(format!(
            "online cases failed:\n\n{}",
            failures.join("\n\n")
        )));
    }
    write_results(&args.record, &results)?;
    if args.explicit_record {
        println!("recorded online support results: {}", args.record.display());
    }
    Ok(())
}

fn parse_args() -> Result<OnlineArgs, TestSupportError> {
    let mut dialect = None;
    let mut record = PathBuf::from("results/online-support-results.yaml");
    let mut explicit_record = false;
    let mut failure_output = None;
    let mut case_args = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--dialect" => {
                let value = raw.next().ok_or_else(|| {
                    TestSupportError::message(
                        "--dialect requires postgres, sqlite, mysql, or mariadb",
                    )
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
            "--failure-output" => {
                let value = raw
                    .next()
                    .ok_or_else(|| TestSupportError::message("--failure-output requires a path"))?;
                failure_output = Some(PathBuf::from(value));
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
        failure_output,
        case_args,
    })
}

fn parse_online_dialect(value: &str) -> Result<OnlineDialect, TestSupportError> {
    match value {
        "postgres" | "postgresql" => Ok(OnlineDialect::Postgres),
        "sqlite" | "sqlite3" => Ok(OnlineDialect::Sqlite),
        "mysql" => Ok(OnlineDialect::Mysql),
        "mariadb" => Ok(OnlineDialect::Mariadb),
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

fn expected_verification<'a>(
    name: &str,
    section: &'a support::OnlineDialectCase,
) -> Result<&'a support::ExpectedVerification, TestSupportError> {
    section.expect_verification.as_ref().ok_or_else(|| {
        TestSupportError::message(format!(
            "{name}: verify check succeeded, but the fixture has no expect_verification"
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
    FakeVerified,
}

fn expected_error_action(section: &support::OnlineDialectCase) -> Option<ExpectedErrorAction> {
    if !section.checks.contains(&OnlineCheck::Error) {
        return None;
    }
    if section.checks.contains(&OnlineCheck::FakeVerified) {
        Some(ExpectedErrorAction::FakeVerified)
    } else if section.checks.contains(&OnlineCheck::Migrate) {
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

fn required_fake_verified_target<'a>(
    name: &str,
    section: &'a support::OnlineDialectCase,
) -> Result<&'a str, TestSupportError> {
    section.fake_verified_target.as_deref().ok_or_else(|| {
        TestSupportError::message(format!(
            "{name}: fake_verified check requires fake_verified_target"
        ))
    })
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
            && section.expect_verification.is_none()
            && section.expect_error.is_none()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} verify checks require expect_verification",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::Repair)
            && section.expect_repair_operations.is_empty()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} repair checks require expect_repair_operations",
                dialect.as_str()
            )));
        }
        if section.checks.contains(&OnlineCheck::FakeVerified)
            && section.fake_verified_target.is_none()
        {
            return Err(TestSupportError::message(format!(
                "{name}: {} fake_verified checks require fake_verified_target",
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
                "{name}: {} error checks must pair with migrate, migrate_to, rollback, inspect, verify, or fake_verified",
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
    let mut results = OnlineSupportResults {
        generation: support::generation_id(),
        ..OnlineSupportResults::default()
    };
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
            run_family_online_case(
                name,
                case,
                section,
                gaman::core::Dialect::Mysql,
                MYSQL_DATABASE_URL_ENV,
                explicit_dialect,
            )
            .await
        }
        OnlineDialect::Mariadb => {
            run_family_online_case(
                name,
                case,
                section,
                gaman::core::Dialect::Mariadb,
                MARIADB_DATABASE_URL_ENV,
                explicit_dialect,
            )
            .await
        }
    }
}

#[cfg(any(feature = "mysql", feature = "mariadb"))]
async fn run_family_online_case(
    name: &str,
    case: &OnlineCase,
    section: &support::OnlineDialectCase,
    dialect: gaman::core::Dialect,
    env: &str,
    explicit: bool,
) -> CaseStatus {
    if std::env::var(env).is_err() {
        return if explicit {
            CaseStatus::Failure(format!("{env} must be set"))
        } else {
            CaseStatus::Unimplemented(format!("{env} is not set"))
        };
    }
    let harness = match support::MysqlFamilyHarness::new(dialect).await {
        Ok(harness) => harness,
        Err(error) => return CaseStatus::Failure(error.to_string()),
    };
    let result = run_family_checks(name, &harness, case, section).await;
    let cleanup = harness.cleanup().await;
    match (result, cleanup) {
        (Ok(()), Ok(())) => CaseStatus::Success,
        (Err(error), _) => CaseStatus::Failure(error.to_string()),
        (Ok(()), Err(error)) => CaseStatus::Failure(error.to_string()),
    }
}

#[cfg(not(any(feature = "mysql", feature = "mariadb")))]
async fn run_family_online_case(
    _: &str,
    _: &OnlineCase,
    _: &support::OnlineDialectCase,
    _: gaman::core::Dialect,
    _: &str,
    explicit: bool,
) -> CaseStatus {
    if explicit {
        CaseStatus::Failure("matching MySQL-family feature is not enabled".to_string())
    } else {
        CaseStatus::Unimplemented("matching MySQL-family feature is not enabled".to_string())
    }
}

#[cfg(any(feature = "mysql", feature = "mariadb"))]
async fn run_family_checks(
    name: &str,
    harness: &support::MysqlFamilyHarness,
    case: &OnlineCase,
    section: &support::OnlineDialectCase,
) -> Result<(), TestSupportError> {
    if let Some(sql) = section.setup_sql(case) {
        harness.batch_execute(sql).await?;
    }
    let migrations = section.migrations(case);
    let mut runner = support::build_mysql_family_runner(name, harness, migrations).await?;
    let mut migrated = false;
    let mut migration_attempted = false;
    let error_action = expected_error_action(section);
    if section.checks.contains(&OnlineCheck::Migrate) {
        let result = support::apply_runner(&mut runner, None)
            .await
            .map(|_| ())
            .map_err(|error| TestSupportError::message(error.to_string()));
        migration_attempted = true;
        if error_action == Some(ExpectedErrorAction::Migrate) {
            assert_error_contains(name, result, expected_error(name, section)?)?;
        } else {
            result?;
            migrated = true;
        }
    }
    if section.checks.contains(&OnlineCheck::MigrateTwice) {
        support::apply_runner(&mut runner, None)
            .await
            .map_err(|error| TestSupportError::message(error.to_string()))?;
        let second = support::apply_runner(&mut runner, None)
            .await
            .map_err(|error| TestSupportError::message(error.to_string()))?;
        if second.applied != 0 || second.reverted != 0 {
            return Err(TestSupportError::message(format!(
                "{name}: second apply was not idempotent"
            )));
        }
        migration_attempted = true;
        migrated = true;
    }
    if section.checks.contains(&OnlineCheck::MigrateTo) {
        let target = required_target(name, section, "migrate_to")?;
        let result = support::apply_runner(&mut runner, Some(target))
            .await
            .map(|_| ())
            .map_err(|error| TestSupportError::message(error.to_string()));
        migration_attempted = true;
        if error_action == Some(ExpectedErrorAction::MigrateTo) {
            assert_error_contains(name, result, expected_error(name, section)?)?;
        } else {
            result?;
            migrated = true;
        }
    }
    if section.checks.contains(&OnlineCheck::Rollback) {
        if !migrated && !migration_attempted && !migrations.is_empty() {
            support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| TestSupportError::message(error.to_string()))?;
            migration_attempted = true;
        }
        let target = required_target(name, section, "rollback")?;
        let result = support::apply_runner(&mut runner, Some(target))
            .await
            .map(|_| ())
            .map_err(|error| TestSupportError::message(error.to_string()));
        if error_action == Some(ExpectedErrorAction::Rollback) {
            assert_error_contains(name, result, expected_error(name, section)?)?;
        } else {
            result?;
            migrated = true;
        }
    }
    if section.checks.contains(&OnlineCheck::LockBehavior) {
        harness.assert_lock_released().await?;
    }
    if section.checks.contains(&OnlineCheck::Repair) {
        if !migrated && !migration_attempted && !migrations.is_empty() {
            support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| TestSupportError::message(error.to_string()))?;
            migration_attempted = true;
            migrated = true;
        }
        if let Some(sql) = section.mutate_sql(case) {
            harness.batch_execute(sql).await?;
        }
        let report = support::repair_runner(
            &mut runner,
            Vec::new(),
            section.repair_apply,
            section.repair_allow_pending,
            section.repair_sql_only,
        )
        .await
        .map_err(|error| TestSupportError::message(error.to_string()))?;
        support::assert_repair_operations(
            name,
            &report.operations,
            &section.expect_repair_operations,
        )?;
        support::assert_repair_sql(name, &report.sql, &section.expect_repair_sql)?;
        if report.applied != section.repair_apply {
            return Err(TestSupportError::message(format!(
                "{name}: repair applied flag mismatch"
            )));
        }
        if section.repair_apply && !report.verification.findings.is_empty() {
            return Err(TestSupportError::message(format!(
                "{name}: applied repair left verification findings"
            )));
        }
    }
    if section.checks.contains(&OnlineCheck::FakeVerified) {
        if let Some(sql) = section.mutate_sql(case) {
            harness.batch_execute(sql).await?;
        }
        let result = support::fake_verified_runner(
            &mut runner,
            required_fake_verified_target(name, section)?,
            Vec::new(),
        )
        .await
        .map(|_| ())
        .map_err(|error| TestSupportError::message(error.to_string()));
        if error_action == Some(ExpectedErrorAction::FakeVerified) {
            assert_error_contains(name, result, expected_error(name, section)?)?;
        } else {
            result?;
        }
    }
    if section.checks.contains(&OnlineCheck::MigrationRecords) {
        assert_records_match(
            name,
            harness.migration_records().await?,
            &section.expect_records,
        )?;
    }
    if section.checks.contains(&OnlineCheck::Inspect) {
        let result = async {
            let actual = support::inspect_runner(&mut runner, Vec::new()).await?;
            let expected = expected_inspect_schema(name, section)?;
            support::assert_inspected_schema_exact(name, actual, expected)
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
            support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| TestSupportError::message(error.to_string()))?;
            migration_attempted = true;
            migrated = true;
        }
        if let Some(sql) = section.mutate_sql(case) {
            harness.batch_execute(sql).await?;
        }
        let result = async {
            let actual = support::verify_runner(&mut runner, Vec::new()).await?;
            support::assert_verification_matches(
                name,
                &actual,
                expected_verification(name, section)?,
            )
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
            support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| TestSupportError::message(error.to_string()))?;
        }
        for check in &section.data {
            if let Some(expected) = &check.expect_error {
                assert_error_contains(name, harness.batch_execute(&check.sql).await, expected)?;
            } else {
                let actual = harness.fetch_strings(&check.sql).await?;
                if actual != check.expect {
                    return Err(TestSupportError::message(format!(
                        "{name}: data mismatch: expected {:?}, observed {:?}",
                        check.expect, actual
                    )));
                }
            }
        }
    }
    Ok(())
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
        let mut runner = support::build_postgres_runner(name, harness, migrations).await?;
        let mut migrated = false;
        let mut migration_attempted = false;
        let error_action = expected_error_action(section);
        if section.checks.contains(&OnlineCheck::Migrate) {
            let result = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: migrate failed unexpectedly: {error}"
                    ))
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
            let first = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
                    TestSupportError::message(format!("{name}: first migrate failed: {error}"))
                })?;
            let second = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
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
            let result = support::apply_runner(&mut runner, Some(target))
                .await
                .map_err(|error| {
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
                        TestSupportError::message(format!(
                            "{name}: setup migrate failed unexpectedly: {error}"
                        ))
                    })?;
            }
            let target = required_target(name, section, "rollback")?;
            let result = support::apply_runner(&mut runner, Some(target))
                .await
                .map_err(|error| {
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
        if section.checks.contains(&OnlineCheck::Repair) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| TestSupportError::message(error.to_string()))?;
                migration_attempted = true;
                migrated = true;
            }
            if let Some(sql) = section.mutate_sql(case) {
                let sql = support::postgres_placeholder_text(sql, harness.schema_name());
                harness.batch_execute(&sql).await?;
            }
            let report = support::repair_runner(
                &mut runner,
                vec![harness.schema_name().to_string()],
                section.repair_apply,
                section.repair_allow_pending,
                section.repair_sql_only,
            )
            .await
            .map_err(|error| TestSupportError::message(error.to_string()))?;
            support::assert_repair_operations(
                name,
                &report.operations,
                &section.expect_repair_operations,
            )?;
            support::assert_repair_sql(name, &report.sql, &section.expect_repair_sql)?;
            if report.applied != section.repair_apply {
                return Err(TestSupportError::message(format!(
                    "{name}: repair applied flag mismatch"
                )));
            }
            if section.repair_apply && !report.verification.findings.is_empty() {
                return Err(TestSupportError::message(format!(
                    "{name}: applied repair left verification findings"
                )));
            }
        }
        if section.checks.contains(&OnlineCheck::FakeVerified) {
            if let Some(sql) = section.mutate_sql(case) {
                let sql = support::postgres_placeholder_text(sql, harness.schema_name());
                harness.batch_execute(&sql).await?;
            }
            let result = support::fake_verified_runner(
                &mut runner,
                required_fake_verified_target(name, section)?,
                vec![harness.schema_name().to_string()],
            )
            .await
            .map(|_| ())
            .map_err(|error| TestSupportError::message(error.to_string()));
            if error_action == Some(ExpectedErrorAction::FakeVerified) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::MigrationRecords) {
            let actual = harness.migration_records().await?;
            assert_records_match(name, actual, &section.expect_records)?;
        }
        if section.checks.contains(&OnlineCheck::Inspect) {
            let result = async {
                let mut actual =
                    support::inspect_runner(&mut runner, vec![harness.schema_name().to_string()])
                        .await?;
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
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
                let actual =
                    support::verify_runner(&mut runner, vec![harness.schema_name().to_string()])
                        .await?;
                support::assert_verification_matches(
                    name,
                    &actual,
                    expected_verification(name, section)?,
                )
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
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
        let mut runner = support::build_sqlite_runner(name, &harness, migrations).await?;
        let mut migrated = false;
        let mut migration_attempted = false;
        let error_action = expected_error_action(section);
        if section.checks.contains(&OnlineCheck::Migrate) {
            let result = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
                    TestSupportError::message(format!(
                        "{name}: migrate failed unexpectedly: {error}"
                    ))
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
            let first = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
                    TestSupportError::message(format!("{name}: first migrate failed: {error}"))
                })?;
            let second = support::apply_runner(&mut runner, None)
                .await
                .map_err(|error| {
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
            let result = support::apply_runner(&mut runner, Some(target))
                .await
                .map_err(|error| {
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
                        TestSupportError::message(format!(
                            "{name}: setup migrate failed unexpectedly: {error}"
                        ))
                    })?;
            }
            let target = required_target(name, section, "rollback")?;
            let result = support::apply_runner(&mut runner, Some(target))
                .await
                .map_err(|error| {
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
        if section.checks.contains(&OnlineCheck::Repair) {
            if !migrated && !migration_attempted && !migrations.is_empty() {
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| TestSupportError::message(error.to_string()))?;
                migration_attempted = true;
                migrated = true;
            }
            if let Some(sql) = section.mutate_sql(case) {
                harness.batch_execute(sql).await?;
            }
            let report = support::repair_runner(
                &mut runner,
                Vec::new(),
                section.repair_apply,
                section.repair_allow_pending,
                section.repair_sql_only,
            )
            .await
            .map_err(|error| TestSupportError::message(error.to_string()))?;
            support::assert_repair_operations(
                name,
                &report.operations,
                &section.expect_repair_operations,
            )?;
            support::assert_repair_sql(name, &report.sql, &section.expect_repair_sql)?;
            if report.applied != section.repair_apply {
                return Err(TestSupportError::message(format!(
                    "{name}: repair applied flag mismatch"
                )));
            }
            if section.repair_apply && !report.verification.findings.is_empty() {
                return Err(TestSupportError::message(format!(
                    "{name}: applied repair left verification findings"
                )));
            }
        }
        if section.checks.contains(&OnlineCheck::FakeVerified) {
            if let Some(sql) = section.mutate_sql(case) {
                harness.batch_execute(sql).await?;
            }
            let result = support::fake_verified_runner(
                &mut runner,
                required_fake_verified_target(name, section)?,
                Vec::new(),
            )
            .await
            .map(|_| ())
            .map_err(|error| TestSupportError::message(error.to_string()));
            if error_action == Some(ExpectedErrorAction::FakeVerified) {
                assert_error_contains(name, result, expected_error(name, section)?)?;
            } else {
                result?;
            }
        }
        if section.checks.contains(&OnlineCheck::MigrationRecords) {
            let actual = harness.migration_records().await?;
            assert_records_match(name, actual, &section.expect_records)?;
        }
        if section.checks.contains(&OnlineCheck::Inspect) {
            let result = async {
                let actual = support::inspect_runner(&mut runner, Vec::new()).await?;
                let expected = expected_inspect_schema(name, section)?;
                support::assert_inspected_schema_exact(name, actual, expected)
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
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
                let actual = support::verify_runner(&mut runner, Vec::new()).await?;
                support::assert_verification_matches(
                    name,
                    &actual,
                    expected_verification(name, section)?,
                )
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
                support::apply_runner(&mut runner, None)
                    .await
                    .map_err(|error| {
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

fn write_results(path: &Path, results: &OnlineSupportResults) -> Result<(), TestSupportError> {
    support::write_yaml_atomic(path, results).map_err(|error| TestSupportError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}
