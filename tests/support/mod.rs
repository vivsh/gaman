#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
#[cfg(feature = "postgres")]
use std::sync::atomic::{AtomicU32, Ordering};

use gaman::Config;
use gaman::Migration;
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use gaman::core::Introspectable;
use gaman::core::{
    BoxFuture, Clarification, Decision, Dialect, Environment, EnvironmentError,
    EnvironmentExecutor, Migrator, TRACKING_TABLE, VecAdapter,
};
#[cfg(feature = "postgres")]
use gaman::core::{Executor, ExecutorError, PostgresExecutor};
use gaman::schema::{Operation, Schema};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
#[cfg(any(feature = "postgres", feature = "sqlite"))]
use sqlx::ConnectOptions;
#[cfg(feature = "postgres")]
use sqlx::Row;
#[cfg(feature = "postgres")]
use sqlx::postgres::{PgConnectOptions, PgSslMode};
#[cfg(feature = "sqlite")]
use sqlx::sqlite::SqliteConnectOptions;
use thiserror::Error;

#[cfg(feature = "postgres")]
static COUNTER: AtomicU32 = AtomicU32::new(0);
pub const POSTGRES_DATABASE_URL_ENV: &str = "POSTGRES_DATABASE_URL";
pub const SQLITE_DATABASE_URL_ENV: &str = "SQLITE_DATABASE_URL";
pub const MYSQL_DATABASE_URL_ENV: &str = "MYSQL_DATABASE_URL";

