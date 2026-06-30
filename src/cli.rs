use std::sync::Arc;

use argh::FromArgs;
use crate::conf::Config;
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::migrator::{Migrator, MigratorError};
use crate::states::Schema;
use crate::adapters::YamlAdapter;
use crate::dialects::Dialect;
use crate::executor::{BoxFuture, Invoker, SubprocessInvoker, connect_environment_executor};
use crate::prompter::CliPromptEngine;
use crate::disambiguator::{Decision, PromptEngine};

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

    /// database dialect to use when it cannot be inferred from DATABASE_URL
    #[argh(option)]
    pub dialect: Option<String>,

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
    Migrator(#[from] MigratorError),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

struct CommandEnvironment {
    config: Arc<Config>,
    dialect: Option<Dialect>,
}

impl CommandEnvironment {
    fn new(config: Arc<Config>, dialect: Option<Dialect>) -> Self {
        Self { config, dialect }
    }
}

impl Environment for CommandEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(&'a self) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, EnvironmentError>> {
        Box::pin(async move {
            let url = self.config.database_url.as_deref()
                .ok_or_else(|| EnvironmentError::Config(
                    "DATABASE_URL is not set — pass --database-url or set it in .env".into(),
                ))?;
            connect_environment_executor(self.dialect(), url, self.config.tls)
                .await
                .map_err(EnvironmentError::from)
        })
    }

    fn invoker(&self) -> Result<Option<Box<dyn Invoker>>, EnvironmentError> {
        Ok(Some(Box::new(SubprocessInvoker)))
    }

    fn dialect(&self) -> Dialect {
        self.dialect
            .or_else(|| self.config.dialect())
            .unwrap_or(Dialect::Postgres)
    }
}

impl GamanArgs {
    /// Apply CLI overrides onto `config` and return the selected subcommand.
    pub(crate) fn apply_to(self, config: &mut Config) -> (Command, Option<String>) {
        if let Some(dir) = self.migrations_dir {
            config.migrations_dir = std::path::PathBuf::from(dir);
        }
        if let Some(sf) = self.schema_file {
            config.schema_file = std::path::PathBuf::from(sf);
        }
        if let Some(url) = self.database_url {
            config.database_url = Some(url);
        }
        (self.command, self.dialect)
    }
}

pub(crate) fn parse_dialect(value: Option<String>) -> Result<Option<Dialect>, CommandError> {
    match value {
        Some(value) => Dialect::parse(&value)
            .map(Some)
            .ok_or_else(|| CommandError::Config(format!("unsupported dialect '{value}'"))),
        None => Ok(None),
    }
}

pub async fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    let mut config = Config::default();
    let (cmd, dialect) = args.apply_to(&mut config);
    let dialect = parse_dialect(dialect)?;
    let config = Arc::new(config);
    let source = Box::new(YamlAdapter { directory: config.migrations_dir.clone() });
    let environment = Box::new(CommandEnvironment::new(Arc::clone(&config), dialect));
    let migrator = Migrator::new(source, environment)?;
    dispatch(migrator, None, cmd).await
}

