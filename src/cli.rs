use crate::conf::Config;
use crate::engine::{EngineError, MigrationEngine, clarifications_disabled_message};
use crate::migrator::{MigrationListing, MigratorError, RepairOptions, RepairReport};
use argh::FromArgs;
use gaman_core::parsers::ParseError;
use gaman_core::states::SchemaLoadError;
use std::error::Error;
use std::fmt;

/// Gaman CLI.
#[derive(FromArgs, Debug)]
pub struct GamanArgs {
    /// load environment variables from this file before resolving config
    #[argh(option)]
    pub env: Option<String>,

    /// path to the migrations directory (env: MIGRATIONS_DIR, default: ./migrations)
    #[argh(option, short = 'm')]
    pub migrations_dir: Option<String>,

    /// path to the schema file or directory (env: SCHEMA, default: ./schema.yaml)
    #[argh(option, short = 's')]
    pub schema: Option<String>,

    /// database connection string (env: DATABASE_URL, default: postgres:///)
    #[argh(option, short = 'd')]
    pub database_url: Option<String>,

    /// show internal error causes after the concise CLI diagnostic (env: GAMAN_DEBUG=1)
    #[argh(switch)]
    pub verbose: bool,

    #[argh(subcommand)]
    pub command: Command,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
pub enum Command {
    Make(MakeCmd),
    Apply(ApplyCmd),
    Rollback(RollbackCmd),
    Status(StatusCmd),
    Show(ShowCmd),
    Sql(SqlCmd),
    Config(ShowConfigCmd),
    Inspect(InspectCmd),
    Verify(VerifyCmd),
    Repair(RepairCmd),
}

/// Print the resolved configuration.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "config")]
pub struct ShowConfigCmd {}

/// Write a new migration from the current schema.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "make")]
pub struct MakeCmd {
    /// optional name for the migration (required when using --empty or --merge)
    #[argh(positional)]
    pub name: Option<String>,

    /// create an empty migration with no operations
    #[argh(switch)]
    pub empty: bool,

    /// detect conflicts and create a merge migration
    #[argh(switch)]
    pub merge: bool,

    /// check for schema changes without writing any files
    #[argh(switch)]
    pub check: bool,

    /// show what would be generated without writing any files
    #[argh(switch)]
    pub dry_run: bool,

    /// fail instead of prompting when clarifications are required
    #[argh(switch)]
    pub non_interactive: bool,
}

/// List migrations with applied and pending markers.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "status")]
pub struct StatusCmd {
    /// show newest migrations first
    #[argh(switch, short = 'r')]
    pub reverse: bool,

    /// show only migrations whose id or canonical content contains this pattern
    #[argh(option, short = 'q')]
    pub search: Option<String>,
}

/// Show canonical migration YAML content.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "show")]
pub struct ShowCmd {
    /// migration id to show; omit to show all migrations
    #[argh(positional)]
    pub id: Option<String>,

    /// show newest migrations first
    #[argh(switch, short = 'r')]
    pub reverse: bool,

    /// show only migrations whose id or canonical content contains this pattern
    #[argh(option, short = 'q')]
    pub search: Option<String>,
}

/// Print SQL for one migration or the full plan.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "sql")]
pub struct SqlCmd {
    /// migration id to print SQL for; omit to print SQL for all migrations in order
    #[argh(positional)]
    pub id: Option<String>,

    /// print SQL for the backward (revert) direction instead of forward
    #[argh(switch)]
    pub backwards: bool,
}

/// Apply pending migrations.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "apply")]
pub struct ApplyCmd {
    /// target migration id to migrate forward or backward to
    #[argh(option)]
    pub target: Option<String>,

    /// mark migrations as applied without running them
    #[argh(switch)]
    pub fake: bool,

    /// show the list of migrations that would be applied
    #[argh(switch)]
    pub plan: bool,

    /// check for unapplied migrations without applying them
    #[argh(switch)]
    pub check: bool,
}

/// Roll back to a target migration id.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "rollback")]
pub struct RollbackCmd {
    /// migration id to roll back to
    #[argh(positional)]
    pub target: String,