#[derive(Debug, Error)]
pub enum TestSupportError {
    #[error("I/O error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FixtureDialect {
    #[default]
    Postgres,
    Sqlite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineDialect {
    Postgres,
    Sqlite,
    Mysql,
}

impl OnlineDialect {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
        }
    }

    pub fn all() -> [Self; 3] {
        [Self::Postgres, Self::Sqlite, Self::Mysql]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineCheck {
    Migrate,
    MigrateTwice,
    MigrateTo,
    Rollback,
    MigrationRecords,
    LockBehavior,
    Inspect,
    Verify,
    Data,
    Error,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlineCase {
    pub description: String,
    pub features: Vec<String>,
    #[serde(default)]
    pub migrations: Vec<InlineMigration>,
    #[serde(default)]
    pub setup_sql: Option<String>,
    #[serde(default)]
    pub mutate_sql: Option<String>,
    pub dialects: std::collections::BTreeMap<OnlineDialect, OnlineDialectCase>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlineDialectCase {
    pub checks: Vec<OnlineCheck>,
    /// PostgreSQL extensions required for this case to be meaningful.
    ///
    /// The harness records unavailable extensions as host capability gaps
    /// rather than treating them as a Gaman regression.
    #[serde(default)]
    pub requires_extensions: Vec<String>,
    #[serde(default)]
    pub migrations: Option<Vec<InlineMigration>>,
    #[serde(default)]
    pub setup_sql: Option<String>,
    #[serde(default)]
    pub mutate_sql: Option<String>,
    #[serde(default)]
    pub expect_schema: Option<Schema>,
    #[serde(default)]
    pub expect_extensions: Vec<String>,
    #[serde(default)]
    pub expect_verify: Option<Vec<Operation>>,
    #[serde(default)]
    pub expect_error: Option<String>,
    #[serde(default)]
    pub target: Option<String>,
    #[serde(default)]
    pub expect_records: Vec<String>,
    #[serde(default)]
    pub data: Vec<OnlineDataCheck>,
}

impl OnlineDialectCase {
    pub fn migrations<'a>(&'a self, case: &'a OnlineCase) -> &'a [InlineMigration] {
        self.migrations.as_deref().unwrap_or(&case.migrations)
    }

    pub fn setup_sql<'a>(&'a self, case: &'a OnlineCase) -> Option<&'a str> {
        self.setup_sql.as_deref().or(case.setup_sql.as_deref())
    }

    pub fn mutate_sql<'a>(&'a self, case: &'a OnlineCase) -> Option<&'a str> {
        self.mutate_sql.as_deref().or(case.mutate_sql.as_deref())
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OnlineDataCheck {
    pub sql: String,
    #[serde(default)]
    pub expect: Vec<String>,
    #[serde(default)]
    pub expect_error: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureCatalog {
    pub features: Vec<FeatureEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FeatureEntry {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineFeatureCatalog {
    pub features: Vec<OfflineFeatureEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineFeatureEntry {
    pub id: String,
    pub label: String,
    pub category: String,
    #[serde(default)]
    pub dialect: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct OfflineSupportResults {
    pub features: BTreeMap<String, OfflineFeatureResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineFeatureResult {
    pub status: OfflineResultStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<OfflineEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OfflineResultStatus {
    Success,
    Failure,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfflineEvidence {
    pub case: String,
    pub description: String,
    pub group: String,
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dialect: Option<String>,
}

#[derive(Debug, Default, Serialize)]
pub struct OnlineSupportResults {
    pub features:
        std::collections::BTreeMap<String, std::collections::BTreeMap<String, OnlineFeatureResult>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlineFeatureResult {
    pub status: OnlineResultStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<OnlineEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OnlineResultStatus {
    Success,
    Failure,
    Unimplemented,
}

#[derive(Debug, Clone, Serialize)]
pub struct OnlineEvidence {
    pub case: String,
    pub description: String,
    pub checks: Vec<OnlineCheck>,
}

impl FixtureDialect {
    pub fn to_dialect(self) -> Result<Dialect, TestSupportError> {
        match self {
            Self::Postgres => Ok(Dialect::Postgres),
            #[cfg(feature = "sqlite")]
            Self::Sqlite => Ok(Dialect::Sqlite),
            #[cfg(not(feature = "sqlite"))]
            Self::Sqlite => Err(TestSupportError::Message(
                "sqlite fixture requires the sqlite feature".to_string(),
            )),
        }
    }

    pub fn is_available(self) -> bool {
        match self {
            Self::Postgres => true,
            Self::Sqlite => cfg!(feature = "sqlite"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParserFixtureDialect {
    #[default]
    Postgres,
    Sqlite,
    Mysql,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ParseExpectation {
    Ok,
    Error,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoweringExpectation {
    Ok,
    Unsupported,
    Error,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SqlDirection {
    #[default]
    Forward,
    Rollback,
}

struct FixtureEnvironment {
    config: Arc<Config>,
    dialect: Dialect,
}

impl FixtureEnvironment {
    fn new(config: Arc<Config>, dialect: Dialect) -> Self {
        Self { config, dialect }
    }
}

impl Environment for FixtureEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        Box::pin(async {
            Err(EnvironmentError::Config(
                "executor is not available in the fixture environment".into(),
            ))
        })
    }

    fn dialect(&self) -> Dialect {
        self.dialect
    }
}

#[cfg(feature = "postgres")]
struct PostgresHarnessEnvironment {
    config: Arc<Config>,
    schema: String,
}

#[cfg(feature = "postgres")]
impl PostgresHarnessEnvironment {
    fn new(url: &str, schema: &str) -> Self {
        let config = Config::new(
            url.to_string(),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
            Dialect::Postgres,
        );
        Self {
            config: Arc::new(config),
            schema: schema.to_string(),
        }
    }
}

#[cfg(feature = "sqlite")]
struct SqliteHarnessEnvironment {
    config: Arc<Config>,
}

#[cfg(feature = "sqlite")]
impl SqliteHarnessEnvironment {
    fn new(url: &str) -> Self {
        let config = Config::new(
            url.to_string(),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
            Dialect::Sqlite,
        );
        Self {
            config: Arc::new(config),
        }
    }
}

#[cfg(feature = "sqlite")]
impl Environment for SqliteHarnessEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        let url = self.config.database_url.clone();
        Box::pin(async move {
            let opts = sqlite_connect_options(&url).map_err(EnvironmentError::Connect)?;
            let conn = opts
                .connect()
                .await
                .map_err(|e| EnvironmentError::Connect(e.to_string()))?;
            Ok(Box::new(gaman::core::SqliteExecutor::new(conn))
                as Box<dyn EnvironmentExecutor + Send>)
        })
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }
}

#[cfg(feature = "postgres")]
impl Environment for PostgresHarnessEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        let url = self.config.database_url.clone();
        let schema = self.schema.clone();
        Box::pin(async move {
            let opts = url
                .parse::<PgConnectOptions>()
                .map_err(|e| EnvironmentError::Connect(e.to_string()))?
                .ssl_mode(PgSslMode::Disable);
            let mut conn = opts
                .connect()
                .await
                .map_err(|e| EnvironmentError::Connect(e.to_string()))?;
            sqlx::query(&format!("SET search_path TO \"{schema}\""))
                .execute(&mut conn)
                .await
                .map_err(|e| {
                    EnvironmentError::Connect(format!("failed to set search_path: {e}"))
                })?;
            Ok(Box::new(PostgresExecutor::new(conn)) as Box<dyn EnvironmentExecutor + Send>)
        })
    }

    fn dialect(&self) -> Dialect {
        Dialect::Postgres
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InlineMigration {
    pub id: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub operations: Vec<Operation>,
    #[serde(default = "default_atomic")]
    pub atomic: bool,
}

#[derive(Debug, Deserialize)]
pub struct OfflineCase {
    pub description: String,
    pub group: String,
    pub features: Vec<String>,
    #[serde(default)]
    pub dialect: FixtureDialect,
    #[serde(flatten)]
    pub spec: OfflineSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct ExpectedDriftFinding {
    pub operation: String,
    pub entity_kind: String,
    pub entity_name: String,
    pub property: String,
    pub expected: String,
    pub observed: String,
    #[serde(default)]
    pub note: Option<String>,
}

impl OfflineCase {
    pub fn validate(&self, case_name: &str, path: &Path) -> Result<(), TestSupportError> {
        if self.description.trim().is_empty() {
            return Err(TestSupportError::message(format!(
                "{case_name}: offline fixture description must not be empty"
            )));
        }
        if self.group.trim().is_empty() {
            return Err(TestSupportError::message(format!(
                "{case_name}: offline fixture group must not be empty"
            )));
        }
        if self.features.is_empty() {
            return Err(TestSupportError::message(format!(
                "{case_name}: offline fixture must list at least one feature"
            )));
        }
        self.spec.validate(case_name)?;
        self.spec.validate_group(case_name, &self.group, path)
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OfflineSpec {
    Parser {
        parser_dialect: ParserFixtureDialect,
        sql: String,
        expect_parse: ParseExpectation,
        expect_lowering: LoweringExpectation,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    SqlToSchema {
        sql: String,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    SqlSchemaToMigration {
        name: String,
        sql: String,
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        #[serde(default)]
        decisions: Vec<Decision>,
        #[serde(default)]
        expect_no_changes: bool,
        expect_clarifications: Option<Vec<Clarification>>,
        expect_pending_clarifications: Option<Vec<Clarification>>,
        expect_operations: Option<Vec<Operation>>,
        expect_schema: Option<Schema>,
        expect_sql: Option<String>,
        expect_error: Option<String>,
    },
    SchemaToMigration {
        name: String,
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        current: Schema,
        #[serde(default)]
        decisions: Vec<Decision>,
        #[serde(default)]
        expect_no_changes: bool,
        expect_clarifications: Option<Vec<Clarification>>,
        expect_pending_clarifications: Option<Vec<Clarification>>,
        expect_operations: Option<Vec<Operation>>,
        expect_sql: Option<String>,
        expect_error: Option<String>,
    },
    MigrationToReplay {
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    MigrationToSql {
        #[serde(default)]
        direction: SqlDirection,
        #[serde(default)]
        ids: Vec<String>,
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        expect_sql: Option<String>,
        expect_error: Option<String>,
    },
    Verify {
        #[serde(default = "default_verify_schema")]
        schema: String,
        replayed: Schema,
        inspected: Schema,
        expect_findings: Option<Vec<ExpectedDriftFinding>>,
        expect_operations: Option<Vec<Operation>>,
        expect_report: Option<Vec<String>>,
        expect_error: Option<String>,
    },
    EndToEnd {
        name: String,
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        current: Schema,
        #[serde(default)]
        decisions: Vec<Decision>,
        expect_operations: Option<Vec<Operation>>,
        expect_schema: Option<Schema>,
        expect_sql: Option<String>,
        expect_error: Option<String>,
    },
}

impl OfflineSpec {
    fn validate(&self, case_name: &str) -> Result<(), TestSupportError> {
        match self {
            Self::Parser {
                expect_parse,
                expect_lowering,
                expect_schema,
                expect_error,
                ..
            } => {
                if *expect_parse == ParseExpectation::Error
                    && *expect_lowering != LoweringExpectation::Error
                {
                    return Err(invalid_fixture(
                        case_name,
                        "parse-error fixtures must set expect_lowering: error",
                    ));
                }
                if expect_schema.is_some() && *expect_lowering != LoweringExpectation::Ok {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_schema requires expect_lowering: ok",
                    ));
                }
                if expect_error.is_some()
                    && *expect_parse == ParseExpectation::Ok
                    && *expect_lowering == LoweringExpectation::Ok
                {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_error is only valid for parse, unsupported, or lowering failures",
                    ));
                }
            }
            Self::SqlToSchema {
                expect_schema,
                expect_error,
                ..
            }
            | Self::MigrationToReplay {
                expect_schema,
                expect_error,
                ..
            } => {
                expect_one_of(
                    case_name,
                    "expect_schema",
                    expect_schema.is_some(),
                    "expect_error",
                    expect_error.is_some(),
                )?;
            }
            Self::SchemaToMigration {
                expect_no_changes,
                expect_clarifications,
                expect_pending_clarifications,
                expect_operations,
                expect_sql,
                expect_error,
                ..
            } => {
                if expect_clarifications.is_some() && expect_pending_clarifications.is_some() {
                    return Err(invalid_fixture(
                        case_name,
                        "use either expect_clarifications or expect_pending_clarifications, not both",
                    ));
                }
                let expects_generated = expect_operations.is_some() || expect_sql.is_some();
                let expects_clarification =
                    expect_clarifications.is_some() || expect_pending_clarifications.is_some();
                if *expect_no_changes
                    && (expects_generated || expects_clarification || expect_error.is_some())
                {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_no_changes cannot be combined with generated, clarification, or error expectations",
                    ));
                }
                if expect_error.is_some() && (expects_generated || expects_clarification) {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_error cannot be combined with generated or clarification expectations",
                    ));
                }
                if expects_clarification && expect_sql.is_some() {
                    return Err(invalid_fixture(
                        case_name,
                        "clarification fixtures cannot also expect SQL",
                    ));
                }
            }
            Self::SqlSchemaToMigration {
                expect_no_changes,
                expect_clarifications,
                expect_pending_clarifications,
                expect_operations,
                expect_schema,
                expect_sql,
                expect_error,
                ..
            } => {
                validate_migration_expectations(
                    case_name,
                    *expect_no_changes,
                    expect_clarifications,
                    expect_pending_clarifications,
                    expect_operations,
                    expect_schema,
                    expect_sql,
                    expect_error,
                )?;
            }
            Self::MigrationToSql {
                expect_sql,
                expect_error,
                ..
            } => {
                expect_one_of(
                    case_name,
                    "expect_sql",
                    expect_sql.is_some(),
                    "expect_error",
                    expect_error.is_some(),
                )?;
            }
            Self::Verify {
                expect_findings,
                expect_operations,
                expect_report,
                expect_error,
                ..
            } => {
                if expect_error.is_some()
                    && (expect_findings.is_some()
                        || expect_operations.is_some()
                        || expect_report.is_some())
                {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_error cannot be combined with verify success expectations",
                    ));
                }
                if expect_error.is_none()
                    && expect_findings.is_none()
                    && expect_operations.is_none()
                    && expect_report.is_none()
                {
                    return Err(invalid_fixture(
                        case_name,
                        "verify requires at least one success expectation or expect_error",
                    ));
                }
            }
            Self::EndToEnd {
                expect_operations,
                expect_schema,
                expect_sql,
                expect_error,
                ..
            } => {
                if expect_error.is_some()
                    && (expect_operations.is_some()
                        || expect_schema.is_some()
                        || expect_sql.is_some())
                {
                    return Err(invalid_fixture(
                        case_name,
                        "expect_error cannot be combined with end-to-end success expectations",
                    ));
                }
                if expect_error.is_none()
                    && expect_operations.is_none()
                    && expect_schema.is_none()
                    && expect_sql.is_none()
                {
                    return Err(invalid_fixture(
                        case_name,
                        "end_to_end requires at least one success expectation or expect_error",
                    ));
                }
            }
        }
        Ok(())
    }

    fn validate_group(
        &self,
        case_name: &str,
        group: &str,
        _path: &Path,
    ) -> Result<(), TestSupportError> {
        if group == "clarifier" && !matches!(self, Self::SchemaToMigration { .. }) {
            return Err(invalid_fixture(
                case_name,
                "clarifier fixtures must use kind: schema_to_migration",
            ));
        }
        if group == "rollback"
            && !matches!(
                self,
                Self::MigrationToSql {
                    direction: SqlDirection::Rollback,
                    ..
                }
            )
        {
            return Err(invalid_fixture(
                case_name,
                "fixtures under rollback must use kind: migration_to_sql and direction: rollback",
            ));
        }
        Ok(())
    }
}

#[allow(clippy::too_many_arguments)]
fn validate_migration_expectations(
    case_name: &str,
    expect_no_changes: bool,
    expect_clarifications: &Option<Vec<Clarification>>,
    expect_pending_clarifications: &Option<Vec<Clarification>>,
    expect_operations: &Option<Vec<Operation>>,
    expect_schema: &Option<Schema>,
    expect_sql: &Option<String>,
    expect_error: &Option<String>,
) -> Result<(), TestSupportError> {
    if expect_clarifications.is_some() && expect_pending_clarifications.is_some() {
        return Err(invalid_fixture(
            case_name,
            "use either expect_clarifications or expect_pending_clarifications, not both",
        ));
    }
    let expects_generated =
        expect_operations.is_some() || expect_schema.is_some() || expect_sql.is_some();
    let expects_clarification =
        expect_clarifications.is_some() || expect_pending_clarifications.is_some();
    if expect_no_changes && (expects_generated || expects_clarification || expect_error.is_some()) {
        return Err(invalid_fixture(
            case_name,
            "expect_no_changes cannot be combined with generated, clarification, or error expectations",
        ));
    }
    if expect_error.is_some() && (expects_generated || expects_clarification) {
        return Err(invalid_fixture(
            case_name,
            "expect_error cannot be combined with generated or clarification expectations",
        ));
    }
    if expects_clarification && (expect_schema.is_some() || expect_sql.is_some()) {
        return Err(invalid_fixture(
            case_name,
            "clarification fixtures cannot also expect schema or SQL",
        ));
    }
    if !expect_no_changes && !expects_generated && !expects_clarification && expect_error.is_none()
    {
        return Err(invalid_fixture(
            case_name,
            "migration fixture requires a generated, clarification, no-change, or error expectation",
        ));
    }
    Ok(())
}

fn invalid_fixture(case_name: &str, reason: &str) -> TestSupportError {
    TestSupportError::message(format!("{case_name}: invalid offline fixture: {reason}"))
}

fn expect_one_of(
    case_name: &str,
    left_name: &str,
    left: bool,
    right_name: &str,
    right: bool,
) -> Result<(), TestSupportError> {
    match (left, right) {
        (true, false) | (false, true) => Ok(()),
        (false, false) => Err(invalid_fixture(
            case_name,
            &format!("expected either {left_name} or {right_name}"),
        )),
        (true, true) => Err(invalid_fixture(
            case_name,
            &format!("cannot combine {left_name} and {right_name}"),
        )),
    }
}

fn default_atomic() -> bool {
    true
}

fn default_verify_schema() -> String {
    "public".to_string()
}

impl InlineMigration {
    pub fn to_migration(&self) -> Migration {
        Migration {
            id: self.id.clone(),
            dependencies: self.dependencies.clone(),
            operations: self.operations.clone(),
            atomic: self.atomic,
        }
    }
}

impl TestSupportError {
    fn io(path: &Path, error: impl Display) -> Self {
        Self::Io {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    fn parse(path: &Path, error: impl Display) -> Self {
        Self::Parse {
            path: path.display().to_string(),
            message: error.to_string(),
        }
    }

    pub fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

pub fn offline_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/offline")
}

pub fn offline_features_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/offline-features.yaml")
}

pub fn offline_results_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("results/offline-results.yaml")
}

pub fn online_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/online")
}

pub fn features_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/features.yaml")
}

pub fn online_results_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("results/online-results.yaml")
}

pub fn selected_cases(root: &Path, args: &[String]) -> Result<Vec<PathBuf>, TestSupportError> {
    let root = root
        .canonicalize()
        .map_err(|error| TestSupportError::io(root, error))?;
    let mut files = Vec::new();
    if args.is_empty() {
        discover_cases_into(&root, &mut files)?;
    } else {
        for arg in args {
            if arg.starts_with('-') {
                return Err(TestSupportError::message(format!(
                    "unsupported harness argument '{arg}'; pass YAML case files or directories"
                )));
            }
            if contains_glob_meta(arg) {
                expand_case_glob(&root, arg, &mut files)?;
                continue;
            }
            let input = Path::new(arg);
            let path = if input.is_absolute() {
                input.to_path_buf()
            } else {
                Path::new(env!("CARGO_MANIFEST_DIR")).join(input)
            };
            let path = path
                .canonicalize()
                .map_err(|error| TestSupportError::io(&path, error))?;
            ensure_inside_root(&root, &path)?;
            if path.is_dir() {
                discover_cases_into(&path, &mut files)?;
            } else if is_case_file(&path) {
                files.push(path);
            } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                return Err(TestSupportError::message(format!(
                    "case path '{}' is a harness metadata file, not a case file",
                    path.display()
                )));
            } else {
                return Err(TestSupportError::message(format!(
                    "case path '{}' is not a YAML file",
                    path.display()
                )));
            }
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

fn expand_case_glob(
    root: &Path,
    pattern: &str,
    files: &mut Vec<PathBuf>,
) -> Result<(), TestSupportError> {
    let input = Path::new(pattern);
    let path = if input.is_absolute() {
        input.to_path_buf()
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join(input)
    };
    let (base, components) = split_glob_pattern(&path);
    let base = base
        .canonicalize()
        .map_err(|error| TestSupportError::io(&base, error))?;
    ensure_inside_root(root, &base)?;

    let before = files.len();
    expand_glob_components(&base, &components, files)?;
    if files.len() == before {
        return Err(TestSupportError::message(format!(
            "case glob '{pattern}' did not match any YAML case files"
        )));
    }
    Ok(())
}

fn split_glob_pattern(pattern: &Path) -> (PathBuf, Vec<String>) {
    let mut base = PathBuf::new();
    let mut components = Vec::new();
    let mut in_glob = false;
    for component in pattern.components() {
        let text = component.as_os_str().to_string_lossy().to_string();
        if !in_glob && !contains_glob_meta(&text) {
            base.push(component.as_os_str());
        } else {
            in_glob = true;
            components.push(text);
        }
    }
    (base, components)
}

fn expand_glob_components(
    current: &Path,
    components: &[String],
    files: &mut Vec<PathBuf>,
) -> Result<(), TestSupportError> {
    let Some((head, tail)) = components.split_first() else {
        if current.is_dir() {
            discover_cases_into(current, files)?;
        } else if is_case_file(current) {
            files.push(current.to_path_buf());
        }
        return Ok(());
    };

    if contains_glob_meta(head) {
        for entry in fs::read_dir(current).map_err(|error| TestSupportError::io(current, error))? {
            let entry = entry.map_err(|error| TestSupportError::io(current, error))?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if glob_component_matches(head, &name) {
                expand_glob_components(&entry.path(), tail, files)?;
            }
        }
    } else {
        let next = current.join(head);
        if next.exists() {
            expand_glob_components(&next, tail, files)?;
        }
    }
    Ok(())
}

fn contains_glob_meta(value: &str) -> bool {
    value.contains('*') || value.contains('?')
}

fn glob_component_matches(pattern: &str, value: &str) -> bool {
    glob_component_matches_inner(pattern.as_bytes(), value.as_bytes())
}

fn glob_component_matches_inner(pattern: &[u8], value: &[u8]) -> bool {
    match pattern.split_first() {
        None => value.is_empty(),
        Some((&b'*', rest)) => {
            glob_component_matches_inner(rest, value)
                || value
                    .split_first()
                    .is_some_and(|(_, tail)| glob_component_matches_inner(pattern, tail))
        }
        Some((&b'?', rest)) => value
            .split_first()
            .is_some_and(|(_, tail)| glob_component_matches_inner(rest, tail)),
        Some((&expected, rest)) => value.split_first().is_some_and(|(&actual, tail)| {
            actual == expected && glob_component_matches_inner(rest, tail)
        }),
    }
}

pub fn discover_cases(root: &Path) -> Result<Vec<PathBuf>, TestSupportError> {
    let mut files = Vec::new();
    discover_cases_into(root, &mut files)?;
    files.sort();
    files.dedup();
    Ok(files)
}

fn discover_cases_into(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), TestSupportError> {
    if !root.exists() {
        return Ok(());
    }

    for entry in fs::read_dir(root).map_err(|error| TestSupportError::io(root, error))? {
        let entry = entry.map_err(|error| TestSupportError::io(root, error))?;
        let path = entry.path();
        if path.is_dir() {
            discover_cases_into(&path, files)?;
        } else if is_case_file(&path) {
            files.push(path);
        }
    }

    Ok(())
}

fn is_case_file(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("yaml")
}

fn ensure_inside_root(root: &Path, path: &Path) -> Result<(), TestSupportError> {
    if path.starts_with(root) {
        Ok(())
    } else {
        Err(TestSupportError::message(format!(
            "case path '{}' is outside harness root '{}'",
            path.display(),
            root.display()
        )))
    }
}

pub fn case_name(path: &Path) -> Result<String, TestSupportError> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| TestSupportError::parse(path, "case file name is not valid UTF-8"))
}

pub fn case_label(case_name: &str, description: Option<&str>) -> String {
    description.unwrap_or(case_name).to_string()
}

pub fn read_case_file<T: DeserializeOwned>(path: &Path) -> Result<T, TestSupportError> {
    let raw = fs::read_to_string(path).map_err(|error| TestSupportError::io(path, error))?;
    serde_yaml::from_str(&raw).map_err(|error| TestSupportError::parse(path, error))
}

pub fn run_case_set<F>(
    harness_name: &str,
    files: Vec<PathBuf>,
    mut run_one: F,
) -> Result<(), TestSupportError>
where
    F: FnMut(&Path, &str) -> Result<String, TestSupportError>,
{
    if files.is_empty() {
        return Err(TestSupportError::message(format!(
            "{harness_name}: no case files selected"
        )));
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
        match run_one(&file, &name) {
            Ok(label) => println!("  ok: {label} ({})", file.display()),
            Err(error) => failures.push(format!("{}: {error}", file.display())),
        }
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(TestSupportError::message(format!(
            "{harness_name} cases failed:\n\n{}",
            failures.join("\n\n")
        )))
    }
}

pub fn build_migrator(
    case_name: &str,
    dialect: FixtureDialect,
    migrations: &[InlineMigration],
) -> Result<Migrator, TestSupportError> {
    let dialect = dialect.to_dialect()?;
    let config = Arc::new(Config::new(
        "postgres:///".to_string(),
        PathBuf::from("migrations"),
        PathBuf::from("schema.yaml"),
        dialect,
    ));
    let source = Box::new(VecAdapter::new(to_migrations(migrations)));
    let environment = Box::new(FixtureEnvironment::new(config, dialect));
    Migrator::new(source, environment).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to construct migrator: {error}"
        ))
    })
}

#[cfg(feature = "postgres")]
pub fn build_postgres_migrator(
    case_name: &str,
    harness: &PgHarness,
    migrations: &[InlineMigration],
) -> Result<Migrator, TestSupportError> {
    let migrations = postgres_placeholder_migrations(migrations, &harness.schema)?;
    let source = Box::new(VecAdapter::new(to_migrations(&migrations)));
    let environment = Box::new(PostgresHarnessEnvironment::new(
        &harness.url,
        &harness.schema,
    ));
    Migrator::new(source, environment).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to construct migrator: {error}"
        ))
    })
}

#[cfg(feature = "postgres")]
pub fn postgres_placeholder_text(input: &str, schema: &str) -> String {
    input.replace("{{schema}}", schema)
}

#[cfg(feature = "postgres")]
fn postgres_placeholder_migrations(
    migrations: &[InlineMigration],
    schema: &str,
) -> Result<Vec<InlineMigration>, TestSupportError> {
    let yaml = serde_yaml::to_string(migrations).map_err(|error| {
        TestSupportError::message(format!("failed to serialize online migrations: {error}"))
    })?;
    let yaml = postgres_placeholder_text(&yaml, schema);
    serde_yaml::from_str(&yaml).map_err(|error| {
        TestSupportError::message(format!("failed to deserialize online migrations: {error}"))
    })
}

#[cfg(feature = "sqlite")]
pub fn build_sqlite_migrator(
    case_name: &str,
    harness: &SqliteHarness,
    migrations: &[InlineMigration],
) -> Result<Migrator, TestSupportError> {
    let source = Box::new(VecAdapter::new(to_migrations(migrations)));
    let environment = Box::new(SqliteHarnessEnvironment::new(&harness.url));
    Migrator::new(source, environment).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to construct migrator: {error}"
        ))
    })
}

