use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use gaman::core::Dialect;
use gaman::schema::Schema;
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Error)]
enum ParserHarnessError {
    #[error("I/O error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("{0}")]
    Message(String),
}

impl ParserHarnessError {
    fn message(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ParserDialect {
    Postgres,
    Sqlite,
}

impl ParserDialect {
    fn to_dialect(self) -> Dialect {
        match self {
            Self::Postgres => Dialect::Postgres,
            Self::Sqlite => Dialect::Sqlite,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum EntityKind {
    Table,
    Column,
    Constraint,
    ForeignKey,
    Index,
    Trigger,
    Function,
    View,
    Enum,
    Extension,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EntityExpectation {
    kind: EntityKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    table: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ParserCase {
    description: String,
    dialect: ParserDialect,
    sql: String,
    #[serde(default)]
    expect_entities: Vec<EntityExpectation>,
    #[serde(default)]
    expect_schema: Option<Schema>,
    #[serde(default)]
    expect_error: Option<String>,
}

#[derive(Debug, Default, Serialize)]
struct ParserResults {
    cases: BTreeMap<String, ParserCaseResult>,
}

#[derive(Debug, Serialize)]
struct ParserCaseResult {
    description: String,
    dialect: ParserDialect,
    status: ParserStatus,
    entities: Vec<EntityExpectation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum ParserStatus {
    Success,
    Failure,
}

struct ParserArgs {
    record: Option<PathBuf>,
    case_args: Vec<String>,
}

enum CaseStatus {
    Success,
    Failure(String),
}

fn main() {
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(1);
        }
    };
    if let Err(error) = run_parser_cases(args) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn parse_args() -> Result<ParserArgs, ParserHarnessError> {
    let mut record = None;
    let mut case_args = Vec::new();
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--record" => {
                let value = raw
                    .next()
                    .ok_or_else(|| ParserHarnessError::message("--record requires a path"))?;
                record = Some(PathBuf::from(value));
            }
            value if value.starts_with("--") => {
                return Err(ParserHarnessError::message(format!(
                    "unsupported parser harness argument '{value}'"
                )));
            }
            _ => case_args.push(arg),
        }
    }
    Ok(ParserArgs { record, case_args })
}

fn run_parser_cases(args: ParserArgs) -> Result<(), ParserHarnessError> {
    let root = parser_cases_root();
    let files = selected_cases(&root, &args.case_args)?;
    if files.is_empty() {
        return Err(ParserHarnessError::message(
            "parser: no case files selected",
        ));
    }

    let mut descriptions = BTreeSet::new();
    let mut names = BTreeSet::new();
    let mut results = ParserResults::default();
    let mut failures = Vec::new();

    for file in files {
        let name = case_name(&file)?;
        if !names.insert(name.clone()) {
            return Err(ParserHarnessError::message(format!(
                "parser case file stem '{name}' is duplicated"
            )));
        }
        let case: ParserCase = read_yaml(&file)?;
        validate_case(&name, &case)?;
        if !descriptions.insert(case.description.clone()) {
            return Err(ParserHarnessError::message(format!(
                "{name}: duplicate parser fixture description '{}'",
                case.description
            )));
        }

        let status = run_case(&name, &case);
        record_case_status(&mut results, &name, &case, &status);
        match status {
            CaseStatus::Success => println!("  ok: {} ({})", case.description, file.display()),
            CaseStatus::Failure(message) => failures.push(format!("{}: {message}", file.display())),
        }
    }

    if let Some(path) = args.record {
        write_results(&path, &results)?;
        println!("recorded parser results: {}", path.display());
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(ParserHarnessError::message(format!(
            "parser cases failed:\n\n{}",
            failures.join("\n\n")
        )))
    }
}

fn validate_case(name: &str, case: &ParserCase) -> Result<(), ParserHarnessError> {
    if case.description.trim().is_empty() {
        return Err(ParserHarnessError::message(format!(
            "{name}: parser fixture description must not be empty"
        )));
    }
    if case.sql.trim().is_empty() {
        return Err(ParserHarnessError::message(format!(
            "{name}: parser fixture sql must not be empty"
        )));
    }
    if case.expect_schema.is_some() && case.expect_error.is_some() {
        return Err(ParserHarnessError::message(format!(
            "{name}: use either expect_schema or expect_error, not both"
        )));
    }
    if case.expect_schema.is_none() && case.expect_error.is_none() {
        return Err(ParserHarnessError::message(format!(
            "{name}: parser fixture requires expect_schema or expect_error"
        )));
    }
    if case.expect_schema.is_some() && case.expect_entities.is_empty() {
        return Err(ParserHarnessError::message(format!(
            "{name}: expect_schema requires at least one expect_entities entry"
        )));
    }
    if case.expect_error.is_some() && !case.expect_entities.is_empty() {
        return Err(ParserHarnessError::message(format!(
            "{name}: expect_error fixtures must not list expect_entities"
        )));
    }
    Ok(())
}

fn run_case(name: &str, case: &ParserCase) -> CaseStatus {
    match run_case_inner(name, case) {
        Ok(()) => CaseStatus::Success,
        Err(error) => CaseStatus::Failure(error.to_string()),
    }
}

fn run_case_inner(name: &str, case: &ParserCase) -> Result<(), ParserHarnessError> {
    let result = gaman::parsers::parse_sql(&case.sql, case.dialect.to_dialect());
    if let Some(expected_error) = &case.expect_error {
        return match result {
            Ok(schema) => Err(ParserHarnessError::message(format!(
                "{name}: expected error containing '{expected_error}', but parsed schema: {}",
                yaml_string(&schema)
            ))),
            Err(error) => {
                let actual = error.to_string();
                if actual.contains(expected_error) {
                    Ok(())
                } else {
                    Err(ParserHarnessError::message(format!(
                        "{name}: expected error containing '{expected_error}', got '{actual}'"
                    )))
                }
            }
        };
    }

    let actual = result.map_err(|error| {
        ParserHarnessError::message(format!("{name}: parser failed unexpectedly: {error}"))
    })?;
    for entity in &case.expect_entities {
        assert_entity(name, &actual, entity)?;
    }
    if let Some(expected) = &case.expect_schema {
        let mut expected = expected.clone();
        expected.normalize();
        if actual != expected {
            return Err(ParserHarnessError::message(format!(
                "{name}: parsed schema mismatch\nexpected:\n{}\nactual:\n{}",
                yaml_string(&expected),
                yaml_string(&actual)
            )));
        }
    }
    Ok(())
}

fn assert_entity(
    case_name: &str,
    schema: &Schema,
    entity: &EntityExpectation,
) -> Result<(), ParserHarnessError> {
    match entity.kind {
        EntityKind::Table => {
            let name = required_name(case_name, entity)?;
            has(schema.tables.contains_key(name), case_name, entity)
        }
        EntityKind::Column => {
            let table = required_table(case_name, entity)?;
            let name = required_name(case_name, entity)?;
            let found = schema
                .tables
                .get(table)
                .is_some_and(|table| table.columns.iter().any(|column| column.name == name));
            has(found, case_name, entity)
        }
        EntityKind::Constraint => {
            let table = required_table(case_name, entity)?;
            let name = required_name(case_name, entity)?;
            let found = schema.tables.get(table).is_some_and(|table| {
                table.primary_key.as_ref().is_some_and(|pk| pk.name == name)
                    || table
                        .constraints
                        .iter()
                        .any(|constraint| constraint.name() == name)
            });
            has(found, case_name, entity)
        }
        EntityKind::ForeignKey => {
            let table = required_table(case_name, entity)?;
            let name = required_name(case_name, entity)?;
            let found = schema.tables.get(table).is_some_and(|table| {
                table
                    .foreign_keys
                    .iter()
                    .any(|foreign_key| foreign_key.name == name)
            });
            has(found, case_name, entity)
        }
        EntityKind::Index => {
            let table = required_table(case_name, entity)?;
            let name = required_name(case_name, entity)?;
            let found = schema
                .tables
                .get(table)
                .is_some_and(|table| table.indexes.iter().any(|index| index.name == name));
            has(found, case_name, entity)
        }
        EntityKind::Trigger => {
            let table = required_table(case_name, entity)?;
            let name = required_name(case_name, entity)?;
            let found = schema.tables.get(table).is_some_and(|table| {
                table
                    .triggers
                    .iter()
                    .any(|trigger| trigger.name.as_deref() == Some(name))
            });
            has(found, case_name, entity)
        }
        EntityKind::Function => {
            let name = required_name(case_name, entity)?;
            has(schema.functions.contains_key(name), case_name, entity)
        }
        EntityKind::View => {
            let name = required_name(case_name, entity)?;
            has(schema.views.contains_key(name), case_name, entity)
        }
        EntityKind::Enum => {
            let name = required_name(case_name, entity)?;
            has(schema.enums.contains_key(name), case_name, entity)
        }
        EntityKind::Extension => {
            let name = required_name(case_name, entity)?;
            has(schema.extensions.contains_key(name), case_name, entity)
        }
    }
}

fn required_name<'a>(
    case_name: &str,
    entity: &'a EntityExpectation,
) -> Result<&'a str, ParserHarnessError> {
    entity.name.as_deref().ok_or_else(|| {
        ParserHarnessError::message(format!(
            "{case_name}: entity {:?} requires a name",
            entity.kind
        ))
    })
}

fn required_table<'a>(
    case_name: &str,
    entity: &'a EntityExpectation,
) -> Result<&'a str, ParserHarnessError> {
    entity.table.as_deref().ok_or_else(|| {
        ParserHarnessError::message(format!(
            "{case_name}: entity {:?} requires a table",
            entity.kind
        ))
    })
}

