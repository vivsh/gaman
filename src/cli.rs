use crate::conf::Config;
use crate::engine::{EngineError, MigrationEngine, clarifications_disabled_message};
use argh::FromArgs;

/// Gaman CLI.
#[derive(FromArgs, Debug)]
pub struct GamanArgs {
    /// path to the migrations directory (default: ./migrations)
    #[argh(option, short = 'm')]
    pub migrations_dir: Option<String>,

    /// path to the schema file (default: ./schema.yaml)
    #[argh(option, short = 's')]
    pub schema_file: Option<String>,

    /// database connection string (overrides DATABASE_URL env var)
    #[argh(option, short = 'd')]
    pub database_url: Option<String>,

    #[argh(subcommand)]
    pub command: Command,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
pub enum Command {
    MakeMigrations(MakeMigrationsCmd),
    Migrate(MigrateCmd),
    ShowMigrations(ShowMigrationsCmd),
    SqlMigrate(SqlMigrateCmd),
    Config(ShowConfigCmd),
    InspectDb(InspectDbCmd),
    VerifyDb(VerifyDbCmd),
}

/// Print the resolved configuration.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "config")]
pub struct ShowConfigCmd {}

/// Write a new migration from the current schema.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "make_migration")]
pub struct MakeMigrationsCmd {
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
#[argh(subcommand, name = "show_migrations")]
pub struct ShowMigrationsCmd {}

/// Print SQL for one migration or the full plan.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "sql_migrate")]
pub struct SqlMigrateCmd {
    /// migration id to print SQL for; omit to print SQL for all migrations in order
    #[argh(positional)]
    pub id: Option<String>,