pub fn ordered_migrations(
    case_name: &str,
    migrator: &Migrator,
) -> Result<Vec<Migration>, TestSupportError> {
    let mut ordered = Vec::new();
    let ids = migrator.graph.topological_order().map_err(|error| {
        TestSupportError::message(format!("{case_name}: topological ordering failed: {error}"))
    })?;

    for id in ids {
        let migration = migrator.graph.get(id).cloned().ok_or_else(|| {
            TestSupportError::message(format!("{case_name}: graph is missing migration '{id}'"))
        })?;
        ordered.push(migration);
    }

    Ok(ordered)
}

pub fn replay_schema(case_name: &str, migrator: &Migrator) -> Result<Schema, TestSupportError> {
    let mut replay = Schema::default();
    let ids = migrator.graph.topological_order().map_err(|error| {
        TestSupportError::message(format!("{case_name}: topological ordering failed: {error}"))
    })?;

    for id in ids {
        let migration = migrator.graph.get(id).ok_or_else(|| {
            TestSupportError::message(format!("{case_name}: graph is missing migration '{id}'"))
        })?;
        for (index, op) in migration.operations.iter().enumerate() {
            replay.apply(op).map_err(|error| {
                TestSupportError::message(format!(
                    "{case_name}: replay failed for migration '{}' operation {} ({}): {}",
                    migration.id,
                    index + 1,
                    op.type_name(),
                    error,
                ))
            })?;
        }
    }

    canonicalize_schema(&mut replay, migrator.dialect());
    Ok(replay)
}

