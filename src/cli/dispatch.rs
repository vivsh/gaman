use crate::SchemaCheckReport;
use crate::cli::args::{
    ApplyCmd, Command, InspectCmd, MakeCmd, RepairCmd, ShowCmd, SqlCmd, StatusCmd, VerifyCmd,
};
use crate::cli::diagnostic::CommandError;
use crate::cli::output::{
    drift_detected_error, filter_artifacts, filter_status_listings, print_migration_contents,
    print_migration_movement, print_migration_result, print_migration_row, print_repair_report,
    print_schema_check_report, print_sql_statements,
};
use crate::conf::Config;
use crate::engine::{EngineError, MigrationEngine};
use crate::migrator::RepairOptions;
use crate::prompter::CliPromptEngine;
use crate::schema_file::{SchemaCheckPathEntry, collect_schema_check_entries};
use gaman_core::clarifier::{Decision, PromptEngine};

use super::args::GamanArgs;

/// Resolves CLI configuration and dispatches one command.
pub async fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    args.load_env_file()?;
    let mut config = Config::from_env_with_database_url(args.database_url.clone())
        .map_err(CommandError::from_config_error)?;
    let command = args.apply_to(&mut config)?;
    if command.validates_schema_sql_only() {
        config
            .validate_schema_check()
            .map_err(CommandError::from_config_error)?;
    } else if command.requires_writable_migrations() {
        config.validate().map_err(CommandError::from_config_error)?;
    } else {
        config
            .validate_read_only()
            .map_err(CommandError::from_config_error)?;
    }
    dispatch(MigrationEngine::from_directory(config), command).await
}

/// Dispatches a parsed, validated CLI command through `MigrationEngine`.
pub(crate) async fn dispatch(
    engine: MigrationEngine,
    command: Command,
) -> Result<(), CommandError> {
    match command {
        Command::Config(command) => print_config(&engine, command.show_database_url),
        Command::Make(command) => handle_make(&engine, command),
        Command::Apply(command) => handle_apply(engine, command).await,
        Command::Status(command) => handle_status(&engine, command).await,
        Command::Show(command) => handle_show(&engine, command),
        Command::Sql(command) => handle_sql(&engine, command),
        Command::CheckSchema(_) => handle_check_schema(&engine).await,
        Command::Inspect(command) => handle_inspect(engine, command).await,
        Command::Verify(command) => handle_verify(engine, command).await,
        Command::Repair(command) => handle_repair(engine, command).await,
    }
}

/// Collects native schema files, validates SQL through the engine, and formats one report.
async fn handle_check_schema(engine: &MigrationEngine) -> Result<(), CommandError> {
    let config = engine.config();
    let entries = collect_schema_check_entries(&config.schema_file)
        .map_err(EngineError::from)
        .map_err(CommandError::from_engine)?;
    let inputs = entries.iter().filter_map(|entry| match entry {
        SchemaCheckPathEntry::Sql(input) => Some(input.clone()),
        SchemaCheckPathEntry::Ignored(_) => None,
    });
    let checked = engine
        .check_sql_schema(inputs)
        .await
        .map_err(CommandError::from_engine)?;
    let report = merge_schema_check_reports(entries, checked)?;
    print_schema_check_report(&report);
    if report.has_failures() {
        return Err(CommandError::diagnostic("schema check failed"));
    }
    Ok(())
}

/// Restores native discovery ordering around checked SQL and ignored structured inputs.
fn merge_schema_check_reports(
    entries: Vec<SchemaCheckPathEntry>,
    checked: SchemaCheckReport,
) -> Result<SchemaCheckReport, CommandError> {
    let mut checked = checked.files.into_iter();
    let mut files = Vec::with_capacity(entries.len());
    for entry in entries {
        match entry {
            SchemaCheckPathEntry::Sql(_) => files.push(checked.next().ok_or_else(|| {
                CommandError::diagnostic("schema check did not return a result for an SQL file")
            })?),
            SchemaCheckPathEntry::Ignored(report) => files.push(report),
        }
    }
    if checked.next().is_some() {
        return Err(CommandError::diagnostic(
            "schema check returned unexpected SQL file results",
        ));
    }
    Ok(SchemaCheckReport { files })
}