    /// print SQL for the backward (revert) direction instead of forward
    #[argh(switch)]
    pub backwards: bool,
}

/// Apply pending migrations.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "migrate")]
pub struct MigrateCmd {
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

/// Compare the live database against replayed migration state.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "verify_db")]
pub struct VerifyDbCmd {
    /// schema to verify (default: public)
    #[argh(option)]
    pub schema: Option<String>,
}

/// Introspect a live database and print schema YAML.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "inspect_db")]
pub struct InspectDbCmd {
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

#[derive(Debug, thiserror::Error)]
pub enum CommandError {
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

impl GamanArgs {
    /// Apply CLI overrides onto `config` and return the selected subcommand.
    pub(crate) fn apply_to(self, config: &mut Config) -> Result<Command, CommandError> {
        if let Some(dir) = self.migrations_dir {
            config.migrations_dir = std::path::PathBuf::from(dir);
        }
        if let Some(sf) = self.schema_file {
            config.schema_file = std::path::PathBuf::from(sf);
        }
        if let Some(url) = self.database_url {
            let dialect = Config::dialect_from_database_url(&url).map_err(|err| {
                CommandError::Config(format!("failed to parse dialect from database_url: {err}"))
            })?;
            config.database_url = url;
            config.dialect = dialect
        }
        Ok(self.command)
    }
}

pub async fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    let mut config = Config::from_env().map_err(|err| {
        CommandError::Config(format!("failed to parse dialect from DATABASE_URL: {err}"))
    })?;
    let cmd = args.apply_to(&mut config)?;
    config
        .validate()
        .map_err(|err| CommandError::Config(format!("invalid configuration: {err}")))?;
    let engine = MigrationEngine::from_cli_config(config, None);
    dispatch(engine, cmd).await
}

pub(crate) async fn dispatch(engine: MigrationEngine, cmd: Command) -> Result<(), CommandError> {
    match cmd {
        Command::Config(_) => {
            let config = engine.config();
            println!("  migrations_dir  {}", config.migrations_dir.display());
            println!("  schema_file     {}", config.schema_file.display());
            println!("  database_url    {}", config.database_url);
            Ok(())
        }
        Command::MakeMigrations(cmd) => {
            if cmd.merge {
                let name = cmd
                    .name
                    .ok_or_else(|| CommandError::Config("a name is required for --merge".into()))?;
                engine.make_merge_migration(&name).map_err(command_error)?;
                Ok(())
            } else if cmd.empty {
                let name = cmd
                    .name
                    .ok_or_else(|| CommandError::Config("a name is required for --empty".into()))?;
                engine.make_empty_migration(&name).map_err(command_error)?;
                Ok(())
            } else if cmd.check {
                engine.make_migration_check().map_err(command_error)
            } else if cmd.dry_run {
                let result = if cmd.non_interactive {
                    engine.make_migration_dry_run_non_interactive(cmd.name.as_deref())
                } else {
                    engine.make_migration_dry_run(cmd.name.as_deref())
                }
                .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            } else if cmd.non_interactive {
                let result = engine
                    .make_migration_non_interactive(cmd.name.as_deref())
                    .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            } else {
                let result = engine
                    .make_migration_named(cmd.name.as_deref())
                    .map_err(command_error)?;
                print_migration_result(result);
                Ok(())
            }
        }
        Command::Migrate(cmd) => {
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
                let has_pending = engine.check().await.map_err(command_error)?;
                if has_pending {
                    Err(CommandError::Config("pending migrations exist".into()))
                } else {
                    Ok(())
                }
            } else if cmd.fake {
                match cmd.target.as_deref() {
                    Some(target) => engine
                        .fake_migrate_to(target)
                        .await
                        .map(|_| ())
                        .map_err(command_error),
                    None => engine
                        .fake_migrate()
                        .await
                        .map(|_| ())
                        .map_err(command_error),
                }
            } else {
                match cmd.target.as_deref() {
                    Some(target) => engine
                        .migrate_to(target)
                        .await
                        .map(|_| ())
                        .map_err(command_error),
                    None => engine.migrate().await.map(|_| ()).map_err(command_error),
                }
            }
        }
        Command::ShowMigrations(_) => {
            let rows = engine.show_migrations().await.map_err(command_error)?;
            if rows.is_empty() {
                println!("No migrations found.");
            } else {
                for (id, applied) in &rows {
                    let marker = if *applied { "[X]" } else { "[ ]" };
                    println!("  {marker} {id}");
                }
            }
            Ok(())
        }
        Command::SqlMigrate(cmd) => {
            let stmts = if cmd.backwards {
                match cmd.id.as_deref() {
                    Some(id) => engine.sql_rollback(&[id]),
                    None => engine.sql_rollback(&[]),
                }
                .map_err(command_error)?
            } else {
                match cmd.id.as_deref() {
                    Some(id) => engine.sql_migrate_id(id),
                    None => engine.sql_migrate(),
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
        Command::InspectDb(cmd) => {
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
                None => engine.inspect_db(&schemas).await.map_err(command_error)?,
            };

            let yaml =
                serde_yaml::to_string(&state).map_err(|e| CommandError::Config(e.to_string()))?;

            match &cmd.output {
                Some(path) => std::fs::write(path, &yaml).map_err(CommandError::Io)?,
                None => print!("{yaml}"),
            }
            Ok(())
        }
        Command::VerifyDb(cmd) => {
            let schema = cmd.schema.as_deref().unwrap_or("public");
            let drift = engine.verify(schema).await.map_err(command_error)?;
            if drift.is_empty() {
                println!("No drift detected.");
                Ok(())
            } else {
                for op in &drift {
                    println!("  drift: {}", op.type_name());
                }
                Err(CommandError::Config(format!(
                    "{} drift operation(s) detected",
                    drift.len()
                )))
            }
        }
    }
}

fn command_error(err: EngineError) -> CommandError {
    match err {
        EngineError::NeedsInput(clarifications) => CommandError::Config(
            clarifications_disabled_message("make_migration", &clarifications),
        ),
        other => CommandError::Config(other.to_string()),
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

    /// Verifies CLI database URL overrides infer the PostgreSQL dialect.
    #[test]
    fn apply_to_infers_postgres_dialect_from_database_url() {
        let mut config = Config::default().with_dialect(Dialect::Postgres);

        let command = GamanArgs {
            migrations_dir: None,
            schema_file: None,
            database_url: Some("postgres://localhost/app".to_string()),
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
            migrations_dir: None,
            schema_file: None,
            database_url: Some("sqlite::memory:".to_string()),
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
            migrations_dir: None,
            schema_file: None,
            database_url: Some("oracle://localhost/app".to_string()),
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
        )
        .unwrap();
        let engine = test_engine(schema);

        let err = dispatch(
            engine,
            Command::MakeMigrations(MakeMigrationsCmd {
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
        )
        .unwrap();
        let engine = test_engine(schema);

        let err = dispatch(
            engine,
            Command::MakeMigrations(MakeMigrationsCmd {
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
