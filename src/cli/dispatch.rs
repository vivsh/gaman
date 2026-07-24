//! Native CLI resolution, clarification, and presentation over the core runner.

use crate::cli::args::{
    AdoptCmd, ApplyCmd, Command as ArgsCommand, InspectCmd, MakeCmd, RepairCmd, ShowCmd, SqlCmd,
    apply_to, load_env_file,
};
use crate::cli::diagnostic::CommandError;
use crate::conf::Config;
use crate::prompter::CliPromptEngine;
use crate::runner_factory::NativeRunnerFactory;
use crate::schema_file::{
    SchemaCheckPathEntry, collect_schema_check_entries, load_schema_path, write_adopted_schema,
};
use gaman_core::Dialect;
use gaman_core::clarifier::PromptEngine;
use gaman_core::runner::EntityFilter;
use gaman_core::schema::{Operation, Schema};
use gaman_core::{
    ApplyCommand, Command, CommandError as RunnerError, CommandResult, MakeCommand, MakeResult,
    MigrationRunner, RepairOptions, SchemaCheckFailure, SchemaCheckInput, SchemaCheckStatus,
};

use super::args::GamanArgs;

/// Resolves one parsed CLI command and presents its host-specific result.
pub async fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    load_env_file(&args)?;
    let mut config = Config::from_env_with_database_url(args.database_url.clone())
        .map_err(CommandError::from_config_error)?;
    let parsed = apply_to(args, &mut config)?;
    match parsed {
        ArgsCommand::Config(command) => print_config(&config, command.show_database_url),
        ArgsCommand::Adopt(command) => {
            config.validate().map_err(CommandError::from_config_error)?;
            handle_adopt(config, command).await
        }
        command => {
            validate_config(&config, &command)?;
            let (command, interactive, inspect_output) = resolve_command(&config, command)?;
            let mut runner = NativeRunnerFactory::from_directory(config).build();
            let result = run_with_clarifications(&mut runner, command, interactive).await?;
            present_result(result, inspect_output)
        }
    }
}

/// Applies command-specific configuration validation before filesystem or live work begins.
fn validate_config(config: &Config, command: &ArgsCommand) -> Result<(), CommandError> {
    if command.validates_schema_sql_only() {
        config
            .validate_schema_check()
            .map_err(CommandError::from_config_error)
    } else if command.requires_writable_migrations() {
        config.validate().map_err(CommandError::from_config_error)
    } else {
        config
            .validate_read_only()
            .map_err(CommandError::from_config_error)
    }
}

/// Resolves host paths and configuration into one core runner command.
fn resolve_command(
    config: &Config,
    command: ArgsCommand,
) -> Result<(Command, bool, Option<String>), CommandError> {
    match command {
        ArgsCommand::Make(command) => resolve_make(config, command),
        ArgsCommand::Apply(command) => Ok((resolve_apply(config, command)?, false, None)),
        ArgsCommand::Status(command) => Ok((
            Command::Status {
                reverse: command.reverse,
                search: command.search,
            },
            false,
            None,
        )),
        ArgsCommand::Show(command) => Ok((resolve_show(command), false, None)),
        ArgsCommand::Sql(command) => Ok((resolve_sql(command), false, None)),
        ArgsCommand::CheckSchema(_) => Ok((resolve_check_schema(config)?, false, None)),
        ArgsCommand::Inspect(command) => {
            let output = command.output.clone();
            Ok((resolve_inspect(config, command)?, false, output))
        }
        ArgsCommand::Verify(command) => Ok((
            Command::Verify {
                schemas: selected_schemas(config.dialect, command.schema),
            },
            false,
            None,
        )),
        ArgsCommand::Repair(command) => Ok((resolve_repair(config, command), false, None)),
        ArgsCommand::Config(_) | ArgsCommand::Adopt(_) => Err(CommandError::diagnostic(
            "config does not require a migration runner",
        )),
    }
}

