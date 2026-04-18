#![allow(dead_code)]

use std::fmt::Display;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use gaman::Config;
use gaman::Migration;
use gaman::core::{Decision, Dialect, Executor, ExecutorError, Introspectable, Migrator, PostgresExecutor, VecAdapter};
use gaman::schema::{Operation, Schema};
use postgres::{Client, NoTls};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use thiserror::Error;

static COUNTER: AtomicU32 = AtomicU32::new(0);
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
}

impl FixtureDialect {
    pub fn to_dialect(self) -> Dialect {
        match self {
            Self::Postgres => Dialect::Postgres,
        }
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

#[derive(Debug, Deserialize)]
pub struct PostgresCase {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(flatten)]
    pub spec: PostgresSpec,
}

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

pub fn postgres_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cases/postgres")
}

pub fn discover_case_dirs(root: &Path) -> Result<Vec<PathBuf>, TestSupportError> {
    if !root.exists() {
        return Ok(vec![]);
    }

    let mut dirs = Vec::new();
    for entry in fs::read_dir(root).map_err(|error| TestSupportError::io(root, error))? {
        let entry = entry.map_err(|error| TestSupportError::io(root, error))?;
        let path = entry.path();
        if !entry.file_type().map_err(|error| TestSupportError::io(&path, error))?.is_dir() {
            continue;
        }
        if path.join("case.yaml").exists() {
            dirs.push(path);
        }
    }

    dirs.sort();
    Ok(dirs)
}

pub fn case_name(dir: &Path) -> Result<String, TestSupportError> {
    dir.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_owned)
        .ok_or_else(|| TestSupportError::parse(dir, "case directory name is not valid UTF-8"))
}

pub fn case_label(case_name: &str, description: Option<&str>) -> String {
    description.unwrap_or(case_name).to_string()
}

pub fn read_case_file<T: DeserializeOwned>(dir: &Path) -> Result<T, TestSupportError> {
    let path = dir.join("case.yaml");
    let raw = fs::read_to_string(&path).map_err(|error| TestSupportError::io(&path, error))?;
    serde_yaml::from_str(&raw).map_err(|error| TestSupportError::parse(&path, error))
}

pub fn build_migrator(
    case_name: &str,
    dialect: FixtureDialect,
    migrations: &[InlineMigration],
) -> Result<Migrator, TestSupportError> {
    let config = Arc::new(Config::default());
    let source = Box::new(VecAdapter::new(to_migrations(migrations)));
    Migrator::new(config, source, dialect.to_dialect()).map_err(|error| {
        TestSupportError::message(format!("{case_name}: failed to construct migrator: {error}"))
    })
}

pub fn ordered_migrations(case_name: &str, migrator: &Migrator) -> Result<Vec<Migration>, TestSupportError> {
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

    canonicalize_schema(&mut replay);
    Ok(replay)
}

pub fn assert_schema_matches(
    case_name: &str,
    label: &str,
    mut actual: Schema,
    mut expected: Schema,
) -> Result<(), TestSupportError> {
    canonicalize_schema(&mut actual);
    canonicalize_schema(&mut expected);
    if actual == expected {
        return Ok(());
    }

    let actual_yaml = serde_yaml::to_string(&actual).map_err(|error| {
        TestSupportError::message(format!("{case_name}: failed to serialize actual {label}: {error}"))
    })?;
    let expected_yaml = serde_yaml::to_string(&expected).map_err(|error| {
        TestSupportError::message(format!("{case_name}: failed to serialize expected {label}: {error}"))
    })?;

    Err(TestSupportError::message(format!(
        "{case_name}: {label} mismatch\nexpected:\n{expected_yaml}\nactual:\n{actual_yaml}",
    )))
}