pub(crate) async fn dispatch(migrator: Migrator, embedded_schema: Option<Schema>, cmd: Command) -> Result<(), CommandError> {
    match cmd {
        Command::Config(_) => {
            println!("  migrations_dir  {}", migrator.config().migrations_dir.display());
            println!("  schema_file     {}", migrator.config().schema_file.display());
            println!(
                "  database_url    {}",
                migrator.config().database_url.as_deref().unwrap_or("(not set)")
            );
            Ok(())
        }
        Command::MakeMigrations(cmd) => {
            if cmd.merge {
                let name = cmd.name.ok_or_else(|| CommandError::Config("a name is required for --merge".into()))?;
                migrator.make_merge_migration(name).map(|_| ())?;
                Ok(())
            } else if cmd.empty {
                let name = cmd.name.ok_or_else(|| CommandError::Config("a name is required for --empty".into()))?;
                migrator.make_empty_migration(name).map(|_| ())?;
                Ok(())
            } else if cmd.check {
                let current = match embedded_schema {
                    Some(s) => s,
                    None => Schema::from_file(&migrator.config().schema_file)
                        .map_err(|e| CommandError::Config(e.to_string()))?,
                };
                let name = cmd.name.clone().unwrap_or_else(|| "check".into());
                match migrator.make_migrations(Some(name), current, true, &[])? {
                    Some(_) => Err(CommandError::Config("schema has changes not yet in a migration".into())),
                    None => Ok(()),
                }
            } else {
                let current = match embedded_schema {
                    Some(s) => s,
                    None => Schema::from_file(&migrator.config().schema_file)
                        .map_err(|e| CommandError::Config(e.to_string()))?,
                };
                let engine = CliPromptEngine;
                let mut decisions: Vec<Decision> = vec![];
                loop {
                    match migrator.make_migrations(cmd.name.clone(), current.clone(), cmd.dry_run, &decisions) {
                        Err(MigratorError::NeedsInput(clars)) => {
                            let new = engine.prompt(&clars).map_err(|e| CommandError::Config(e.to_string()))?;
                            decisions.extend(new);
                        }
                        Err(e) => return Err(CommandError::Migrator(e)),
                        Ok(Some(m)) => { println!("Created: {}", m.id); break Ok(()); }
                        Ok(None) => { println!("No changes detected."); break Ok(()); }
                    }
                }
            }
        }
        Command::Migrate(cmd) => {
            if cmd.plan {
                match migrator.plan().await {
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
                    Err(e) => Err(CommandError::Migrator(e)),
                }
            } else if cmd.check {
                let has_pending = migrator.check().await.map_err(CommandError::from)?;
                if has_pending {
                    Err(CommandError::Config("pending migrations exist".into()))
                } else {
                    Ok(())
                }
            } else {
                migrator.migrate(cmd.target.as_deref(), cmd.fake).await.map(|_| ()).map_err(CommandError::from)
            }
        }
        Command::ShowMigrations(_) => {
            let rows = migrator.show_migrations().await?;
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
            let order = migrator.graph.topological_order().map_err(MigratorError::Graph)?;

            let migrations: Vec<_> = if let Some(ref id) = cmd.id {
                match migrator.graph.get(id) {
                    Some(m) => vec![m.clone()],
                    None => return Err(CommandError::Config(format!("unknown migration id '{id}'"))),
                }
            } else {
                order.iter().filter_map(|id| migrator.graph.get(id).cloned()).collect()
            };

            let migrations_to_print: Vec<_> = if cmd.backwards {
                let mut result = Vec::with_capacity(migrations.len());
                for mut m in migrations.into_iter().rev() {
                    let mut inv_ops = Vec::with_capacity(m.operations.len());
                    for op in m.operations.iter().rev() {
                        match op.inverse() {
                            Some(inv) => inv_ops.push(inv),
                            None => return Err(CommandError::Config(format!(
                                "migration '{}' is not reversible: operation '{}' has no inverse",
                                m.id, op.type_name()
                            ))),
                        }
                    }
                    m.operations = inv_ops;
                    result.push(m);
                }
                result
            } else {
                migrations
            };

            let stmts = migrator.sql_migrate(&migrations_to_print)?;
            if stmts.is_empty() {
                println!("-- No operations.");
            } else {
                for stmt in stmts {
                    println!("{stmt};");
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
            let mut state = migrator.inspect_db(&schemas).await.map_err(CommandError::from)?;

            if let Some(table) = &cmd.table {
                state.tables.retain(|k, _| k == table);
            }

            let yaml = serde_yaml::to_string(&state)
                .map_err(|e| CommandError::Config(e.to_string()))?;

            match &cmd.output {
                Some(path) => std::fs::write(path, &yaml).map_err(CommandError::Io)?,
                None => print!("{yaml}"),
            }
            Ok(())
        }
        Command::VerifyDb(cmd) => {
            let schema = cmd.schema.as_deref().unwrap_or("public");
            let drift = migrator.verify(schema).await.map_err(CommandError::from)?;
            if drift.is_empty() {
                println!("No drift detected.");
                Ok(())
            } else {
                for op in &drift {
                    println!("  drift: {}", op.type_name());
                }
                Err(CommandError::Config(format!("{} drift operation(s) detected", drift.len())))
            }
        }
    }
}