/// Resolves authored schema input for normal, dry-run, and check migration generation.
fn resolve_make(
    config: &Config,
    command: MakeCmd,
) -> Result<(Command, bool, Option<String>), CommandError> {
    if command.empty {
        return Ok((
            Command::Make(MakeCommand::Empty {
                name: required_name(command.name, "--empty")?,
            }),
            false,
            None,
        ));
    }
    if command.merge {
        return Ok((
            Command::Make(MakeCommand::Merge {
                name: required_name(command.name, "--merge")?,
            }),
            false,
            None,
        ));
    }
    let schema = load_schema_path(&config.schema_file, config.dialect)
        .map_err(CommandError::from_schema_load)?;
    let make = if command.check {
        MakeCommand::Check {
            schema,
            decisions: Vec::new(),
        }
    } else {
        MakeCommand::Generate {
            schema,
            name: command.name,
            dry_run: command.dry_run,
            decisions: Vec::new(),
        }
    };
    Ok((Command::Make(make), !command.non_interactive, None))
}

/// Resolves one migration-application mode without opening a database connection.
fn resolve_apply(config: &Config, command: ApplyCmd) -> Result<Command, CommandError> {
    let apply = if command.plan {
        ApplyCommand::Plan
    } else if command.check {
        ApplyCommand::Check
    } else {
        ApplyCommand::Execute {
            target: command.target,
            fake: command.fake,
            fake_verified: command.fake_verified,
            schemas: selected_schemas(config.dialect, command.schema),
        }
    };
    Ok(Command::Apply(apply))
}

/// Resolves migration-content presentation options.
fn resolve_show(command: ShowCmd) -> Command {
    Command::Show {
        id: command.id,
        reverse: command.reverse,
        search: command.search,
    }
}

/// Resolves migration SQL rendering options.
fn resolve_sql(command: SqlCmd) -> Command {
    Command::Sql {
        id: command.id,
        backwards: command.backwards,
    }
}

/// Collects authored SQL files into host-neutral schema-check inputs.
fn resolve_check_schema(config: &Config) -> Result<Command, CommandError> {
    let entries = collect_schema_check_entries(&config.schema_file)
        .map_err(CommandError::from_schema_load)?;
    let inputs = entries
        .into_iter()
        .map(|entry| match entry {
            SchemaCheckPathEntry::Sql(input) => SchemaCheckInput::Sql(input),
            SchemaCheckPathEntry::Ignored { name, reason } => {
                SchemaCheckInput::Ignored { name, reason }
            }
        })
        .collect();
    Ok(Command::CheckSchema { inputs })
}

/// Resolves live inspection while retaining the requested output path for presentation.
fn resolve_inspect(config: &Config, command: InspectCmd) -> Result<Command, CommandError> {
    let filters = command
        .filter
        .iter()
        .map(|filter| EntityFilter::parse(filter).map_err(CommandError::from_runner))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Command::Inspect {
        schemas: selected_schemas(config.dialect, command.schema),
        filters,
        table: None,
    })
}

/// Adopts selected modeled live entities through the normal desired-schema and make lifecycle.
async fn handle_adopt(config: Config, command: AdoptCmd) -> Result<(), CommandError> {
    let authored = load_schema_path(&config.schema_file, config.dialect)
        .map_err(CommandError::from_schema_load)?;
    let filters = command
        .filter
        .iter()
        .map(|filter| EntityFilter::parse(filter).map_err(CommandError::from_runner))
        .collect::<Result<Vec<_>, _>>()?;
    let schemas = selected_schemas(config.dialect, command.schema.clone());
    let mut runner = NativeRunnerFactory::from_directory(config.clone()).build();
    ensure_adoption_preflight(&mut runner, authored.clone(), command.non_interactive).await?;
    let fragment = inspect_for_adoption(&mut runner, schemas.clone(), filters).await?;
    ensure_adoption_dependencies(&fragment, &authored)?;
    let desired = authored
        .clone()
        .merge(fragment.clone())
        .map_err(CommandError::from_schema_load)?;
    let preview = preview_adoption(&mut runner, desired.clone(), &command).await?;
    ensure_additive_adoption(&preview)?;
    let filename = command
        .output
        .clone()
        .unwrap_or_else(|| format!("{}_{}.yaml", preview.id, command.name));
    let written = write_adopted_schema(&config.schema_file, &fragment, &desired, Some(&filename))
        .map_err(CommandError::from_schema_load)?;
    let created = persist_adoption(&mut runner, desired, &command)
        .await
        .map_err(|error| error.detail("schema source was updated; no migration was created"))?;
    println!("Adopted schema: {}", written.display());
    present_result(
        CommandResult::Make(MakeResult::Created(created.clone())),
        None,
    )?;
    if command.apply {
        let result = run_with_clarifications(
            &mut runner,
            Command::Apply(ApplyCommand::Execute {
                target: Some(created.id),
                fake: false,
                fake_verified: true,
                schemas,
            }),
            false,
        )
        .await?;
        present_result(result, None)?;
    }
    Ok(())
}