fn print_config(engine: &MigrationEngine, show_database_url: bool) -> Result<(), CommandError> {
    let config = engine.config();
    let database_url = if show_database_url {
        config.database_url.clone()
    } else {
        config.redacted_database_url()
    };
    println!("  migrations_dir  {}", config.migrations_dir.display());
    println!("  schema          {}", config.schema_file.display());
    println!("  dialect         {}", config.dialect.as_str());
    println!("  database_url    {database_url}");
    Ok(())
}

fn handle_make(engine: &MigrationEngine, command: MakeCmd) -> Result<(), CommandError> {
    if command.merge {
        let name = command
            .name
            .ok_or_else(|| CommandError::diagnostic("a name is required for --merge"))?;
        let migration = engine
            .make_merge(&name)
            .map_err(CommandError::from_engine)?;
        println!("Created merge migration: {}", migration.id);
        return Ok(());
    }
    if command.empty {
        let name = command
            .name
            .ok_or_else(|| CommandError::diagnostic("a name is required for --empty"))?;
        let migration = engine
            .make_empty(&name)
            .map_err(CommandError::from_engine)?;
        println!("Created empty migration: {}", migration.id);
        return Ok(());
    }
    if command.check {
        return engine.make_check().map_err(CommandError::from_engine);
    }
    let result = if command.non_interactive {
        make_non_interactive(engine, command.name.as_deref(), command.dry_run)
    } else {
        make_interactive(engine, command.name.as_deref(), command.dry_run)
    }?;
    print_migration_result(result, command.dry_run)
}

fn make_non_interactive(
    engine: &MigrationEngine,
    name: Option<&str>,
    dry_run: bool,
) -> Result<Option<gaman_core::migrations::Migration>, CommandError> {
    if dry_run {
        engine
            .make_dry_run_non_interactive(name)
            .map_err(CommandError::from_engine)
    } else {
        engine
            .make_non_interactive(name)
            .map_err(CommandError::from_engine)
    }
}

fn make_interactive(
    engine: &MigrationEngine,
    name: Option<&str>,
    dry_run: bool,
) -> Result<Option<gaman_core::migrations::Migration>, CommandError> {
    let mut decisions = Vec::<Decision>::new();
    loop {
        let result = if dry_run {
            engine.make_dry_run_with_decisions(name, &decisions)
        } else {
            engine.make_with_decisions(name, &decisions)
        };
        match result {
            Err(EngineError::NeedsInput(clarifications)) => {
                let new_decisions = CliPromptEngine
                    .prompt(&clarifications)
                    .map_err(|error| CommandError::diagnostic(error.to_string()))?;
                decisions.extend(new_decisions);
            }
            result => return result.map_err(CommandError::from_engine),
        }
    }
}

async fn handle_apply(engine: MigrationEngine, command: ApplyCmd) -> Result<(), CommandError> {
    if command.plan {
        return print_pending_plan(engine).await;
    }
    if command.check {
        return check_pending(engine).await;
    }
    let target = resolve_optional_id(&engine, command.target.as_deref())?;
    let count = if command.fake {
        match target.as_deref() {
            Some(target) => engine.fake_apply_to(target).await,
            None => engine.fake_apply().await,
        }
    } else {
        match target.as_deref() {
            Some(target) => engine.apply_to(target).await,
            None => engine.apply().await,
        }
    }
    .map_err(CommandError::from_engine)?;
    print_migration_movement(count, command.fake);
    Ok(())
}

async fn print_pending_plan(engine: MigrationEngine) -> Result<(), CommandError> {
    let pending = engine.plan().await.map_err(CommandError::from_engine)?;
    if pending.is_empty() {
        println!("No pending migrations.");
    } else {
        for id in pending {
            println!("  {id}");
        }
    }
    Ok(())
}

async fn check_pending(engine: MigrationEngine) -> Result<(), CommandError> {
    let pending = engine.plan().await.map_err(CommandError::from_engine)?;
    if pending.is_empty() {
        Ok(())
    } else {
        Err(
            CommandError::diagnostic(format!("{} pending migration(s) exist", pending.len()))
                .detail(pending.join(", "))
                .hint("run `gaman status` to inspect or `gaman apply` to apply them"),
        )
    }
}