pub fn assert_schema_matches(
    case_name: &str,
    label: &str,
    actual: Schema,
    expected: Schema,
) -> Result<(), TestSupportError> {
    assert_schema_matches_with_dialect(case_name, label, actual, expected, Dialect::Postgres)
}

pub fn assert_schema_matches_with_dialect(
    case_name: &str,
    label: &str,
    mut actual: Schema,
    mut expected: Schema,
    dialect: Dialect,
) -> Result<(), TestSupportError> {
    canonicalize_schema(&mut actual, dialect);
    canonicalize_schema(&mut expected, dialect);
    normalize_schema_type_comparison_keys(&mut actual, dialect);
    normalize_schema_type_comparison_keys(&mut expected, dialect);
    if actual == expected {
        return Ok(());
    }

    let actual_yaml = serde_yaml::to_string(&actual).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize actual {label}: {error}"
        ))
    })?;
    let expected_yaml = serde_yaml::to_string(&expected).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize expected {label}: {error}"
        ))
    })?;

    Err(TestSupportError::message(format!(
        "{case_name}: {label} mismatch\nexpected:\n{expected_yaml}\nactual:\n{actual_yaml}",
    )))
}

/// Rewrites column type text to test-only dialect comparison keys.
///
/// SQLite catalog inspection preserves the database's declared spelling, while
/// desired input preserves the author's spelling. The online harness compares
/// their documented semantic affinity without changing either production value.
fn normalize_schema_type_comparison_keys(schema: &mut Schema, dialect: Dialect) {
    for table in schema.tables.values_mut() {
        for column in &mut table.columns {
            column.col_type = dialect.type_comparison_key(&column.col_type);
        }
    }
}

