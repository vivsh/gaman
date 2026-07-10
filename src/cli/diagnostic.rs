use std::error::Error;
use std::fmt;

use crate::conf::ConfigError;
use crate::engine::EngineError;
use crate::migrator::MigratorError;
use gaman_core::clarifier::{Clarification, clarification_message};
use gaman_core::graphs::GraphError;
use gaman_core::parsers::ParseError;
use gaman_core::states::SchemaLoadError;

/// A concise CLI diagnostic with optional details, hints, and debug causes.
#[derive(Debug, Clone)]
pub struct CliDiagnostic {
    summary: String,
    details: Vec<String>,
    hints: Vec<String>,
    debug: Vec<String>,
}

impl CliDiagnostic {
    pub(crate) fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: Vec::new(),
            hints: Vec::new(),
            debug: Vec::new(),
        }
    }

    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    fn debug(mut self, debug: Vec<String>) -> Self {
        self.debug = debug;
        self
    }

    fn from_error(error: &(dyn Error + 'static)) -> Self {
        Self::new(strip_error_prefixes(&error.to_string())).debug(source_chain(error))
    }
}

/// Errors reported by CLI parsing, configuration, execution, and output paths.
#[derive(Debug)]
pub enum CommandError {
    Diagnostic(CliDiagnostic),
}

impl CommandError {
    pub(crate) fn diagnostic(summary: impl Into<String>) -> Self {
        Self::Diagnostic(CliDiagnostic::new(summary))
    }

    pub(crate) fn detail(self, detail: impl Into<String>) -> Self {
        match self {
            Self::Diagnostic(diagnostic) => Self::Diagnostic(diagnostic.detail(detail)),
        }
    }

    pub(crate) fn hint(self, hint: impl Into<String>) -> Self {
        match self {
            Self::Diagnostic(diagnostic) => Self::Diagnostic(diagnostic.hint(hint)),
        }
    }

    pub(crate) fn from_config_error(error: ConfigError) -> Self {
        let debug = source_chain(&error);
        let diagnostic = match &error {
            ConfigError::MissingDatabaseUrl | ConfigError::EmptyDatabaseUrl => {
                CliDiagnostic::new(error.to_string())
                    .hint("set DATABASE_URL or pass --database-url")
            }
            ConfigError::InvalidDatabaseUrl(_) => CliDiagnostic::new(error.to_string())
                .hint("set DATABASE_URL or pass --database-url"),
            ConfigError::SchemaPathInvalid(path) => {
                CliDiagnostic::new(format!("schema path is not a file or directory: {path}"))
                    .hint("pass --schema <file-or-dir> or set SCHEMA")
            }
            ConfigError::MigrationsDirParentMissing(path) => CliDiagnostic::new(format!(
                "migrations directory parent does not exist: {path}"
            ))
            .hint("pass --migrations-dir <dir> or create the parent directory"),
            ConfigError::MigrationsDirNotDirectory(path) => {
                CliDiagnostic::new(format!("migrations path is not a directory: {path}"))
                    .hint("pass --migrations-dir <dir> or set MIGRATIONS_DIR")
            }
            ConfigError::MigrationsDirNotWritable(path)
            | ConfigError::MigrationsDirParentNotWritable(path) => {
                CliDiagnostic::new(format!("migrations path is not writable: {path}"))
            }
            ConfigError::DialectMismatch { .. } => CliDiagnostic::new(error.to_string()).hint(
                "make DATABASE_URL and the configured dialect refer to the same database kind",
            ),
            ConfigError::DialectUnavailable { .. } => CliDiagnostic::new(error.to_string())
                .hint("use a database dialect implemented and enabled by this Gaman build"),
        };
        Self::Diagnostic(diagnostic.debug(debug))
    }

    fn from_schema_load(error: &SchemaLoadError) -> Self {
        Self::Diagnostic(schema_load_diagnostic(error).debug(source_chain(error)))
    }