    /// mark rolled-back migrations as reverted without running SQL
    #[argh(switch)]
    pub fake: bool,
}

/// Compare the live database against replayed migration state.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "verify")]
pub struct VerifyCmd {
    /// schema to verify (default: public)
    #[argh(option)]
    pub schema: Option<String>,
}

/// Introspect a live database and print schema YAML.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "inspect")]
pub struct InspectCmd {
    /// schemas to introspect; may be repeated (default: public)
    #[argh(option)]
    pub schema: Vec<String>,

    /// restrict output to a single table
    #[argh(option)]
    pub table: Option<String>,

    /// write output to a file instead of stdout
    #[argh(option)]
    pub output: Option<String>,
}

/// Plan or apply one-off SQL that repairs verified database drift without writing migrations.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "repair")]
pub struct RepairCmd {
    /// execute the repair SQL; default is dry-run
    #[argh(switch)]
    pub apply: bool,

    /// allow repair when migrations are pending
    #[argh(switch)]
    pub allow_pending: bool,

    /// repair safe findings and leave unsupported findings reported
    #[argh(switch)]
    pub allow_partial: bool,

    /// print only repair SQL
    #[argh(switch)]
    pub sql_only: bool,
}

#[derive(Debug, Clone)]
pub struct CliDiagnostic {
    summary: String,
    details: Vec<String>,
    hints: Vec<String>,
    debug: Vec<String>,
}

impl CliDiagnostic {
    fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: Vec::new(),
            hints: Vec::new(),
            debug: Vec::new(),
        }
    }

    fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    fn hint(mut self, hint: impl Into<String>) -> Self {
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

#[derive(Debug)]
pub enum CommandError {
    Diagnostic(CliDiagnostic),
    Io(std::io::Error),
}

impl CommandError {
    fn diagnostic(summary: impl Into<String>) -> Self {
        Self::Diagnostic(CliDiagnostic::new(summary))
    }

    fn from_config_error(error: crate::conf::ConfigError) -> Self {
        let debug = source_chain(&error);
        let diagnostic = match &error {
            crate::conf::ConfigError::InvalidDatabaseUrl(_) => {
                CliDiagnostic::new(error.to_string())
                    .hint("set DATABASE_URL or pass --database-url")
            }
            crate::conf::ConfigError::EmptyDatabaseUrl => CliDiagnostic::new(error.to_string())
                .hint("set DATABASE_URL or pass --database-url"),
            crate::conf::ConfigError::SchemaPathInvalid(path) => {
                CliDiagnostic::new(format!("schema path is not a file or directory: {path}"))
                    .hint("pass --schema <file-or-dir> or set SCHEMA")
            }
            crate::conf::ConfigError::MigrationsDirParentMissing(path) => CliDiagnostic::new(
                format!("migrations directory parent does not exist: {path}"),
            )
            .hint("pass --migrations-dir <dir> or create the parent directory"),
            crate::conf::ConfigError::MigrationsDirNotDirectory(path) => {
                CliDiagnostic::new(format!("migrations path is not a directory: {path}"))
                    .hint("pass --migrations-dir <dir> or set MIGRATIONS_DIR")
            }
            crate::conf::ConfigError::MigrationsDirNotWritable(path)
            | crate::conf::ConfigError::MigrationsDirParentNotWritable(path) => {
                CliDiagnostic::new(format!("migrations path is not writable: {path}"))
            }
            crate::conf::ConfigError::DialectMismatch { .. } => CliDiagnostic::new(
                error.to_string(),
            )
            .hint("make DATABASE_URL and the configured dialect refer to the same database kind"),
        };
        Self::Diagnostic(diagnostic.debug(debug))
    }

    fn from_schema_load(error: &SchemaLoadError) -> Self {
        Self::Diagnostic(schema_load_diagnostic(error).debug(source_chain(error)))
    }

    fn from_engine(error: EngineError) -> Self {
        match error {
            EngineError::NeedsInput(clarifications) => Self::Diagnostic(
                CliDiagnostic::new("clarification needed")
                    .detail(clarifications_disabled_message("make", &clarifications))
                    .hint("run the command interactively or provide clarification decisions"),
            ),
            EngineError::SchemaLoad(error) => Self::from_schema_load(&error),
            EngineError::Config(message) => Self::Diagnostic(config_message_diagnostic(&message)),
            EngineError::Migrator(error) => Self::from_migrator(error),
            other => Self::Diagnostic(CliDiagnostic::from_error(&other)),
        }
    }

    fn from_migrator(error: MigratorError) -> Self {
        let debug = source_chain(&error);
        let diagnostic = match &error {
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

    pub fn print(&self, verbose: bool) {
        match self {
            Self::Diagnostic(diagnostic) => {
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
            Self::Io(error) => eprintln!("error: {error}"),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Diagnostic(diagnostic) => {
                f.write_str(&diagnostic.summary)?;
                for detail in &diagnostic.details {
                    write!(f, "\n  {detail}")?;
                }
                for hint in &diagnostic.hints {
                    write!(f, "\n  hint: {hint}")?;
                }
                Ok(())
            }
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl Error for CommandError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Diagnostic(_) => None,
        }
    }
}

impl From<std::io::Error> for CommandError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

impl GamanArgs {
    /// Loads explicitly requested environment variables before configuration resolution.
    pub(crate) fn load_env_file(&self) -> Result<(), CommandError> {
        if let Some(path) = &self.env {
            dotenvy::from_path(path).map_err(|err| {
                CommandError::Diagnostic(
                    CliDiagnostic::new(format!("failed to load env file: {path}"))
                        .detail(err.to_string())
                        .hint("pass an existing file to --env"),
                )
            })?;
        }
        Ok(())
    }

    /// Apply CLI overrides onto `config` and return the selected subcommand.
    pub(crate) fn apply_to(self, config: &mut Config) -> Result<Command, CommandError> {
        if let Some(dir) = self.migrations_dir {
            config.migrations_dir = std::path::PathBuf::from(dir);
        }
        if let Some(schema) = self.schema {
            config.schema_file = std::path::PathBuf::from(schema);
        }
        if let Some(url) = self.database_url {
            let dialect = Config::dialect_from_database_url(&url).map_err(|err| {
                CommandError::Diagnostic(
                    CliDiagnostic::new(format!("invalid database_url: {err}"))
                        .hint("use postgres://, postgresql://, sqlite://, mysql://, or mariadb://"),
                )
            })?;
            config.database_url = url;
            config.dialect = dialect
        }
        Ok(self.command)
    }
}

pub async fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    args.load_env_file()?;
    let mut config = Config::from_env().map_err(CommandError::from_config_error)?;
    let cmd = args.apply_to(&mut config)?;
    config.validate().map_err(CommandError::from_config_error)?;
    let engine = MigrationEngine::from_cli_config(config, None);
    dispatch(engine, cmd).await
}

pub(crate) async fn dispatch(engine: MigrationEngine, cmd: Command) -> Result<(), CommandError> {
    match cmd {
        Command::Config(_) => {
            let config = engine.config();
            println!("  migrations_dir  {}", config.migrations_dir.display());
            println!("  schema          {}", config.schema_file.display());
            println!("  database_url    {}", config.database_url);
            Ok(())
        }
        Command::Make(cmd) => {
            if cmd.merge {
                let name = cmd
                    .name
                    .ok_or_else(|| CommandError::diagnostic("a name is required for --merge"))?;
                engine.make_merge(&name).map_err(command_error)?;
                Ok(())
            } else if cmd.empty {
                let name = cmd
                    .name
                    .ok_or_else(|| CommandError::diagnostic("a name is required for --empty"))?;
                engine.make_empty(&name).map_err(command_error)?;
                Ok(())
            } else if cmd.check {
                engine.make_check().map_err(command_error)
            } else if cmd.dry_run {
                let result = if cmd.non_interactive {
                    engine.make_dry_run_non_interactive(cmd.name.as_deref())
                } else {
                    engine.make_dry_run(cmd.name.as_deref())
                }
                .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            } else if cmd.non_interactive {
                let result = engine
                    .make_non_interactive(cmd.name.as_deref())
                    .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            } else {
                let result = engine
                    .make_named(cmd.name.as_deref())
                    .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            }
        }
        Command::Apply(cmd) => {
            if cmd.plan {
                match engine.plan().await.map_err(command_error) {
                    Ok(pending) if pending.is_empty() => {
                        println!("No pending migrations.");
                        Ok(())
                    }
                    Ok(pending) => {
                        for id in &pending {
                            println!("  {id}");
                        }
                        Ok(())
                    }
                    Err(e) => Err(e),
                }
            } else if cmd.check {
                let pending = engine.plan().await.map_err(command_error)?;
                if pending.is_empty() {
                    Ok(())
                } else {
                    Err(CommandError::Diagnostic(
                        CliDiagnostic::new(format!("{} pending migration(s) exist", pending.len()))
                            .detail(pending.join(", "))
                            .hint("run `gaman status` to inspect or `gaman apply` to apply them"),
                    ))
                }
            } else if cmd.fake {
                match cmd.target.as_deref() {
                    Some(target) => engine
                        .fake_apply_to(target)
                        .await
                        .map(|_| ())
                        .map_err(command_error),
                    None => engine.fake_apply().await.map(|_| ()).map_err(command_error),
                }
            } else {
                match cmd.target.as_deref() {
                    Some(target) => engine
                        .apply_to(target)
                        .await
                        .map(|_| ())
                        .map_err(command_error),
                    None => engine.apply().await.map(|_| ()).map_err(command_error),
                }
            }
        }
        Command::Rollback(cmd) => {
            if cmd.fake {
                engine
                    .fake_rollback_to(&cmd.target)
                    .await
                    .map(|_| ())
                    .map_err(command_error)
            } else {
                engine
                    .rollback_to(&cmd.target)
                    .await
                    .map(|_| ())
                    .map_err(command_error)
            }
        }
        Command::Status(cmd) => {
            let mut rows = engine.show().await.map_err(command_error)?;
            filter_migration_listings(&mut rows, cmd.search.as_deref());
            if cmd.reverse {
                rows.reverse();
            }
            if rows.is_empty() {
                if let Some(search) = &cmd.search {
                    println!("No migrations match '{search}'.");
                } else {
                    println!("No migrations found.");
                }
            } else {
                for row in &rows {
                    print_migration_row(row);
                }
            }
            Ok(())
        }
        Command::Show(cmd) => {
            let mut rows = engine.show().await.map_err(command_error)?;
            if let Some(id) = &cmd.id {
                let row = rows.into_iter().find(|row| &row.id == id).ok_or_else(|| {
                    CommandError::Diagnostic(
                        CliDiagnostic::new(format!("unknown migration id '{id}'"))
                            .hint("run `gaman status` to list known migrations"),
                    )
                })?;
                rows = vec![row];
            }
            filter_migration_listings(&mut rows, cmd.search.as_deref());
            if rows.is_empty() {
                if let Some(search) = &cmd.search {
                    println!("No migrations match '{search}'.");
                } else {
                    println!("No migrations found.");
                }
                return Ok(());
            }
            if cmd.reverse {
                rows.reverse();
            }
            print_migration_contents(&rows);
            Ok(())
        }
        Command::Sql(cmd) => {
            let stmts = if cmd.backwards {
                match cmd.id.as_deref() {
                    Some(id) => engine.sql_rollback(&[id]),
                    None => engine.sql_rollback(&[]),
                }
                .map_err(command_error)?
            } else {
                match cmd.id.as_deref() {
                    Some(id) => engine.sql_id(id),
                    None => engine.sql(),
                }
                .map_err(command_error)?
            };
            if stmts.is_empty() {
                println!("-- No operations.");
            } else {
                for stmt in stmts {
                    println!("{}", sql_statement_for_cli(&stmt));
                }
            }
            Ok(())
        }
        Command::Inspect(cmd) => {
            let schemas: Vec<&str> = if cmd.schema.is_empty() {
                vec!["public"]
            } else {
                cmd.schema.iter().map(|s| s.as_str()).collect()
            };
            let state = match &cmd.table {
                Some(table) => engine
                    .inspect_table(&schemas, table)
                    .await
                    .map_err(command_error)?,
                None => engine.inspect(&schemas).await.map_err(command_error)?,
            };

            let yaml = serde_yaml::to_string(&state)
                .map_err(|e| CommandError::diagnostic(e.to_string()))?;

            match &cmd.output {
                Some(path) => std::fs::write(path, &yaml).map_err(CommandError::Io)?,
                None => print!("{yaml}"),
            }
            Ok(())
        }
        Command::Verify(cmd) => {
            let schema = cmd.schema.as_deref().unwrap_or("public");
            let report = engine.verify_report(schema).await.map_err(command_error)?;
            if report.findings.is_empty() && report.pending_migrations.is_empty() {
                println!("No drift detected.");
                Ok(())
            } else {
                for line in gaman_core::drift::format_report(&report) {
                    println!("{line}");
                }
                Err(drift_detected_error(
                    report.findings.len(),
                    report.pending_migrations.len(),
                ))
            }
        }
        Command::Repair(cmd) => {
            let report = engine
                .repair(RepairOptions {
                    apply: cmd.apply,
                    allow_pending: cmd.allow_pending,
                    allow_partial: cmd.allow_partial,
                    sql_only: cmd.sql_only,
                })
                .await
                .map_err(command_error)?;
            print_repair_report(&report, cmd.sql_only);
            if !report.verification.findings.is_empty()
                || !report.verification.pending_migrations.is_empty()
                || !report.skipped_findings.is_empty()
            {
                Err(repair_remaining_error(&report))
            } else {
                Ok(())
            }
        }
    }
}

fn command_error(err: EngineError) -> CommandError {
    CommandError::from_engine(err)
}

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
    let mut stripped = message;
    for prefix in [
        "migration error: ",
        "configuration error: ",
        "schema load error: ",
        "database operation failed: ",
    ] {
        if let Some(rest) = stripped.strip_prefix(prefix) {
            stripped = rest;
        }
    }
    stripped.to_string()
}

fn config_message_diagnostic(message: &str) -> CliDiagnostic {
    let mut diagnostic = CliDiagnostic::new(strip_error_prefixes(message));
    if message.starts_with("pending migrations block repair") {
        diagnostic = diagnostic.hint("run `gaman apply` first or pass --allow-pending");
    } else if message.contains("cannot be repaired automatically") {
        diagnostic = diagnostic.hint("pass --allow-partial to repair only supported drift");
    } else if message.contains("schema has changes") {
        diagnostic = diagnostic.hint("run `gaman make` to write a migration");
    } else if message.contains("rollback can only move backward") {
        diagnostic = diagnostic.hint("run `gaman status` and choose an applied migration");
    }
    diagnostic
}

fn schema_load_diagnostic(error: &SchemaLoadError) -> CliDiagnostic {
    match error {
        SchemaLoadError::Io(path, io) if io.kind() == std::io::ErrorKind::NotFound => {
            CliDiagnostic::new(format!("schema path not found: {path}"))
                .hint("pass --schema <file-or-dir> or set SCHEMA")
        }
        SchemaLoadError::Io(path, io) => {
            CliDiagnostic::new(format!("cannot read schema path: {path}")).detail(io.to_string())
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
            let mut diagnostic = CliDiagnostic::new(summary).detail(message.clone());
            if let Some(segment) = segment_ordinal {
                diagnostic = diagnostic.detail(format!("segment: {segment}"));
            }
            diagnostic
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

fn drift_detected_error(findings: usize, pending: usize) -> CommandError {
    CommandError::Diagnostic(
        CliDiagnostic::new(format!(
            "{findings} drift finding(s), {pending} pending migration(s) detected"
        ))
        .hint("review the reported properties; run `gaman repair` only for local drift recovery"),
    )
}

fn repair_remaining_error(report: &RepairReport) -> CommandError {
    let mut diagnostic = CliDiagnostic::new(format!(
        "{} drift finding(s), {} pending migration(s), {} skipped repair finding(s)",
        report.verification.findings.len(),
        report.verification.pending_migrations.len(),
        report.skipped_findings.len()
    ));
    if !report.verification.pending_migrations.is_empty() {
        diagnostic = diagnostic.hint("run `gaman apply` first or pass --allow-pending");
    }
    if !report.skipped_findings.is_empty() {
        diagnostic = diagnostic.hint("pass --allow-partial to repair only supported drift");
    }
    CommandError::Diagnostic(diagnostic)
}

fn filter_migration_listings(rows: &mut Vec<MigrationListing>, search: Option<&str>) {
    if let Some(search) = search {
        let needle = search.to_lowercase();
        rows.retain(|row| {
            row.id.to_lowercase().contains(&needle) || row.content.to_lowercase().contains(&needle)
        });
    }
}

fn print_migration_row(row: &MigrationListing) {
    let marker = if row.applied { "[X]" } else { "[ ]" };
    println!("  {marker} {}", row.id);
}

fn print_migration_contents(rows: &[MigrationListing]) {
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            println!();
        }
        let status = if row.applied { "applied" } else { "pending" };
        println!("--- {} ({status})", row.id);
        print!("{}", row.content);
        if !row.content.ends_with('\n') {
            println!();
        }
    }
}

fn print_repair_report(report: &RepairReport, sql_only: bool) {
    if sql_only {
        print_sql_statements(&report.sql);
        return;
    }
    if report.verification.findings.is_empty()
        && report.verification.pending_migrations.is_empty()
        && report.skipped_findings.is_empty()
        && report.sql.is_empty()
    {
        println!("No drift detected.");
        return;
    }
    if report.applied {
        println!("repair applied");
    } else {
        println!("repair dry-run: use --apply to execute this SQL");
    }
    for line in gaman_core::drift::format_report(&report.verification) {
        println!("{line}");
    }
    if !report.skipped_findings.is_empty() {
        println!(
            "  skipped repair finding(s): {}",
            report.skipped_findings.len()
        );
    }
    if report.sql.is_empty() {
        println!("-- No repair SQL.");
    } else {
        println!("repair sql:");
        print_sql_statements(&report.sql);
    }
}

fn print_sql_statements(stmts: &[String]) {
    if stmts.is_empty() {
        println!("-- No operations.");
    } else {
        for stmt in stmts {
            println!("{}", sql_statement_for_cli(stmt));
        }
    }
}

fn print_migration_result(result: Option<gaman_core::migrations::Migration>) {
    match result {
        Some(migration) => println!("Created: {}", migration.id),
        None => println!("No changes detected."),
    }
}

fn sql_statement_for_cli(stmt: &str) -> String {
    if stmt.trim_end().ends_with(';') {
        stmt.to_string()
    } else {
        format!("{stmt};")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Config;
    use gaman_core::dialects::Dialect;
    use gaman_core::states::Schema;

    /// Verifies CLI SQL formatting does not duplicate an existing statement terminator.
    #[test]
    fn sql_statement_for_cli_does_not_duplicate_semicolon() {
        assert_eq!(sql_statement_for_cli("SELECT 1;"), "SELECT 1;");
    }

    /// Verifies CLI SQL formatting preserves multiline SQL while adding one terminator.
    #[test]
    fn sql_statement_for_cli_preserves_multiline_statement() {
        assert_eq!(
            sql_statement_for_cli("CREATE VIEW v AS\nSELECT 1"),
            "CREATE VIEW v AS\nSELECT 1;"
        );
    }

    /// Verifies CLI diagnostics keep hints in normal display without exposing debug causes.
    #[test]
    fn command_error_display_includes_actionable_hint_without_debug_chain() {
        let error = CommandError::Diagnostic(
            CliDiagnostic::new("schema path not found: schema.yaml")
                .hint("pass --schema <file-or-dir>")
                .debug(vec!["low-level filesystem cause".to_string()]),
        );
        let message = error.to_string();

        assert!(message.contains("schema path not found"));
        assert!(message.contains("hint: pass --schema"));
        assert!(!message.contains("low-level filesystem cause"));
    }

    /// Verifies schema I/O errors are presented as schema path problems with a useful hint.
    #[test]
    fn schema_load_not_found_diagnostic_mentions_schema_path() {
        let error = SchemaLoadError::Io(
            "schema/schema.sql".to_string(),
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        let message = CommandError::from_schema_load(&error).to_string();

        assert!(message.contains("schema path not found: schema/schema.sql"));
        assert!(message.contains("pass --schema"));
    }

    /// Verifies configuration diagnostics do not mislabel every config error as DATABASE_URL.
    #[test]
    fn config_diagnostic_preserves_actual_config_field() {
        let error = crate::conf::ConfigError::SchemaPathInvalid("schema.sock".to_string());
        let message = CommandError::from_config_error(error).to_string();

        assert!(message.contains("schema path is not a file or directory"));
        assert!(!message.contains("DATABASE_URL"));
    }

    /// Verifies CLI database URL overrides infer the PostgreSQL dialect.
    #[test]
    fn apply_to_infers_postgres_dialect_from_database_url() {
        let mut config = Config::default().with_dialect(Dialect::Postgres);

        let command = GamanArgs {
            env: None,
            migrations_dir: None,
            schema: None,
            database_url: Some("postgres://localhost/app".to_string()),
            verbose: false,
            command: Command::Config(ShowConfigCmd {}),
        }
        .apply_to(&mut config)
        .unwrap();

        assert!(matches!(command, Command::Config(_)));
        assert_eq!(config.database_url, "postgres://localhost/app");
        assert_eq!(config.dialect, Dialect::Postgres);
    }

    /// Verifies CLI database URL overrides infer the SQLite dialect when enabled.
    #[cfg(feature = "sqlite")]
    #[test]
    fn apply_to_infers_sqlite_dialect_from_database_url() {
        let mut config = Config::default().with_dialect(Dialect::Postgres);

        GamanArgs {
            env: None,
            migrations_dir: None,
            schema: None,
            database_url: Some("sqlite::memory:".to_string()),
            verbose: false,
            command: Command::Config(ShowConfigCmd {}),
        }
        .apply_to(&mut config)
        .unwrap();

        assert_eq!(config.database_url, "sqlite::memory:");
        assert_eq!(config.dialect, Dialect::Sqlite);
    }

    /// Verifies CLI database URL overrides reject unsupported dialect schemes.
    #[test]
    fn apply_to_rejects_unsupported_database_url_scheme() {
        let mut config = Config::default().with_dialect(Dialect::Postgres);

        let err = GamanArgs {
            env: None,
            migrations_dir: None,
            schema: None,
            database_url: Some("oracle://localhost/app".to_string()),
            verbose: false,
            command: Command::Config(ShowConfigCmd {}),
        }
        .apply_to(&mut config)
        .unwrap_err();

        assert!(
            err.to_string()
                .contains("unsupported database URL dialect scheme")
        );
        assert_eq!(config.dialect, Dialect::Postgres);
    }

    /// Verifies non-interactive CLI migration generation fails instead of prompting.
    #[tokio::test]
    async fn make_migration_non_interactive_fails_when_clarification_is_needed() {
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: id
        type: mystery_type
"#,
            Dialect::Postgres,
        )
        .unwrap();
        let engine = test_engine(schema);

        let err = dispatch(
            engine,
            Command::Make(MakeCmd {
                name: Some("add_users".to_string()),
                empty: false,
                merge: false,
                check: false,
                dry_run: false,
                non_interactive: true,
            }),
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("--non-interactive"));
        assert!(message.contains("clarification"));
        assert!(message.contains("mystery_type"));
    }

    /// Verifies CLI check mode reports clarification requirements without prompting.
    #[tokio::test]
    async fn make_migration_check_reports_needed_clarifications_without_prompting() {
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: id
        type: mystery_type
"#,
            Dialect::Postgres,
        )
        .unwrap();
        let engine = test_engine(schema);

        let err = dispatch(
            engine,
            Command::Make(MakeCmd {
                name: None,
                empty: false,
                merge: false,
                check: true,
                dry_run: false,
                non_interactive: false,
            }),
        )
        .await
        .unwrap_err();

        let message = err.to_string();
        assert!(message.contains("--check"));
        assert!(message.contains("clarification"));
        assert!(message.contains("mystery_type"));
    }

    fn test_engine(schema: Schema) -> MigrationEngine {
        MigrationEngine::from_cli_config(Config::default(), Some(schema))
    }
}