pub fn assert_sql_matches(
    case_name: &str,
    actual: &[String],
    expected: &str,
) -> Result<(), TestSupportError> {
    let actual_normalized = normalize_text(&actual.join("\n"));
    let expected_normalized = normalize_text(expected);
    if actual_normalized == expected_normalized {
        return Ok(());
    }

    Err(TestSupportError::message(format!(
        "{case_name}: SQL mismatch\nexpected:\n{}\nactual:\n{}",
        expected_normalized, actual_normalized,
    )))
}

pub fn assert_ops_match(
    case_name: &str,
    label: &str,
    actual: &[Operation],
    expected: &[Operation],
) -> Result<(), TestSupportError> {
    if actual == expected {
        return Ok(());
    }

    let actual_yaml = serde_yaml::to_string(actual).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize actual {label}: {error}"
        ))
    })?;
    let expected_yaml = serde_yaml::to_string(expected).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize expected {label}: {error}"
        ))
    })?;

    Err(TestSupportError::message(format!(
        "{case_name}: {label} mismatch\nexpected:\n{expected_yaml}\nactual:\n{actual_yaml}",
    )))
}

pub fn assert_clarifications_match(
    case_name: &str,
    label: &str,
    actual: &[Clarification],
    expected: &[Clarification],
) -> Result<(), TestSupportError> {
    if actual == expected {
        return Ok(());
    }

    let actual_yaml = serde_yaml::to_string(actual).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize actual {label}: {error}"
        ))
    })?;
    let expected_yaml = serde_yaml::to_string(expected).map_err(|error| {
        TestSupportError::message(format!(
            "{case_name}: failed to serialize expected {label}: {error}"
        ))
    })?;

    Err(TestSupportError::message(format!(
        "{case_name}: {label} mismatch\nexpected:\n{expected_yaml}\nactual:\n{actual_yaml}",
    )))
}