fn has(found: bool, case_name: &str, entity: &EntityExpectation) -> Result<(), ParserHarnessError> {
    if found {
        Ok(())
    } else {
        Err(ParserHarnessError::message(format!(
            "{case_name}: expected entity not found: {}",
            yaml_string(entity)
        )))
    }
}

fn record_case_status(
    results: &mut ParserResults,
    name: &str,
    case: &ParserCase,
    status: &CaseStatus,
) {
    let (status, reason) = match status {
        CaseStatus::Success => (ParserStatus::Success, None),
        CaseStatus::Failure(message) => (ParserStatus::Failure, Some(message.clone())),
    };
    results.cases.insert(
        name.to_string(),
        ParserCaseResult {
            description: case.description.clone(),
            dialect: case.dialect,
            status,
            entities: case.expect_entities.clone(),
            reason,
        },
    );
}

fn parser_cases_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("cases")
        .join("parser")
}

fn selected_cases(root: &Path, args: &[String]) -> Result<Vec<PathBuf>, ParserHarnessError> {
    if args.is_empty() {
        return discover_yaml_files(root);
    }
    let all = discover_yaml_files(root)?;
    let mut selected = BTreeSet::new();
    for arg in args {
        let path = PathBuf::from(arg);
        let resolved = if path.exists() { path } else { root.join(arg) };
        if resolved.is_file() {
            ensure_yaml(&resolved)?;
            selected.insert(resolved);
            continue;
        }
        if resolved.is_dir() {
            for file in discover_yaml_files(&resolved)? {
                selected.insert(file);
            }
            continue;
        }
        if arg.contains('*') {
            for file in &all {
                let rel = file.strip_prefix(root).unwrap_or(file);
                let rel = rel.to_string_lossy();
                let full = file.to_string_lossy();
                if wildcard_match(arg, &rel) || wildcard_match(arg, &full) {
                    selected.insert(file.clone());
                }
            }
            continue;
        }
        return Err(ParserHarnessError::message(format!(
            "parser selection '{arg}' did not match a file, directory, or glob"
        )));
    }
    Ok(selected.into_iter().collect())
}