async fn handle_status(engine: &MigrationEngine, command: StatusCmd) -> Result<(), CommandError> {
    let mut rows = engine
        .status_listings()
        .await
        .map_err(CommandError::from_engine)?;
    filter_status_listings(&mut rows, command.search.as_deref());
    if command.reverse {
        rows.reverse();
    }
    if rows.is_empty() {
        print_empty_migrations(command.search.as_deref());
    } else {
        for row in &rows {
            print_migration_row(row);
        }
    }
    Ok(())
}

fn handle_show(engine: &MigrationEngine, command: ShowCmd) -> Result<(), CommandError> {
    let mut rows = engine.show().map_err(CommandError::from_engine)?;
    if let Some(input) = command.id.as_deref() {
        let id = engine
            .resolve_migration_id(input)
            .map_err(CommandError::from_engine)?;
        rows.retain(|row| row.id == id);
    }
    filter_artifacts(&mut rows, command.search.as_deref());
    if command.reverse {
        rows.reverse();
    }
    if rows.is_empty() {
        print_empty_migrations(command.search.as_deref());
    } else {
        print_migration_contents(&rows);
    }
    Ok(())
}

fn handle_sql(engine: &MigrationEngine, command: SqlCmd) -> Result<(), CommandError> {
    let id = resolve_optional_id(engine, command.id.as_deref())?;
    let statements = if command.backwards {
        match id.as_deref() {
            Some(id) => engine.sql_rollback(&[id]),
            None => engine.sql_rollback(&[]),
        }
    } else {
        match id.as_deref() {
            Some(id) => engine.sql_id(id),
            None => engine.sql(),
        }
    }
    .map_err(CommandError::from_engine)?;
    print_sql_statements(&statements);
    Ok(())
}

async fn handle_inspect(engine: MigrationEngine, command: InspectCmd) -> Result<(), CommandError> {
    let schemas: Vec<&str> = if command.schema.is_empty() {
        vec!["public"]
    } else {
        command.schema.iter().map(String::as_str).collect()
    };
    let schema = match command.table.as_deref() {
        Some(table) => engine.inspect_table(&schemas, table).await,
        None => engine.inspect(&schemas).await,
    }
    .map_err(CommandError::from_engine)?;
    let yaml = serde_yaml::to_string(&schema).map_err(|error| {
        CommandError::diagnostic("failed to serialize inspected schema").detail(error.to_string())
    })?;
    match command.output {
        Some(path) => std::fs::write(&path, yaml).map_err(|error| {
            CommandError::diagnostic(format!("cannot write inspect output: {path}"))
                .detail(error.to_string())
        }),
        None => {
            print!("{yaml}");
            Ok(())
        }
    }
}

async fn handle_verify(engine: MigrationEngine, command: VerifyCmd) -> Result<(), CommandError> {
    let schemas = selected_schemas(&command.schema);
    let report = engine
        .verify_report_schemas(&schemas)
        .await
        .map_err(CommandError::from_engine)?;
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

async fn handle_repair(engine: MigrationEngine, command: RepairCmd) -> Result<(), CommandError> {
    let schemas = selected_schemas(&command.schema);
    let report = engine
        .repair_schemas(
            &schemas,
            RepairOptions {
                apply: command.apply,
                allow_pending: command.allow_pending,
                allow_partial: command.allow_partial,
                sql_only: command.sql_only,
            },
        )
        .await
        .map_err(CommandError::from_engine)?;
    print_repair_report(&report, command.sql_only)
}

fn selected_schemas(schemas: &[String]) -> Vec<&str> {
    if schemas.is_empty() {
        vec!["public"]
    } else {
        schemas.iter().map(String::as_str).collect()
    }
}

fn resolve_optional_id(
    engine: &MigrationEngine,
    input: Option<&str>,
) -> Result<Option<String>, CommandError> {
    input
        .map(|input| {
            engine
                .resolve_migration_id(input)
                .map_err(CommandError::from_engine)
        })
        .transpose()
}

fn print_empty_migrations(search: Option<&str>) {
    if let Some(search) = search {
        println!("No migrations match '{search}'.");
    } else {
        println!("No migrations found.");
    }
}