pub fn assert_error_contains<T>(
    case_name: &str,
    result: Result<T, impl Display>,
    expected: &str,
) -> Result<(), TestSupportError> {
    match result {
        Ok(_) => Err(TestSupportError::message(format!(
            "{case_name}: expected failure containing '{}' but action succeeded",
            normalize_text(expected),
        ))),
        Err(error) => {
            let actual = normalize_text(&error.to_string());
            let expected = normalize_text(expected);
            if actual.contains(&expected) {
                Ok(())
            } else {
                Err(TestSupportError::message(format!(
                    "{case_name}: expected failure containing '{}' but got '{}'",
                    expected, actual,
                )))
            }
        }
    }
}

macro_rules! scope_field {
    ($field:expr, $schema:expr) => {{
        let items = std::mem::take(&mut $field);
        $field = items
            .into_values()
            .filter_map(|mut item| match item.schema.as_deref() {
                None => Some(item),
                Some(current)
                    if current == $schema || ($schema == "public" && current == "public") =>
                {
                    item.schema = None;
                    Some(item)
                }
                _ => None,
            })
            .map(|item| (item.qualified_name(), item))
            .collect();
    }};
}

pub fn scope_schema_for_compare(state: &mut Schema, schema: &str) {
    scope_field!(state.tables, schema);
    let prefix = format!("{schema}.");
    for table in state.tables.values_mut() {
        for column in &mut table.columns {
            column.col_type = strip_schema_references(&column.col_type, schema);
            if let Some(default) = &mut column.default {
                *default = strip_schema_references(default, schema);
            }
        }
        for fk in &mut table.foreign_keys {
            if let Some(local) = fk.to_table.strip_prefix(&prefix) {
                fk.to_table = local.to_string();
            }
        }
        for index in &mut table.indexes {
            if let Some(raw) = index.raw_sql() {
                let raw = strip_schema_references(raw, schema);
                let mut scoped = gaman::schema::Index::from_trusted_raw(index.name.clone(), raw);
                scoped.unique = index.unique;
                scoped.predicate = index.predicate.clone();
                *index = scoped;
            }
        }
        for trigger in &mut table.triggers {
            if let Some(function_name) = &mut trigger.function_name {
                *function_name = strip_schema_references(function_name, schema);
            }
            if let Some(query) = &mut trigger.query {
                *query = strip_schema_references(query, schema);
            }
            if let Some(when) = &mut trigger.when {
                *when = strip_schema_references(when, schema);
            }
        }
    }
    scope_field!(state.views, schema);
    scope_field!(state.functions, schema);
    scope_field!(state.extensions, schema);
    scope_field!(state.enums, schema);
    for view in state.views.values_mut() {
        view.definition = normalize_text(&strip_schema_references(&view.definition, schema));
    }
    for function in state.functions.values_mut() {
        function.arguments = strip_schema_references(&function.arguments, schema);
        function.returns = strip_schema_references(&function.returns, schema);
        function.body = normalize_text(&strip_schema_references(&function.body, schema));
    }
}

