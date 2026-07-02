#![allow(dead_code)]

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
    BoxFuture, Decision, Dialect, Environment, EnvironmentError, EnvironmentExecutor, Migrator,
    VecAdapter,
};
#[cfg(feature = "postgres")]
use gaman::core::{Executor, ExecutorError, PostgresExecutor};
use gaman::schema::{Operation, Schema};
use serde::Deserialize;
use serde::de::DeserializeOwned;
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
#[cfg(feature = "postgres")]
const TEST_DATABASE_URL_ENV: &str = "TEST_DATABASE_URL";

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
    #[cfg(feature = "sqlite")]
    Sqlite,
}

impl FixtureDialect {
    pub fn to_dialect(self) -> Dialect {
        match self {
            Self::Postgres => Dialect::Postgres,
            #[cfg(feature = "sqlite")]
            Self::Sqlite => Dialect::Sqlite,
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
        let mut config = Config::default();
        config.database_url = Some(url.to_string());
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
        let config = Config {
            database_url: Some(url.to_string()),
            ..Config::default()
        };
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
            let url = url.ok_or_else(|| {
                EnvironmentError::Config("sqlite harness database URL is not configured".into())
            })?;
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
            let url = url.ok_or_else(|| {
                EnvironmentError::Config(
                    "TEST_DATABASE_URL is not configured for the harness environment".into(),
                )
            })?;
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

#[derive(Debug, Clone, Deserialize)]
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
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub dialect: FixtureDialect,
    #[serde(flatten)]
    pub spec: OfflineSpec,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OfflineSpec {
    SqlParse {
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
    SchemaToMigration {
        name: String,
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        current: Schema,
        #[serde(default)]
        decisions: Vec<Decision>,
        #[serde(default)]
        expect_no_changes: bool,
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
        migrations: Vec<InlineMigration>,
        expect_sql: Option<String>,
        expect_error: Option<String>,
    },
}

#[cfg(feature = "sqlite")]
#[derive(Debug, Deserialize)]
pub struct SqliteCase {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(flatten)]
    pub spec: SqliteSpec,
}

#[cfg(feature = "sqlite")]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SqliteSpec {
    Migrate {
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        #[serde(default)]
        setup_sql: Option<String>,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    Inspect {
        #[serde(default)]
        setup_sql: Option<String>,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    Verify {
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        #[serde(default)]
        setup_sql: Option<String>,
        #[serde(default)]
        mutate_sql: Option<String>,
        expect_verify: Option<Vec<Operation>>,
        expect_error: Option<String>,
    },
}

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize)]
pub struct PostgresCase {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(flatten)]
    pub spec: PostgresSpec,
}

#[cfg(feature = "postgres")]
#[derive(Debug, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PostgresSpec {
    Migrate {
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        #[serde(default)]
        setup_sql: Option<String>,
        #[serde(default)]
        target: Option<String>,
        #[serde(default)]
        fake: bool,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
    Verify {
        #[serde(default)]
        migrations: Vec<InlineMigration>,
        #[serde(default)]
        setup_sql: Option<String>,
        #[serde(default)]
        mutate_sql: Option<String>,
        expect_verify: Option<Vec<Operation>>,
        expect_error: Option<String>,
    },
    Inspect {
        #[serde(default)]
        setup_sql: Option<String>,
        expect_schema: Option<Schema>,
        expect_error: Option<String>,
    },
}

fn default_atomic() -> bool {
    true
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

#[cfg(feature = "postgres")]
pub fn postgres_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/postgres")
}

#[cfg(feature = "sqlite")]
pub fn sqlite_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/sqlite")
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
            } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
                files.push(path);
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
        } else if path.extension().and_then(|e| e.to_str()) == Some("yaml") {
            files.push(path);
        }
    }

    Ok(())
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
    let config = Arc::new(Config::default());
    let source = Box::new(VecAdapter::new(to_migrations(migrations)));
    let environment = Box::new(FixtureEnvironment::new(config, dialect.to_dialect()));
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
    let source = Box::new(VecAdapter::new(to_migrations(migrations)));
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
        let migration = migrator.graph.get(&id).cloned().ok_or_else(|| {
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
        let migration = migrator.graph.get(&id).ok_or_else(|| {
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
        for fk in &mut table.foreign_keys {
            if let Some(local) = fk.to_table.strip_prefix(&prefix) {
                fk.to_table = local.to_string();
            }
        }
        for trigger in &mut table.triggers {
            if let Some(function_name) = &mut trigger.function_name
                && let Some(local) = function_name.strip_prefix(&prefix)
            {
                *function_name = local.to_string();
            }
        }
    }
    scope_field!(state.views, schema);
    scope_field!(state.functions, schema);
    scope_field!(state.extensions, schema);
    scope_field!(state.enums, schema);
}

pub fn canonicalize_schema(schema: &mut Schema, dialect: Dialect) {
    schema.normalize();
    for table in schema.tables.values_mut() {
        for column in &mut table.columns {
            column.col_type = dialect.canonical_type(&column.col_type);
        }
    }
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
    std::env::var(TEST_DATABASE_URL_ENV).map_err(|_| {
        TestSupportError::message(format!(
            "{TEST_DATABASE_URL_ENV} must be set to run PostgreSQL integration tests",
        ))
    })
}

#[cfg(feature = "sqlite")]
pub struct SqliteHarness {
    _dir: tempfile::TempDir,
    url: String,
}

#[cfg(feature = "sqlite")]
impl SqliteHarness {
    pub async fn new() -> Result<Self, TestSupportError> {
        let dir = tempfile::tempdir()
            .map_err(|e| TestSupportError::message(format!("failed to create temp dir: {e}")))?;
        let db_path = dir.path().join("gaman.sqlite3");
        let url = format!("sqlite://{}", db_path.display());
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
