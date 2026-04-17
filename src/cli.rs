use std::sync::Arc;

use argh::FromArgs;
use postgres::{Client, NoTls};

use crate::conf::Config;
use crate::migrator::{Migrator, MigratorError};
use crate::states::Schema;
use crate::adapters::YamlAdapter;
use crate::dialects::Dialect;
use crate::executor::{Introspectable, PostgresExecutor, SubprocessInvoker};
use crate::prompter::CliPromptEngine;
use crate::disambiguator::{Decision, PromptEngine};

/// Gaman — PostgreSQL migration tool
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

/// Print the resolved configuration and exit
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "config")]
pub struct ShowConfigCmd {}

/// Generate a new migration from the current schema state
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

/// Show all migrations with [X] applied / [ ] pending markers
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "show_migrations")]
pub struct ShowMigrationsCmd {}

/// Print the SQL statements for one or all migrations — no database connection required
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

/// Apply pending migrations to the database
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

/// Compare the live database against the replayed migration state and report drift
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "verify_db")]
pub struct VerifyDbCmd {
    /// schema to verify (default: public)
    #[argh(option)]
    pub schema: Option<String>,
}

/// Introspect a live database and print the schema state as YAML
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
    Db(#[from] postgres::Error),
    #[error("{0}")]
    Config(String),
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

fn db_connect(config: &Config) -> Result<PostgresExecutor, CommandError> {
    let url = config.database_url.as_deref()
        .ok_or_else(|| CommandError::Config(
            "DATABASE_URL is not set — pass --database-url or set it in .env".into(),
        ))?;
    Ok(PostgresExecutor::new(Client::connect(url, NoTls)?))
}

impl GamanArgs {
    /// Apply the CLI overrides (if any) onto `config` and return the subcommand.
    pub(crate) fn apply_to(self, config: &mut Config) -> Command {
        if let Some(dir) = self.migrations_dir {
            config.migrations_dir = std::path::PathBuf::from(dir);
        }
        if let Some(sf) = self.schema_file {
            config.schema_file = std::path::PathBuf::from(sf);
        }
        if let Some(url) = self.database_url {
            config.database_url = Some(url);
        }
        self.command
    }
}

pub fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    let mut config = Config::default();
    let cmd = args.apply_to(&mut config);
    let config = Arc::new(config);
    let source = Box::new(YamlAdapter { directory: config.migrations_dir.clone() });
    let migrator = Migrator::new(config, source, Dialect::Postgres)?;
    dispatch(migrator, None, cmd)
}

pub(crate) fn dispatch(migrator: Migrator, embedded_schema: Option<Schema>, cmd: Command) -> Result<(), CommandError> {
    match cmd {
        Command::Config(_) => {
            println!("  migrations_dir  {}", migrator.config.migrations_dir.display());
            println!("  schema_file     {}", migrator.config.schema_file.display());
            println!(
                "  database_url    {}",
                migrator.config.database_url.as_deref().unwrap_or("(not set)")
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
                    None => Schema::load(&migrator.config.schema_file)
                        .map_err(|e| CommandError::Config(e.to_string()))?,
                };
                let name = cmd.name.unwrap_or_else(|| "check".into());
                match migrator.make_migrations(name, current, true, &[])? {
                    Some(_) => Err(CommandError::Config("schema has changes not yet in a migration".into())),
                    None => Ok(()),
                }
            } else {
                let name = cmd.name.ok_or_else(|| CommandError::Config("a migration name is required".into()))?;
                let current = match embedded_schema {
                    Some(s) => s,
                    None => Schema::load(&migrator.config.schema_file)
                        .map_err(|e| CommandError::Config(e.to_string()))?,
                };
                let engine = CliPromptEngine;
                let mut decisions: Vec<Decision> = vec![];
                loop {
                    match migrator.make_migrations(name.clone(), current.clone(), cmd.dry_run, &decisions) {
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
            let mut executor = db_connect(&migrator.config)?;
            let invoker = SubprocessInvoker;
            if cmd.plan {
                match migrator.plan(&mut executor) {
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
                migrator.check(&mut executor).map_err(CommandError::from).and_then(|has_pending| {
                    if has_pending {
                        Err(CommandError::Config("pending migrations exist".into()))
                    } else {
                        Ok(())
                    }
                })
            } else {
                migrator.migrate(&mut executor, Some(&invoker), cmd.target.as_deref(), cmd.fake).map_err(CommandError::from)
            }
        }
        Command::ShowMigrations(_) => {
            let mut executor = db_connect(&migrator.config)?
;            let rows = migrator.show_migrations(&mut executor)?;
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
            let mut executor = db_connect(&migrator.config)?;

            let schemas: Vec<&str> = if cmd.schema.is_empty() {
                vec!["public"]
            } else {
                cmd.schema.iter().map(|s| s.as_str()).collect()
            };
            let mut state = executor
                .inspect_db(&schemas)
                .map_err(|e| CommandError::Config(e.to_string()))?;

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
            let mut executor = db_connect(&migrator.config)?;
            let schema = cmd.schema.as_deref().unwrap_or("public");
            let drift = migrator.verify(&mut executor, schema).map_err(CommandError::from)?;
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