pub fn canonicalize_schema(schema: &mut Schema, dialect: Dialect) {
    schema.normalize();
    for table in schema.tables.values_mut() {
        for column in &mut table.columns {
            column.col_type = dialect.canonical_type(&column.col_type);
            if let Some(default) = &mut column.default {
                *default = canonicalize_default_for_compare(default, dialect);
            }
        }
    }
}

fn strip_schema_references(value: &str, schema: &str) -> String {
    value
        .replace(&format!("\"{schema}\"."), "")
        .replace(&format!("{schema}."), "")
}

fn canonicalize_default_for_compare(default: &str, dialect: Dialect) -> String {
    match dialect {
        Dialect::Postgres => strip_pg_implicit_text_cast(default).to_string(),
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => default.to_string(),
        Dialect::Mysql => default.to_string(),
    }
}

fn strip_pg_implicit_text_cast(default: &str) -> &str {
    default
        .strip_suffix("::text")
        .filter(|value| value.starts_with('\''))
        .unwrap_or(default)
}

fn normalize_text(text: &str) -> String {
    text.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

fn to_migrations(migrations: &[InlineMigration]) -> Vec<Migration> {
    migrations
        .iter()
        .map(InlineMigration::to_migration)
        .collect()
}

#[cfg(feature = "postgres")]
pub struct PgHarness {
    conn: sqlx::PgConnection,
    schema: String,
    url: String,
}

#[cfg(feature = "postgres")]
impl PgHarness {
    pub async fn new() -> Result<Self, TestSupportError> {
        let url = test_database_url()?;
        let opts = url
            .parse::<PgConnectOptions>()
            .map_err(|e| {
                TestSupportError::message(format!("failed to parse test database URL: {e}"))
            })?
            .ssl_mode(PgSslMode::Disable);
        let conn = opts.connect().await.map_err(|e| {
            TestSupportError::message(format!("failed to connect to test database: {e}"))
        })?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let schema = format!("gaman_test_{n}");
        Ok(Self { conn, schema, url })
    }

    pub async fn reset(&mut self) -> Result<(), TestSupportError> {
        let schema = self.schema.clone();
        let sql = format!(
            "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE; CREATE SCHEMA \"{schema}\"; SET search_path TO \"{schema}\";"
        );
        sqlx::raw_sql(&sql)
            .execute(&mut self.conn)
            .await
            .map_err(|e| {
                TestSupportError::message(format!("failed to reset schema '{schema}': {e}"))
            })?;
        Ok(())
    }

    pub async fn cleanup(&mut self) -> Result<(), TestSupportError> {
        let schema = self.schema.clone();
        let sql = format!("DROP SCHEMA IF EXISTS \"{schema}\" CASCADE");
        sqlx::raw_sql(&sql)
            .execute(&mut self.conn)
            .await
            .map_err(|e| {
                TestSupportError::message(format!("failed to cleanup schema '{schema}': {e}"))
            })?;
        Ok(())
    }

    pub fn schema_name(&self) -> &str {
        &self.schema
    }

    pub async fn batch_execute(&mut self, sql: &str) -> Result<(), TestSupportError> {
        sqlx::raw_sql(sql)
            .execute(&mut self.conn)
            .await
            .map_err(|e| TestSupportError::message(format!("execute failed: {e}\n  SQL: {sql}")))?;
        Ok(())
    }

    pub async fn inspect_schema(&mut self) -> Result<Schema, TestSupportError> {
        let opts = self
            .url
            .parse::<PgConnectOptions>()
            .map_err(|e| {
                TestSupportError::message(format!("failed to parse URL for inspect: {e}"))
            })?
            .ssl_mode(PgSslMode::Disable);
        let conn = opts.connect().await.map_err(|e| {
            TestSupportError::message(format!("failed to connect for inspect: {e}"))
        })?;
        let mut executor = PostgresExecutor::new(conn);
        executor
            .inspect_db(&[self.schema.as_str()])
            .await
            .map_err(|e| TestSupportError::message(format!("inspect_db failed: {e}")))
    }

    pub async fn verify(
        &mut self,
        migrator: &Migrator,
    ) -> Result<Vec<Operation>, TestSupportError> {
        migrator
            .verify(self.schema.as_str())
            .await
            .map_err(|e| TestSupportError::message(format!("verify failed: {e}")))
    }

    pub async fn fetch_strings(&mut self, sql: &str) -> Result<Vec<String>, TestSupportError> {
        let rows = sqlx::query_scalar::<_, String>(sql)
            .fetch_all(&mut self.conn)
            .await
            .map_err(|e| TestSupportError::message(format!("query failed: {e}\n  SQL: {sql}")))?;
        Ok(rows)
    }

    pub async fn migration_records(&mut self) -> Result<Vec<String>, TestSupportError> {
        self.fetch_strings(&format!(
            "SELECT id FROM {TRACKING_TABLE} ORDER BY applied_at, id"
        ))
        .await
    }

    pub async fn assert_lock_released(&mut self) -> Result<(), TestSupportError> {
        let acquired =
            sqlx::query_scalar::<_, bool>("SELECT pg_try_advisory_lock(7242068691819328000)")
                .fetch_one(&mut self.conn)
                .await
                .map_err(|e| TestSupportError::message(format!("lock probe failed: {e}")))?;
        if acquired {
            sqlx::query("SELECT pg_advisory_unlock(7242068691819328000)")
                .execute(&mut self.conn)
                .await
                .map_err(|e| TestSupportError::message(format!("lock cleanup failed: {e}")))?;
            Ok(())
        } else {
            Err(TestSupportError::message(
                "migration advisory lock was not released",
            ))
        }
    }
}

#[cfg(feature = "postgres")]
impl Executor for PgHarness {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query(sql)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Execute(format!("{e}\n  SQL: {sql}")))
        })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async move {
            let rows: Vec<sqlx::postgres::PgRow> = sqlx::query(sql)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
            rows.iter()
                .map(|row| {
                    row.try_get::<String, _>(0)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))
                })
                .collect()
        })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("BEGIN")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("COMMIT")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("ROLLBACK")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }
}