fn discover_yaml_files(root: &Path) -> Result<Vec<PathBuf>, ParserHarnessError> {
    let mut files = Vec::new();
    discover_yaml_files_inner(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn discover_yaml_files_inner(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), ParserHarnessError> {
    for entry in fs::read_dir(root).map_err(|error| ParserHarnessError::Io {
        path: root.display().to_string(),
        message: error.to_string(),
    })? {
        let path = entry
            .map_err(|error| ParserHarnessError::Io {
                path: root.display().to_string(),
                message: error.to_string(),
            })?
            .path();
        if path.is_dir() {
            discover_yaml_files_inner(&path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "yaml") {
            files.push(path);
        }
    }
    Ok(())
}

fn ensure_yaml(path: &Path) -> Result<(), ParserHarnessError> {
    if path.extension().is_some_and(|ext| ext == "yaml") {
        Ok(())
    } else {
        Err(ParserHarnessError::message(format!(
            "parser case '{}' is not a YAML file",
            path.display()
        )))
    }
}

fn wildcard_match(pattern: &str, text: &str) -> bool {
    let mut remaining = text;
    let mut parts = pattern.split('*').peekable();
    let Some(first) = parts.next() else {
        return text.is_empty();
    };
    if !pattern.starts_with('*') {
        let first = first.trim_start_matches("tests/cases/parser/");
        if !remaining.starts_with(first) {
            return false;
        }
        remaining = &remaining[first.len()..];
    }
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let Some(index) = remaining.find(part) else {
            return false;
        };
        remaining = &remaining[index + part.len()..];
    }
    pattern.ends_with('*') || remaining.is_empty()
}

fn case_name(path: &Path) -> Result<String, ParserHarnessError> {
    path.file_stem()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            ParserHarnessError::message(format!(
                "parser case path '{}' has no UTF-8 file stem",
                path.display()
            ))
        })
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, ParserHarnessError> {
    let raw = fs::read_to_string(path).map_err(|error| ParserHarnessError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_yaml::from_str(&raw).map_err(|error| ParserHarnessError::Parse {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn write_results(path: &Path, results: &ParserResults) -> Result<(), ParserHarnessError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| ParserHarnessError::Io {
            path: parent.display().to_string(),
            message: error.to_string(),
        })?;
    }
    let yaml = serde_yaml::to_string(results).map_err(|error| {
        ParserHarnessError::message(format!("failed to serialize parser results: {error}"))
    })?;
    fs::write(path, yaml).map_err(|error| ParserHarnessError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn yaml_string<T: Serialize>(value: &T) -> String {
    serde_yaml::to_string(value).unwrap_or_else(|error| format!("<failed to serialize: {error}>"))
}