/// Requires current authored schema to match migration replay before adoption changes it.
async fn ensure_adoption_preflight<M, T, E>(
    runner: &mut MigrationRunner<M, T, E>,
    schema: Schema,
    interactive: bool,
) -> Result<(), CommandError>
where
    M: gaman_core::MigrationStore,
    T: gaman_core::TrackingStore,
    E: gaman_core::Executor + gaman_core::SchemaInspector,
{
    run_with_clarifications(
        runner,
        Command::Make(MakeCommand::Check {
            schema,
            decisions: Vec::new(),
        }),
        interactive,
    )
    .await
    .map(|_| ())
    .map_err(|error| error.hint("run 'gaman make' for existing schema changes before adopting"))
}

/// Runs a safe authored inspection for the requested adoption roots.
async fn inspect_for_adoption<M, T, E>(
    runner: &mut MigrationRunner<M, T, E>,
    schemas: Vec<String>,
    filters: Vec<EntityFilter>,
) -> Result<Schema, CommandError>
where
    M: gaman_core::MigrationStore,
    T: gaman_core::TrackingStore,
    E: gaman_core::Executor + gaman_core::SchemaInspector,
{
    match runner
        .run_command(&Command::Inspect {
            schemas,
            filters,
            table: None,
        })
        .await
        .map_err(CommandError::from_runner)?
    {
        CommandResult::Inspect(schema) => Ok(schema),
        _ => Err(CommandError::diagnostic(
            "inspect returned an unexpected result",
        )),
    }
}

/// Plans adoption before filesystem mutation and returns the exact candidate migration.
async fn preview_adoption<M, T, E>(
    runner: &mut MigrationRunner<M, T, E>,
    schema: Schema,
    command: &AdoptCmd,
) -> Result<gaman_core::Migration, CommandError>
where
    M: gaman_core::MigrationStore,
    T: gaman_core::TrackingStore,
    E: gaman_core::Executor + gaman_core::SchemaInspector,
{
    match run_with_clarifications(
        runner,
        Command::Make(MakeCommand::Generate {
            schema,
            name: Some(command.name.clone()),
            dry_run: true,
            decisions: Vec::new(),
        }),
        !command.non_interactive,
    )
    .await?
    {
        CommandResult::Make(MakeResult::Preview(migration)) => Ok(migration),
        CommandResult::Make(MakeResult::NoChanges) => Err(CommandError::diagnostic(
            "adopt selected no entities that require a migration",
        )),
        _ => Err(CommandError::diagnostic(
            "adopt planning returned an unexpected result",
        )),
    }
}

/// Saves the already-previewed adoption lifecycle through ordinary make behavior.
async fn persist_adoption<M, T, E>(
    runner: &mut MigrationRunner<M, T, E>,
    schema: Schema,
    command: &AdoptCmd,
) -> Result<gaman_core::Migration, CommandError>
where
    M: gaman_core::MigrationStore,
    T: gaman_core::TrackingStore,
    E: gaman_core::Executor + gaman_core::SchemaInspector,
{
    match run_with_clarifications(
        runner,
        Command::Make(MakeCommand::Generate {
            schema,
            name: Some(command.name.clone()),
            dry_run: false,
            decisions: Vec::new(),
        }),
        !command.non_interactive,
    )
    .await?
    {
        CommandResult::Make(MakeResult::Created(migration)) => Ok(migration),
        _ => Err(CommandError::diagnostic("adopt did not create a migration")),
    }
}

