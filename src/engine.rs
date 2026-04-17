use std::sync::Arc;

use thiserror::Error;

use crate::adapters::AdapterError;
use crate::cli::{CommandError, GamanArgs, dispatch};
use crate::conf::Config;
use crate::dialects::Dialect;
use crate::executor::postgres::PostgresExecutor;
use crate::migrator::{Migrator, MigratorError};
use crate::states::{Schema, SchemaBuilder};
use crate::embed::EmbedSource;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("{0}")]
    Command(#[from] CommandError),
    #[error("migration error: {0}")]
    Migrator(#[from] MigratorError),
    #[error("database connection failed: {0}")]
    Connect(String),
    #[error("migration source error: {0}")]
    Adapter(#[from] AdapterError),
    #[error("{0}")]
    Config(String),
}

pub struct MigrationEngine {
    config: Config,
    migrations: &'static [(&'static str, &'static str)],
    schema: Option<Schema>,
    tls: TlsMode,
}

pub enum TlsMode {
    NoTls,
}

impl MigrationEngine {
    pub fn new(config: Config, migrations: &'static [(&'static str, &'static str)]) -> Self {
        Self {
            config,
            migrations,
            schema: None,
            tls: TlsMode::NoTls,
        }
    }

    pub fn with_schema(mut self, f: impl FnOnce(SchemaBuilder) -> Schema) -> Self {
        self.schema = Some(f(SchemaBuilder::new(Dialect::Postgres)));
        self
    }

    pub fn with_tls(mut self, tls: TlsMode) -> Self {
        self.tls = tls;
        self
    }

    fn connect(&self) -> Result<PostgresExecutor, EngineError> {
        let url = self.config.database_url.as_deref()
            .ok_or_else(|| EngineError::Config(
                "database_url is not set — set DATABASE_URL or pass it in Config".into()
            ))?;
        let client = match self.tls {
            TlsMode::NoTls => postgres::Client::connect(url, postgres::NoTls)
                .map_err(|e| EngineError::Connect(e.to_string()))?,
        };
        Ok(PostgresExecutor::new(client))
    }

    fn build_migrator(&self) -> Result<Migrator, EngineError> {
        let config = Arc::new(self.config.clone());
        let source = Box::new(EmbedSource::new(self.migrations));
        Ok(Migrator::new(config, source, Dialect::Postgres)?)
    }

    /// Apply all pending migrations. Safe to call on every startup.
    pub fn migrate(self) -> Result<(), EngineError> {
        let migrator = self.build_migrator()?;
        let mut executor = self.connect()?;
        migrator.migrate(&mut executor, None, None, false)?;
        Ok(())
    }

    /// Parse `std::env::args()` and dispatch the corresponding subcommand using
    /// the embedded migration source and the optionally provided schema.
    /// Supports the full CLI interface: make_migration, migrate, verify_db, etc.
    pub fn handle_args(self) -> Result<(), EngineError> {
        let _ = dotenvy::dotenv();
        let args: GamanArgs = argh::from_env();
        let mut config = self.config;
        let cmd = args.apply_to(&mut config);
        let config = Arc::new(config);
        let source = Box::new(EmbedSource::new(self.migrations));
        let migrator = Migrator::new(config, source, Dialect::Postgres)?;
        dispatch(migrator, self.schema, cmd)?;
        Ok(())
    }
}