pub fn assert_sql_matches(case_name: &str, actual: &[String], expected: &str) -> Result<(), TestSupportError> {
    let actual_normalized = normalize_text(&actual.join("\n"));
    let expected_normalized = normalize_text(expected);
    if actual_normalized == expected_normalized {
        return Ok(());
    }

    Err(TestSupportError::message(format!(
        "{case_name}: SQL mismatch\nexpected:\n{}\nactual:\n{}",
        expected_normalized,
        actual_normalized,
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
        TestSupportError::message(format!("{case_name}: failed to serialize actual {label}: {error}"))
    })?;
    let expected_yaml = serde_yaml::to_string(expected).map_err(|error| {
        TestSupportError::message(format!("{case_name}: failed to serialize expected {label}: {error}"))
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
                    expected,
                    actual,
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
                Some(current) if current == $schema || ($schema == "public" && current == "public") => {
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
    scope_field!(state.views, schema);
    scope_field!(state.functions, schema);
    scope_field!(state.extensions, schema);
    scope_field!(state.enums, schema);
}

pub fn canonicalize_schema(schema: &mut Schema) {
    schema.normalize();
    for table in schema.tables.values_mut() {
        for column in &mut table.columns {
            column.col_type = Dialect::Postgres.normalize_type(&column.col_type).to_string();
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
    migrations.iter().map(InlineMigration::to_migration).collect()
}

pub struct PgHarness {
    client: Client,
    schema: String,
    url: String,
}

impl PgHarness {
    pub fn new() -> Result<Self, TestSupportError> {
        let url = test_database_url()?;
        let client = Client::connect(&url, NoTls)
            .map_err(|error| TestSupportError::message(format!("failed to connect to test database: {error}")))?;
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let schema = format!("gaman_test_{n}");
        Ok(Self { client, schema, url })
    }

    pub fn reset(&mut self) -> Result<(), TestSupportError> {
        let schema = self.schema.clone();
        self.client
            .batch_execute(&format!(
                "DROP SCHEMA IF EXISTS \"{schema}\" CASCADE; CREATE SCHEMA \"{schema}\"; SET search_path TO \"{schema}\";"
            ))
            .map_err(|error| TestSupportError::message(format!("failed to reset schema '{schema}': {error}")))
    }

    pub fn schema_name(&self) -> &str {
        &self.schema
    }

    pub fn batch_execute(&mut self, sql: &str) -> Result<(), TestSupportError> {
        self.client
            .batch_execute(sql)
            .map_err(|error| TestSupportError::message(format!("execute failed: {error}\n  SQL: {sql}")))
    }

    pub fn inspect_schema(&mut self) -> Result<Schema, TestSupportError> {
        let client = Client::connect(&self.url, NoTls)
            .map_err(|error| TestSupportError::message(format!("failed to connect for inspect: {error}")))?;
        let mut executor = PostgresExecutor::new(client);
        executor
            .inspect_db(&[self.schema.as_str()])
            .map_err(|error| TestSupportError::message(format!("inspect_db failed: {error}")))
    }

    pub fn verify(&mut self, migrator: &Migrator) -> Result<Vec<Operation>, TestSupportError> {
        let client = Client::connect(&self.url, NoTls)
            .map_err(|error| TestSupportError::message(format!("failed to connect for verify: {error}")))?;
        let mut executor = PostgresExecutor::new(client);
        executor
            .execute(&format!("SET search_path TO \"{}\"", self.schema))
            .map_err(|error| TestSupportError::message(format!("failed to set verify search_path: {error}")))?;
        migrator
            .verify(&mut executor, self.schema.as_str())
            .map_err(|error| TestSupportError::message(format!("verify failed: {error}")))
    }

    fn drop_schema(&mut self) {
        let _ = self.client.execute(
            &format!("DROP SCHEMA IF EXISTS \"{}\" CASCADE", self.schema),
            &[],
        );
    }
}

impl Drop for PgHarness {
    fn drop(&mut self) {
        self.drop_schema();
    }
}

impl Executor for PgHarness {
    fn execute(&mut self, sql: &str) -> Result<(), ExecutorError> {
        self.client
            .execute(sql, &[])
            .map(|_| ())
            .map_err(|error| ExecutorError::Execute(format!("{error}\n  SQL: {sql}")))
    }

    fn fetch_strings(&mut self, sql: &str) -> Result<Vec<String>, ExecutorError> {
        let rows = self
            .client
            .query(sql, &[])
            .map_err(|error| ExecutorError::Fetch(error.to_string()))?;
        Ok(rows.into_iter().map(|row| row.get::<_, String>(0)).collect())
    }

    fn begin(&mut self) -> Result<(), ExecutorError> {
        self.client
            .execute("BEGIN", &[])
            .map(|_| ())
            .map_err(|error| ExecutorError::Transaction(error.to_string()))
    }

    fn commit(&mut self) -> Result<(), ExecutorError> {
        self.client
            .execute("COMMIT", &[])
            .map(|_| ())
            .map_err(|error| ExecutorError::Transaction(error.to_string()))
    }

    fn rollback(&mut self) -> Result<(), ExecutorError> {
        self.client
            .execute("ROLLBACK", &[])
            .map(|_| ())
            .map_err(|error| ExecutorError::Transaction(error.to_string()))
    }
}

fn test_database_url() -> Result<String, TestSupportError> {
    std::env::var(TEST_DATABASE_URL_ENV).map_err(|_| {
        TestSupportError::message(format!(
            "{TEST_DATABASE_URL_ENV} must be set to run PostgreSQL integration tests",
        ))
    })
}