/// Prevents adoption from creating destructive or unrelated migration operations.
fn ensure_additive_adoption(migration: &gaman_core::Migration) -> Result<(), CommandError> {
    if migration.operations.iter().all(Operation::is_create) {
        Ok(())
    } else {
        Err(CommandError::diagnostic(
            "adopt must produce only additive operations for selected entities",
        ))
    }
}

/// Requires every selected foreign-key target to be selected or already authored.
fn ensure_adoption_dependencies(selected: &Schema, authored: &Schema) -> Result<(), CommandError> {
    for table in selected.tables.values() {
        for foreign_key in &table.foreign_keys {
            if table_exists(selected, &foreign_key.to_table)
                || table_exists(authored, &foreign_key.to_table)
            {
                continue;
            }
            return Err(CommandError::diagnostic(format!(
                "adopted table '{}' references '{}' which is neither selected nor migration-owned",
                table.qualified_name(),
                foreign_key.to_table
            )));
        }
    }
    Ok(())
}

/// Matches a foreign-key target against schema keys and canonical qualified table names.
fn table_exists(schema: &Schema, target: &str) -> bool {
    schema.tables.contains_key(target)
        || schema
            .tables
            .values()
            .any(|table| table.qualified_name() == target || table.name == target)
}

/// Resolves one repair request and its selected namespaces.
fn resolve_repair(config: &Config, command: RepairCmd) -> Command {
    Command::Repair {
        schemas: selected_schemas(config.dialect, command.schema),
        options: RepairOptions {
            apply: command.apply,
            allow_pending: command.allow_pending,
            allow_partial: command.allow_partial,
            sql_only: command.sql_only,
        },
    }
}

/// Runs one resolved command and retries only after terminal clarification input.
async fn run_with_clarifications<M, T, E>(
    runner: &mut MigrationRunner<M, T, E>,
    mut command: Command,
    interactive: bool,
) -> Result<CommandResult, CommandError>
where
    M: gaman_core::MigrationStore,
    T: gaman_core::TrackingStore,
    E: gaman_core::Executor + gaman_core::SchemaInspector,
{
    loop {
        match runner.run_command(&command).await {
            Err(RunnerError::NeedsInput(clarifications)) if interactive => {
                let decisions = CliPromptEngine
                    .prompt(&clarifications)
                    .map_err(|error| CommandError::diagnostic(error.to_string()))?;
                command = command
                    .with_decisions(decisions)
                    .map_err(CommandError::from_runner)?;
            }
            Err(RunnerError::NeedsInput(clarifications)) => {
                return Err(CommandError::clarifications_disabled(
                    "make",
                    &clarifications,
                ));
            }
            Err(error) => return Err(CommandError::from_runner(error)),
            Ok(result) => return Ok(result),
        }
    }
}