    pub(crate) fn from_engine(error: EngineError) -> Self {
        match error {
            EngineError::NeedsInput(clarifications) => Self::Diagnostic(
                CliDiagnostic::new("clarification needed")
                    .detail(clarifications_disabled_message("make", &clarifications))
                    .hint("run the command interactively or provide clarification decisions"),
            ),
            EngineError::SchemaLoad(error) => Self::from_schema_load(&error),
            EngineError::Config(message) => Self::Diagnostic(config_message_diagnostic(&message)),
            error @ EngineError::UnknownInspectedTable(_) => Self::Diagnostic(
                CliDiagnostic::new(error.to_string())
                    .hint("run `gaman inspect` to list available tables"),
            ),
            error @ EngineError::AmbiguousInspectedTable { .. } => Self::Diagnostic(
                CliDiagnostic::new(error.to_string()).hint("use a schema-qualified table name"),
            ),
            EngineError::Migrator(error) => Self::from_migrator(error),
            other => Self::Diagnostic(CliDiagnostic::from_error(&other)),
        }
    }

    fn from_migrator(error: MigratorError) -> Self {
        let debug = source_chain(&error);
        let diagnostic = match &error {
            MigratorError::Graph(GraphError::UnknownId(_)) => CliDiagnostic::new(error.to_string())
                .hint("run `gaman status` to list known migrations"),
            MigratorError::Graph(GraphError::AmbiguousId { .. }) => {
                CliDiagnostic::new(error.to_string())
                    .hint("use a longer prefix or the full migration id")
            }
            MigratorError::Config(message) => config_message_diagnostic(message),
            MigratorError::Executor(_) => {
                CliDiagnostic::new(strip_error_prefixes(&error.to_string()))
                    .hint("check DATABASE_URL and database availability")
            }
            MigratorError::Environment(_) => {
                CliDiagnostic::new(strip_error_prefixes(&error.to_string()))
                    .hint("check DATABASE_URL and dialect configuration")
            }
            _ => CliDiagnostic::new(strip_error_prefixes(&error.to_string())),
        };
        Self::Diagnostic(diagnostic.debug(debug))
    }

    /// Writes a concise diagnostic and optionally includes source-chain detail.
    pub fn print(&self, verbose: bool) {
        let Self::Diagnostic(diagnostic) = self;
        eprintln!("error: {}", diagnostic.summary);
        for detail in &diagnostic.details {
            eprintln!("  {detail}");
        }
        for hint in &diagnostic.hints {
            eprintln!("  hint: {hint}");
        }
        if verbose {
            for cause in &diagnostic.debug {
                eprintln!("  caused by: {cause}");
            }
        }
    }
}

fn clarifications_disabled_message(mode: &str, clarifications: &[Clarification]) -> String {
    let mut message = format!(
        "{mode} requires {} clarification(s), but prompts are disabled",
        clarifications.len()
    );
    for clarification in clarifications {
        let prompt = clarification_message(clarification);
        message.push_str(&format!(
            "\n  - {}: {}",
            clarification.id, prompt.description
        ));
    }
    message
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Diagnostic(diagnostic) = self;
        formatter.write_str(&diagnostic.summary)?;
        for detail in &diagnostic.details {
            write!(formatter, "\n  {detail}")?;
        }
        for hint in &diagnostic.hints {
            write!(formatter, "\n  hint: {hint}")?;
        }
        Ok(())
    }
}

impl Error for CommandError {}

