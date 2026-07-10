use crate::cli::diagnostic::CommandError;
use crate::conf::Config;
use argh::FromArgs;

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

    /// database connection string (env: DATABASE_URL, required)
    #[argh(option, short = 'd')]
    pub database_url: Option<String>,

    /// show internal error causes after the concise CLI diagnostic (env: GAMAN_DEBUG=1)
    #[argh(switch)]
    pub verbose: bool,

    /// print the Gaman version
    #[argh(switch, short = 'V')]
    pub version: bool,

    #[argh(subcommand)]
    pub command: Command,
}

#[derive(FromArgs, Debug)]
#[argh(subcommand)]
pub enum Command {
    Make(MakeCmd),
    Apply(ApplyCmd),
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
pub struct ShowConfigCmd {
    /// print the complete database URL, including credentials
    #[argh(switch)]
    pub show_database_url: bool,
}

/// Write a new migration from the current schema.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "make")]
pub struct MakeCmd {
    /// optional name for a normal or dry-run migration
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

/// Show canonical migration YAML content without a database connection.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "show")]
pub struct ShowCmd {
    /// full migration id or unique id prefix to show; omit to show all migrations
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
    /// full migration id or unique id prefix; omit to render all migrations
    #[argh(positional)]
    pub id: Option<String>,
    /// print the backward direction instead of forward SQL
    #[argh(switch)]
    pub backwards: bool,
}

/// Apply pending migrations or converge on a target migration.
#[derive(FromArgs, Debug)]
#[argh(subcommand, name = "apply")]
pub struct ApplyCmd {
    /// optional full migration id or unique prefix to converge on
    #[argh(positional)]
    pub target: Option<String>,
    /// update migration tracking without running migration SQL
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
#[argh(subcommand, name = "verify")]
pub struct VerifyCmd {
    /// schema to verify; may be repeated (default: public)
    #[argh(option)]
    pub schema: Vec<String>,
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
    /// schema to repair; may be repeated (default: public)
    #[argh(option)]
    pub schema: Vec<String>,
}

impl GamanArgs {
    /// Loads explicitly requested environment variables before configuration resolution.
    pub(crate) fn load_env_file(&self) -> Result<(), CommandError> {
        if let Some(path) = &self.env {
            dotenvy::from_path(path).map_err(|error| {
                CommandError::diagnostic(format!("failed to load env file: {path}"))
                    .detail(error.to_string())
                    .hint("pass an existing file to --env")
            })?;
        }
        Ok(())
    }

    /// Applies CLI configuration overrides and validates the selected command mode.
    pub(crate) fn apply_to(self, config: &mut Config) -> Result<Command, CommandError> {
        if let Some(dir) = self.migrations_dir {
            config.migrations_dir = dir.into();
        }
        if let Some(schema) = self.schema {
            config.schema_file = schema.into();
        }
        if let Some(url) = self.database_url {
            config.dialect = Config::dialect_from_database_url(&url).map_err(|error| {
                CommandError::diagnostic(format!("invalid database_url: {error}"))
                    .hint("use postgres://, postgresql://, or sqlite://")
            })?;
            config.database_url = url;
        }
        self.command.validate()?;
        Ok(self.command)
    }
}

impl Command {
    /// Rejects combinations whose order-dependent behavior would be surprising.
    pub(crate) fn validate(&self) -> Result<(), CommandError> {
        match self {
            Self::Make(command) => validate_make(command),
            Self::Apply(command) => validate_apply(command),
            Self::Repair(command) if command.apply && command.sql_only => Err(
                CommandError::diagnostic("--apply cannot be combined with --sql-only"),
            ),
            _ => Ok(()),
        }
    }

    /// Returns whether this command may persist a generated migration file.
    pub(crate) fn requires_writable_migrations(&self) -> bool {
        matches!(self, Self::Make(command) if !command.check && !command.dry_run)
    }
}

/// Validates exclusive make modes and their required names.
fn validate_make(command: &MakeCmd) -> Result<(), CommandError> {
    let special_modes =
        usize::from(command.empty) + usize::from(command.merge) + usize::from(command.check);
    if special_modes > 1 {
        return Err(CommandError::diagnostic(
            "--empty, --merge, and --check are mutually exclusive",
        ));
    }
    if special_modes == 0 {
        return Ok(());
    }
    if command.dry_run || command.non_interactive {
        return Err(CommandError::diagnostic(
            "--dry-run and --non-interactive apply only to normal migration generation",
        ));
    }
    if (command.empty || command.merge) && command.name.is_none() {
        return Err(CommandError::diagnostic(
            "a name is required for --empty and --merge",
        ));
    }
    if command.check && command.name.is_some() {
        return Err(CommandError::diagnostic(
            "make --check does not accept a migration name",
        ));
    }
    Ok(())
}

/// Validates exclusive apply modes and target support.
fn validate_apply(command: &ApplyCmd) -> Result<(), CommandError> {
    let modes = usize::from(command.fake) + usize::from(command.plan) + usize::from(command.check);
    if modes > 1 {
        return Err(CommandError::diagnostic(
            "--fake, --plan, and --check are mutually exclusive",
        ));
    }
    if command.target.is_some() && (command.plan || command.check) {
        return Err(CommandError::diagnostic(
            "a target id is supported only when applying or fake-applying migrations",
        ));
    }
    Ok(())
}