#[cfg(feature = "postgres")]
fn test_database_url() -> Result<String, TestSupportError> {
    std::env::var(POSTGRES_DATABASE_URL_ENV).map_err(|_| {
        TestSupportError::message(format!(
            "{POSTGRES_DATABASE_URL_ENV} must be set to run PostgreSQL integration tests",
        ))
    })
}

/// Returns PostgreSQL extensions unavailable in the configured test server.
#[cfg(feature = "postgres")]
pub async fn missing_postgres_extensions(
    extensions: &[String],
) -> Result<Vec<String>, TestSupportError> {
    if extensions.is_empty() {
        return Ok(Vec::new());
    }

    let url = test_database_url()?;
    let options = url
        .parse::<PgConnectOptions>()
        .map_err(|error| {
            TestSupportError::message(format!("failed to parse test database URL: {error}"))
        })?
        .ssl_mode(PgSslMode::Disable);
    let mut connection = options.connect().await.map_err(|error| {
        TestSupportError::message(format!("failed to connect to test database: {error}"))
    })?;
    let available = sqlx::query_scalar::<_, String>(
        "SELECT name FROM pg_available_extensions WHERE name = ANY($1)",
    )
    .bind(extensions)
    .fetch_all(&mut connection)
    .await
    .map_err(|error| TestSupportError::message(format!("failed to query extensions: {error}")))?;

    Ok(extensions
        .iter()
        .filter(|extension| !available.iter().any(|name| name == *extension))
        .cloned()
        .collect())
}

#[cfg(feature = "sqlite")]
pub struct SqliteHarness {
    _dir: Option<tempfile::TempDir>,
    url: String,
}

#[cfg(feature = "sqlite")]
impl SqliteHarness {
    pub async fn new() -> Result<Self, TestSupportError> {
        let (dir, url) = match std::env::var(SQLITE_DATABASE_URL_ENV) {
            Ok(url) => (None, url),
            Err(_) => {
                let dir = tempfile::tempdir().map_err(|e| {
                    TestSupportError::message(format!("failed to create temp dir: {e}"))
                })?;
                let db_path = dir.path().join("gaman.sqlite3");
                (Some(dir), format!("sqlite://{}", db_path.display()))
            }
        };
        let harness = Self { _dir: dir, url };
        harness.with_connection(|_| async { Ok(()) }).await?;
        Ok(harness)
    }

    pub async fn batch_execute(&self, sql: &str) -> Result<(), TestSupportError> {
        self.with_connection(|mut conn| async move {
            sqlx::raw_sql(sql).execute(&mut conn).await.map_err(|e| {
                TestSupportError::message(format!("execute failed: {e}\n  SQL: {sql}"))
            })?;
            Ok(())
        })
        .await
    }

    pub async fn inspect_schema(&self) -> Result<Schema, TestSupportError> {
        self.with_connection(|conn| async move {
            let mut executor = gaman::core::SqliteExecutor::new(conn);
            executor
                .inspect_db(&[])
                .await
                .map_err(|e| TestSupportError::message(format!("inspect_db failed: {e}")))
        })
        .await
    }

    pub async fn verify(&self, migrator: &Migrator) -> Result<Vec<Operation>, TestSupportError> {
        migrator
            .verify("")
            .await
            .map_err(|e| TestSupportError::message(format!("verify failed: {e}")))
    }

    pub async fn fetch_strings(&self, sql: &str) -> Result<Vec<String>, TestSupportError> {
        self.with_connection(|mut conn| async move {
            let rows = sqlx::query_scalar::<_, String>(sql)
                .fetch_all(&mut conn)
                .await
                .map_err(|e| {
                    TestSupportError::message(format!("query failed: {e}\n  SQL: {sql}"))
                })?;
            Ok(rows)
        })
        .await
    }

    pub async fn migration_records(&self) -> Result<Vec<String>, TestSupportError> {
        self.fetch_strings(&format!(
            "SELECT id FROM {TRACKING_TABLE} ORDER BY applied_at, id"
        ))
        .await
    }

    pub async fn assert_lock_released(&self) -> Result<(), TestSupportError> {
        Ok(())
    }

    async fn with_connection<F, Fut, T>(&self, f: F) -> Result<T, TestSupportError>
    where
        F: FnOnce(sqlx::SqliteConnection) -> Fut,
        Fut: std::future::Future<Output = Result<T, TestSupportError>>,
    {
        let conn = sqlite_connect_options(&self.url)
            .map_err(TestSupportError::message)?
            .connect()
            .await
            .map_err(|e| TestSupportError::message(format!("failed to connect sqlite: {e}")))?;
        f(conn).await
    }
}

#[cfg(feature = "sqlite")]
fn sqlite_connect_options(url: &str) -> Result<SqliteConnectOptions, String> {
    url.parse::<SqliteConnectOptions>()
        .map_err(|e| e.to_string())
        .map(|opts| opts.create_if_missing(true).foreign_keys(true))
}
