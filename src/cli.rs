use std::fmt;
use std::sync::Arc;

use argh::FromArgs;
use postgres::{Client, NoTls};

use crate::adapters::YamlAdapter;
use crate::conf::Config;
use crate::dialects::Dialect;
use crate::executor::{Introspectable, PostgresExecutor, SubprocessInvoker};
use crate::migrator::{Migrator, MigratorError};
use crate::states::SchemaState;

/// Gaman — PostgreSQL migration tool
#[derive(FromArgs, Debug)]
pub struct GamanArgs {
    /// path to the migrations directory (default: ./migrations)
    #[argh(option, short = 'm')]
    pub migrations_dir: Option<String>,

    /// path to the schema file (default: ./schema.yaml)
    #[argh(option, short = 's')]
    pub schema_file: Option<String>,

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
pub struct ShowMigrationsCmd {
    /// database connection string (overrides DATABASE_URL env var)
    #[argh(option)]
    pub database_url: Option<String>,
}

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
    /// database connection string (overrides DATABASE_URL env var)
    #[argh(option)]
    pub database_url: Option<String>,

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

/// Introspect a live database and print the schema state as YAML
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "inspect_db")]
pub struct InspectDbCmd {
    /// database connection string (overrides DATABASE_URL env var)
    #[argh(option)]
    pub database_url: Option<String>,

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

#[derive(Debug)]
pub enum CommandError {
    Migrator(MigratorError),
    Db(postgres::Error),
    Config(String),
    Io(std::io::Error),
}

impl fmt::Display for CommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CommandError::Migrator(e) => write!(f, "{e}"),
            CommandError::Db(e) => write!(f, "{e}"),
            CommandError::Config(s) => write!(f, "{s}"),
            CommandError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl From<MigratorError> for CommandError {
    fn from(e: MigratorError) -> Self {
        CommandError::Migrator(e)
    }
}

impl From<postgres::Error> for CommandError {
    fn from(e: postgres::Error) -> Self {
        CommandError::Db(e)
    }
}

pub fn handle_cmd(args: GamanArgs) -> Result<(), CommandError> {
    let mut config = Config::default();
    if let Some(dir) = args.migrations_dir {
        config.migrations_dir = std::path::PathBuf::from(dir);
    }
    if let Some(schema) = args.schema_file {
        config.schema_file = std::path::PathBuf::from(schema);
    }
    if let Command::Migrate(ref cmd) = args.command {
        if let Some(url) = &cmd.database_url {
            config.database_url = Some(url.clone());
        }
    }
    if let Command::ShowMigrations(ref cmd) = args.command {
        if let Some(url) = &cmd.database_url {
            config.database_url = Some(url.clone());
        }
    }
    if let Command::InspectDb(ref cmd) = args.command {
        if let Some(url) = &cmd.database_url {
            config.database_url = Some(url.clone());
        }
    }
    let config = Arc::new(config);

    let source = Box::new(YamlAdapter { directory: config.migrations_dir.clone() });
    let migrator = Migrator::new(config, source, Dialect::Postgres)?;

    match args.command {
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
                let current = SchemaState::load(&migrator.config.schema_file)
                    .map_err(|e| CommandError::Config(e.to_string()))?;
                let name = cmd.name.unwrap_or_else(|| "check".into());
                match migrator.make_migrations(name, current, true)? {
                    Some(_) => Err(CommandError::Config("schema has changes not yet in a migration".into())),
                    None => Ok(()),
                }
            } else {
                let name = cmd.name.ok_or_else(|| CommandError::Config("a migration name is required".into()))?;
                let current = SchemaState::load(&migrator.config.schema_file)
                    .map_err(|e| CommandError::Config(e.to_string()))?;
                match migrator.make_migrations(name, current, cmd.dry_run)? {
                    Some(m) => { println!("Created: {}", m.id); Ok(()) },
                    None => { println!("No changes detected."); Ok(()) },
                }
            }
        }
        Command::Migrate(cmd) => {
            let url = migrator
                .config
                .database_url
                .as_deref()
                .ok_or_else(|| CommandError::Config(
                    "DATABASE_URL is not set — pass --database-url or set it in .env".into(),
                ))?
                .to_string();
            let client = Client::connect(&url, NoTls)?;
            let mut executor = PostgresExecutor::new(client);
            let invoker = SubprocessInvoker;
            let result = if cmd.plan {
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
            };
            result
        }
        Command::ShowMigrations(_) => {
            let url = migrator
                .config
                .database_url
                .as_deref()
                .ok_or_else(|| CommandError::Config(
                    "DATABASE_URL is not set — pass --database-url or set it in .env".into(),
                ))?
                .to_string();
            let client = Client::connect(&url, NoTls)?;
            let mut executor = PostgresExecutor::new(client);
            let rows = migrator.show_migrations(&mut executor)?;
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
            let url = migrator
                .config
                .database_url
                .as_deref()
                .ok_or_else(|| CommandError::Config(
                    "DATABASE_URL is not set — pass --database-url or set it in .env".into(),
                ))?
                .to_string();
            let client = Client::connect(&url, NoTls)?;
            let mut executor = PostgresExecutor::new(client);

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
    }
}
