//! Native configuration resolution around the shared `gaman-core` command grammar.

use crate::cli::diagnostic::CommandError;
use crate::conf::Config;

#[allow(unused_imports)]
pub use gaman_core::command_args::{
    ApplyCmd, CheckSchemaCmd, Command, CommandArgs as GamanArgs, InspectCmd, MakeCmd, RepairCmd,
    ShowCmd, ShowConfigCmd, SqlCmd, StatusCmd, VerifyCmd,
};

/// Loads an explicitly requested environment file before native configuration resolution.
pub(crate) fn load_env_file(args: &GamanArgs) -> Result<(), CommandError> {
    if let Some(path) = &args.env {
        dotenvy::from_path(path).map_err(|error| {
            CommandError::diagnostic(format!("failed to load env file: {path}"))
                .detail(error.to_string())
                .hint("pass an existing file to --env")
        })?;
    }
    Ok(())
}

/// Applies native configuration overrides after shared command validation succeeds.
pub(crate) fn apply_to(args: GamanArgs, config: &mut Config) -> Result<Command, CommandError> {
    if let Some(dir) = args.migrations_dir {
        config.migrations_dir = dir.into();
    }
    if let Some(schema) = args.schema {
        config.schema_file = schema.into();
    }
    if let Some(url) = args.database_url {
        config.dialect = Config::dialect_from_database_url(&url).map_err(|error| {
            CommandError::diagnostic(format!("invalid database_url: {error}"))
                .hint("use postgres://, postgresql://, sqlite://, mysql://, or mariadb://")
        })?;
        config.database_url = url;
    }
    args.command
        .validate()
        .map_err(CommandError::from_argument_diagnostic)?;
    Ok(args.command)
}
