//! Shared token-oriented command grammar, help text, and parser diagnostics.
//!
//! These types are the authoritative Gaman command-line vocabulary. Native CLI
//! uses `argh::from_env`; WASM and FFI callers can parse supplied token slices
//! with [`CommandArgs::parse`]. They resolve into typed runner commands in their
//! own host adapter and therefore never carry loaded schemas or database state.

use std::fmt;

use argh::{ArgsInfo, CommandInfoWithArgs, FlagInfoKind, FromArgs, Optionality};
use serde::{Deserialize, Serialize};

/// Parsed global options and one token-oriented Gaman command.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
/// Gaman schema migration command interface.
pub struct CommandArgs {
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
    /// show internal error causes after the concise diagnostic (env: GAMAN_DEBUG=1)
    #[argh(switch)]
    pub verbose: bool,
    /// print the Gaman version
    #[argh(switch, short = 'V')]
    pub version: bool,
    #[argh(subcommand)]
    pub command: Command,
}

/// Parsed command kind before host-specific schema and configuration resolution.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand)]
pub enum Command {
    Make(MakeCmd),
    Apply(ApplyCmd),
    Status(StatusCmd),
    Show(ShowCmd),
    Sql(SqlCmd),
    CheckSchema(CheckSchemaCmd),
    Config(ShowConfigCmd),
    Inspect(InspectCmd),
    Verify(VerifyCmd),
    Repair(RepairCmd),
}

/// Print the resolved host configuration.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand, name = "config")]
pub struct ShowConfigCmd {
    /// print the complete database URL, including credentials
    #[argh(switch)]
    pub show_database_url: bool,
}

/// Write a new migration from the current schema.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
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
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
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
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
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
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand, name = "sql")]
pub struct SqlCmd {
    /// full migration id or unique id prefix; omit to render all migrations
    #[argh(positional)]
    pub id: Option<String>,
    /// print the backward direction instead of forward SQL
    #[argh(switch)]
    pub backwards: bool,
}

/// Prepare authored SQL schema files without executing them.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand, name = "check_schema")]
pub struct CheckSchemaCmd {}

/// Apply pending migrations or converge on a target migration.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
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
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand, name = "verify")]
pub struct VerifyCmd {
    /// schema to verify; may be repeated (default: dialect namespace)
    #[argh(option)]
    pub schema: Vec<String>,
}

/// Introspect a live database and print schema YAML.
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
#[argh(subcommand, name = "inspect")]
pub struct InspectCmd {
    /// namespaces to inspect; may repeat (default: dialect namespace)
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
#[derive(FromArgs, ArgsInfo, Debug, Clone)]
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
    /// schema to repair; may be repeated (default: dialect namespace)
    #[argh(option)]
    pub schema: Vec<String>,
}

/// Parser output intended for presentation without interpreting `argh` text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ArgumentDiagnostic {
    /// Exact parser-generated text.
    pub output: String,
    /// Whether parser output is informational help rather than an error.
    pub success: bool,
}

impl ArgumentDiagnostic {
    /// Creates one semantic argument-validation error using shared command wording.
    pub fn error(message: impl Into<String>) -> Self {
        Self {
            output: message.into(),
            success: false,
        }
    }
}

impl fmt::Display for ArgumentDiagnostic {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.output)
    }
}

impl std::error::Error for ArgumentDiagnostic {}

/// Serializable command and field metadata derived from `argh::ArgsInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommandMetadata {
    /// Command name.
    pub name: String,
    /// One-line command description.
    pub description: String,
    /// Option and positional metadata.
    pub fields: Vec<FieldMetadata>,
    /// Nested command metadata.
    pub commands: Vec<CommandMetadata>,
}

/// One documented option or positional field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FieldMetadata {
    /// Long option name or positional placeholder.
    pub name: String,
    /// Optional short option spelling.
    pub short: Option<char>,
    /// Whether the field expects a value.
    pub takes_value: bool,
    /// Whether the field is required.
    pub required: bool,
    /// User-facing description from the shared annotations.
    pub description: String,
}

/// Shared product wording rendered by hosts with their own executable and version.
#[derive(Debug, Clone, Copy)]
pub struct ProductPresentation {
    /// Product name.
    pub name: &'static str,
    /// Short product tagline.
    pub tagline: &'static str,
    /// Generic command-help guidance.
    pub command_help: &'static str,
}

/// Host-specific values inserted into the shared product banner.
#[derive(Debug, Clone, Copy)]
pub struct BuildInfo<'a> {
    /// Invocation name used in help examples.
    pub executable: &'a str,
    /// Version of the host package currently running.
    pub version: &'a str,
}

/// Returns shared product wording for every Gaman host.
pub const fn product_presentation() -> ProductPresentation {
    ProductPresentation {
        name: "Gaman",
        tagline: "Offline-first migration tool",
        command_help: "Use '<command> --help' for help on a specific command.",
    }
}

impl ProductPresentation {
    /// Renders the common no-command banner without performing any output itself.
    pub fn banner(self, build: BuildInfo<'_>) -> String {
        format!(
            "{} v{}\n{}\n\nType '{} --help' for usage.\nType '{} {}'",
            self.name,
            build.version,
            self.tagline,
            build.executable,
            build.executable,
            self.command_help
        )
    }
}

impl CommandArgs {
    /// Parses supplied command tokens without reading process arguments or environment state.
    pub fn parse(command_name: &[&str], args: &[&str]) -> Result<Self, ArgumentDiagnostic> {
        Self::from_args(command_name, args).map_err(|exit| ArgumentDiagnostic {
            output: exit.output,
            success: exit.status.is_ok(),
        })
    }