/// Presents one core command result using the native CLI's stable wording and exit behavior.
fn present_result(
    result: CommandResult,
    inspect_output: Option<String>,
) -> Result<(), CommandError> {
    match result {
        CommandResult::Make(MakeResult::Created(migration)) => {
            println!("Created: {}", migration.id)
        }
        CommandResult::Make(MakeResult::Preview(migration)) => {
            let yaml = migration
                .to_yaml_string()
                .map_err(|error| CommandError::diagnostic(error.to_string()))?;
            print!("--- {}\n{}", migration.id, yaml);
        }
        CommandResult::Make(MakeResult::NoChanges) => println!("No changes detected."),
        CommandResult::Make(MakeResult::CheckPassed) => println!("No changes detected."),
        CommandResult::Movement(movement) => {
            println!(
                "{} migration(s) applied, {} reverted.",
                movement.applied, movement.reverted
            )
        }
        CommandResult::Pending(ids) if ids.is_empty() => println!("No pending migrations."),
        CommandResult::Pending(ids) => ids.iter().for_each(|id| println!("  {id}")),
        CommandResult::Status(rows) => rows
            .iter()
            .for_each(|row| println!("  {} {}", if row.applied { "[X]" } else { "[ ]" }, row.id)),
        CommandResult::Show(rows) => rows
            .iter()
            .for_each(|row| print!("--- {}\n{}", row.id, row.content)),
        CommandResult::Sql(sql) if sql.is_empty() => println!("-- No operations."),
        CommandResult::Sql(sql) => sql.iter().for_each(|statement| print_sql(statement)),
        CommandResult::SchemaCheck(results) => {
            let mut failed = 0;
            for result in results {
                match result.status {
                    SchemaCheckStatus::Ignored { reason } => {
                        println!("{} ignored ({reason})", result.name);
                    }
                    SchemaCheckStatus::Checked { passed, failures } => {
                        if failures.is_empty() {
                            println!("{} passed ({passed})", result.name);
                            continue;
                        }
                        failed += failures.len();
                        println!(
                            "{} passed ({passed}), failed ({})",
                            result.name,
                            failures.len()
                        );
                        for failure in failures {
                            print_schema_check_failure(failure);
                        }
                    }
                }
            }
            if failed > 0 {
                return Err(CommandError::diagnostic(format!(
                    "schema check failed with {failed} error(s)"
                )));
            }
        }
        CommandResult::Inspect(schema) => {
            let yaml = serde_yaml::to_string(&schema)
                .map_err(|error| CommandError::diagnostic(error.to_string()))?;
            if let Some(path) = inspect_output {
                std::fs::write(path, yaml)
                    .map_err(|error| CommandError::diagnostic(error.to_string()))?;
            } else {
                print!("{yaml}");
            }
        }
        CommandResult::Verify(report) => {
            for line in gaman_core::drift::format_report(&report) {
                println!("{line}");
            }
            if !report.findings.is_empty() || !report.pending_migrations.is_empty() {
                return Err(CommandError::diagnostic(format!(
                    "{} drift finding(s), {} pending migration(s) detected",
                    report.findings.len(),
                    report.pending_migrations.len()
                )));
            }
            println!("No drift detected.");
        }
        CommandResult::Repair(report) => {
            for statement in report.sql {
                print_sql(&statement);
            }
        }
    }
    Ok(())
}

/// Prints one SQL statement while preserving comments and existing terminators.
fn print_sql(statement: &str) {
    let statement = statement.trim_end();
    if statement.ends_with(';') || statement.starts_with("--") {
        println!("{statement}");
    } else {
        println!("{statement};");
    }
}

/// Prints one structured schema-check failure with source identity.
fn print_schema_check_failure(failure: SchemaCheckFailure) {
    match failure {
        SchemaCheckFailure::Segmentation {
            line,
            column,
            message,
        } => match (line, column) {
            (Some(line), Some(column)) => {
                println!("  segmentation (line {line}, column {column}): {message}")
            }
            _ => println!("  segmentation: {message}"),
        },
        SchemaCheckFailure::Statement {
            ordinal,
            line,
            column: _,
            message,
        } => println!("  statement {ordinal} (line {line}): {message}"),
    }
}

/// Resolves the CLI's default namespace only when the dialect has one.
fn selected_schemas(dialect: Dialect, schemas: Vec<String>) -> Vec<String> {
    if schemas.is_empty() && dialect == Dialect::Postgres {
        vec!["public".to_string()]
    } else {
        schemas
    }
}

/// Requires a migration name for one explicit naming mode.
fn required_name(name: Option<String>, mode: &str) -> Result<String, CommandError> {
    name.ok_or_else(|| CommandError::diagnostic(format!("a name is required for {mode}")))
}

/// Prints native configuration without sending it through the lifecycle runner.
fn print_config(config: &Config, show_database_url: bool) -> Result<(), CommandError> {
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