fn source_chain(error: &(dyn Error + 'static)) -> Vec<String> {
    let mut chain = Vec::new();
    let mut source = error.source();
    while let Some(error) = source {
        chain.push(error.to_string());
        source = error.source();
    }
    chain
}

fn strip_error_prefixes(message: &str) -> String {
    [
        "migration error: ",
        "configuration error: ",
        "schema load error: ",
        "database operation failed: ",
    ]
    .iter()
    .find_map(|prefix| message.strip_prefix(prefix))
    .unwrap_or(message)
    .to_string()
}

fn config_message_diagnostic(message: &str) -> CliDiagnostic {
    let diagnostic = CliDiagnostic::new(strip_error_prefixes(message));
    if message.starts_with("pending migrations block repair") {
        diagnostic.hint("run `gaman apply` first or pass --allow-pending")
    } else if message.contains("cannot be repaired automatically") {
        diagnostic.hint("pass --allow-partial to repair only supported drift")
    } else if message.contains("schema has changes") {
        diagnostic.hint("run `gaman make` to write a migration")
    } else if message.contains("rollback can only move backward") {
        diagnostic.hint("run `gaman status` and choose an applied migration")
    } else {
        diagnostic
    }
}

fn schema_load_diagnostic(error: &SchemaLoadError) -> CliDiagnostic {
    match error {
        SchemaLoadError::Io(path, error) if error.kind() == std::io::ErrorKind::NotFound => {
            CliDiagnostic::new(format!("schema path not found: {path}"))
                .hint("pass --schema <file-or-dir> or set SCHEMA")
        }
        SchemaLoadError::Io(path, error) => {
            CliDiagnostic::new(format!("cannot read schema path: {path}")).detail(error.to_string())
        }
        SchemaLoadError::Path { path, source } => {
            schema_load_diagnostic(source).detail(format!("schema source: {path}"))
        }
        SchemaLoadError::Yaml(error) => {
            CliDiagnostic::new("invalid YAML schema").detail(error.to_string())
        }
        SchemaLoadError::Json(error) => {
            CliDiagnostic::new("invalid JSON schema").detail(error.to_string())
        }
        SchemaLoadError::Sql(error) => parse_diagnostic(error),
        SchemaLoadError::Validation(error) => {
            CliDiagnostic::new("schema validation failed").detail(error.to_string())
        }
        SchemaLoadError::Merge { table, a, b } => CliDiagnostic::new(format!(
            "duplicate table '{table}' while merging schema directory"
        ))
        .detail(format!("first source: {a}"))
        .detail(format!("second source: {b}")),
        SchemaLoadError::DuplicateTable(table) => {
            CliDiagnostic::new(format!("duplicate table '{table}' while merging schemas"))
        }
    }
}

fn parse_diagnostic(error: &ParseError) -> CliDiagnostic {
    match error {
        ParseError::SchemaValidation(error) => {
            CliDiagnostic::new("parsed schema failed validation").detail(error.to_string())
        }
        ParseError::Parse {
            dialect,
            message,
            line,
            column,
            segment_ordinal,
            ..
        } => {
            let summary = match (line, column) {
                (Some(line), Some(column)) => {
                    format!("{dialect} SQL parse error at line {line}, column {column}")
                }
                _ => format!("{dialect} SQL parse error"),
            };
            let diagnostic = CliDiagnostic::new(summary).detail(message.clone());
            if let Some(segment) = segment_ordinal {
                diagnostic.detail(format!("segment: {segment}"))
            } else {
                diagnostic
            }
        }
        ParseError::UnsupportedStatement {
            statement, reason, ..
        } => CliDiagnostic::new(format!("unsupported SQL statement: {statement}"))
            .detail(reason.clone())
            .hint("schema files only load CREATE statements for Gaman-modeled entities"),
        ParseError::Segment {
            dialect,
            line,
            column,
            reason,
        } => CliDiagnostic::new(format!(
            "{dialect} SQL segmentation error at line {line}, column {column}"
        ))
        .detail(reason.clone()),
        ParseError::UnsupportedDialect(dialect) => {
            CliDiagnostic::new(format!("unsupported SQL dialect '{dialect}'"))
        }
        ParseError::UnknownTable { table } => {
            CliDiagnostic::new(format!("CREATE INDEX references unknown table '{table}'"))
        }
        ParseError::UnknownTriggerTable { table } => {
            CliDiagnostic::new(format!("CREATE TRIGGER references unknown table '{table}'"))
        }
        ParseError::DuplicateTable(table) => {
            CliDiagnostic::new(format!("duplicate table '{table}'"))
        }
        ParseError::Io { path, source } => {
            CliDiagnostic::new(format!("cannot read SQL source: {path}")).detail(source.to_string())
        }
    }
}