    /// Returns exact generated help text for the top-level command or one subcommand.
    pub fn command_help(command_name: &[&str], command: Option<&str>) -> ArgumentDiagnostic {
        let args = command.map_or_else(|| vec!["help"], |name| vec![name, "help"]);
        match Self::parse(command_name, &args) {
            Err(diagnostic) => diagnostic,
            Ok(_) => ArgumentDiagnostic::error("help did not produce parser output"),
        }
    }

    /// Returns serializable help metadata derived from the same annotated command grammar.
    pub fn command_metadata() -> CommandMetadata {
        metadata_from(Self::get_args_info())
    }
}

impl Command {
    /// Validates cross-field combinations that cannot be expressed by `argh` field annotations.
    pub fn validate(&self) -> Result<(), ArgumentDiagnostic> {
        match self {
            Self::Make(command) => validate_make(command),
            Self::Apply(command) => validate_apply(command),
            Self::Repair(command) if command.apply && command.sql_only => Err(
                ArgumentDiagnostic::error("--apply cannot be combined with --sql-only"),
            ),
            _ => Ok(()),
        }
    }

    /// Reports whether a command needs writable migration storage.
    pub fn requires_writable_migrations(&self) -> bool {
        matches!(self, Self::Make(command) if !command.check && !command.dry_run)
    }

    /// Reports whether a command only validates SQL schema inputs.
    pub fn validates_schema_sql_only(&self) -> bool {
        matches!(self, Self::CheckSchema(_))
    }
}

fn metadata_from(info: CommandInfoWithArgs) -> CommandMetadata {
    let mut fields = info.flags.iter().map(flag_metadata).collect::<Vec<_>>();
    fields.extend(info.positionals.iter().map(positional_metadata));
    CommandMetadata {
        name: info.name.to_string(),
        description: info.description.to_string(),
        fields,
        commands: info
            .commands
            .into_iter()
            .map(|command| metadata_from(command.command))
            .collect(),
    }
}

fn flag_metadata(flag: &argh::FlagInfo<'_>) -> FieldMetadata {
    FieldMetadata {
        name: flag.long.trim_start_matches("--").to_string(),
        short: flag.short,
        takes_value: matches!(flag.kind, FlagInfoKind::Option { .. }),
        required: matches!(flag.optionality, Optionality::Required),
        description: flag.description.to_string(),
    }
}

fn positional_metadata(positional: &argh::PositionalInfo<'_>) -> FieldMetadata {
    FieldMetadata {
        name: positional.name.to_string(),
        short: None,
        takes_value: true,
        required: matches!(positional.optionality, Optionality::Required),
        description: positional.description.to_string(),
    }
}

fn validate_make(command: &MakeCmd) -> Result<(), ArgumentDiagnostic> {
    let special_modes =
        usize::from(command.empty) + usize::from(command.merge) + usize::from(command.check);
    if special_modes > 1 {
        return Err(ArgumentDiagnostic::error(
            "--empty, --merge, and --check are mutually exclusive",
        ));
    }
    if command.check && (command.dry_run || command.non_interactive) {
        return Err(ArgumentDiagnostic::error(
            "--dry-run and --non-interactive apply only to normal migration generation",
        ));
    }
    if command.check && command.name.is_some() {
        return Err(ArgumentDiagnostic::error(
            "--check does not accept a migration name",
        ));
    }
    if command.name.is_none() && (command.empty || command.merge) {
        return Err(ArgumentDiagnostic::error(
            "a name is required for --empty and --merge",
        ));
    }
    Ok(())
}

fn validate_apply(command: &ApplyCmd) -> Result<(), ArgumentDiagnostic> {
    let modes = usize::from(command.fake) + usize::from(command.plan) + usize::from(command.check);
    if modes > 1 {
        return Err(ArgumentDiagnostic::error(
            "--fake, --plan, and --check are mutually exclusive",
        ));
    }
    if (command.plan || command.check) && command.target.is_some() {
        return Err(ArgumentDiagnostic::error(
            "a target id is supported only when applying migrations",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies caller-supplied tokens parse without process argument access.
    #[test]
    fn parses_make_from_supplied_tokens() {
        let parsed = CommandArgs::parse(&["gaman"], &["make", "users"]).unwrap();
        let Command::Make(make) = parsed.command else {
            panic!("expected make command");
        };
        assert_eq!(make.name.as_deref(), Some("users"));
    }

    /// Verifies generated top-level help retains the shared command description.
    #[test]
    fn generated_help_uses_shared_annotations() {
        let help = CommandArgs::command_help(&["gaman"], None);
        assert!(help.success);
        assert!(
            help.output
                .contains("Gaman schema migration command interface")
        );
        assert!(
            help.output
                .contains("Write a new migration from the current schema")
        );
    }

    /// Verifies serializable metadata follows the same annotated command hierarchy.
    #[test]
    fn metadata_includes_make_command_and_schema_field() {
        let metadata = CommandArgs::command_metadata();
        assert!(metadata.fields.iter().any(|field| field.name == "schema"));
        assert!(
            metadata
                .commands
                .iter()
                .any(|command| command.name == "make")
        );
    }

    /// Verifies parser failures retain the exact argh error text for host presentation.
    #[test]
    fn parser_failure_preserves_argh_output() {
        let error = CommandArgs::parse(&["gaman"], &["unknown"]).unwrap_err();
        assert!(!error.success);
        assert!(error.output.contains("Unrecognized argument"));
    }
}
