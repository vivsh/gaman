use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource, YamlAdapter};
use crate::cli::{CommandError, GamanArgs, dispatch};
use crate::conf::Config;
use crate::dialects::Dialect;
use crate::disambiguator::{Decision, PromptEngine};
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::postgres::PostgresExecutor;
use crate::executor::Invoker;
use crate::migrator::{Migrator, MigratorError};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::prompter::CliPromptEngine;
use crate::states::{Schema, SchemaBuilder};

pub struct EmbeddedMigrations {
    pub files: &'static [(&'static str, &'static str)],
    pub dir: &'static str,
}

struct EmbedSource {
    migrations: &'static [(&'static str, &'static str)],
}

impl EmbedSource {
    fn new(migrations: &'static [(&'static str, &'static str)]) -> Self {
        Self { migrations }
    }
}

impl MigrationSource for EmbedSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        self.migrations
            .iter()
            .map(|(id, content)| {
                let mut m: Migration =
                    serde_yaml::from_str(content).map_err(|e| AdapterError::Parse {
                        path: id.to_string(),
                        message: e.to_string(),
                    })?;
                m.id = id.to_string();
                Ok(m)
            })
            .collect()
    }

    fn save(&self, _migration: &Migration) -> Result<(), AdapterError> {
        Err(AdapterError::Io {
            path: "<embedded>".to_string(),
            message: "cannot save to an embedded migration source".to_string(),
        })
    }
}

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
    #[error("no schema set — call with_schema() before make_migration()")]
    NoSchema,
    #[error("migrations dir mismatch: config has '{0}', embedded was compiled from '{1}' — they must match for make_migration")]
    MigrationsDirMismatch(String, &'static str),
}

pub struct MigrationEngine {
    config: Config,
    migrations: &'static EmbeddedMigrations,
    schema: Option<Schema>,
    tls: TlsMode,
}

#[derive(Clone, Copy)]
pub enum TlsMode {
    NoTls,
}

struct EngineEnvironment {
    config: Arc<Config>,
    tls: TlsMode,
}

impl EngineEnvironment {
    fn new(config: Arc<Config>, tls: TlsMode) -> Self {
        Self { config, tls }
    }
}

impl Environment for EngineEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor(&self) -> Result<Box<dyn EnvironmentExecutor>, EnvironmentError> {
        let url = self.config.database_url.as_deref()
            .ok_or_else(|| EnvironmentError::Config(
                "database_url is not set — set DATABASE_URL or pass it in Config".into(),
            ))?;
        let client = match (self.dialect(), self.tls) {
            (Dialect::Postgres, TlsMode::NoTls) => postgres::Client::connect(url, postgres::NoTls)
                .map_err(|e| EnvironmentError::Connect(e.to_string()))?,
        };
        Ok(Box::new(PostgresExecutor::new(client)))
    }

    fn invoker(&self) -> Result<Option<Box<dyn Invoker>>, EnvironmentError> {
        Ok(None)
    }
}

impl MigrationEngine {
    pub fn new(config: Config, migrations: &'static EmbeddedMigrations) -> Self {
        Self {
            config,
            migrations,
            schema: None,
            tls: TlsMode::NoTls,
        }
    }

    pub fn with_schema(mut self, f: impl FnOnce(SchemaBuilder) -> Schema) -> Self {
        let dialect = self.config.dialect().unwrap_or(Dialect::Postgres);
        self.schema = Some(f(SchemaBuilder::new(dialect)));
        self
    }

    pub fn with_tls(mut self, tls: TlsMode) -> Self {
        self.tls = tls;
        self
    }

    pub fn with_database_url(mut self, url: impl Into<String>) -> Self {
        self.config.database_url = Some(url.into());
        self
    }

    fn build_migrator(&self) -> Result<Migrator, EngineError> {
        let source = Box::new(EmbedSource::new(self.migrations.files));
        let environment = Box::new(EngineEnvironment::new(Arc::new(self.config.clone()), self.tls));
        Ok(Migrator::new(source, environment)?)
    }

    /// Apply all pending migrations. Returns the number applied. Safe to call on every startup.
    pub fn migrate(self) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(None, false)?)
    }

    /// Migrate forward or backward to `target` migration id.
    pub fn migrate_to(self, target: &str) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(Some(target), false)?)
    }

    /// Mark all pending migrations as applied without running any SQL.
    /// Useful for bootstrapping a database that was set up outside gaman.
    pub fn fake_migrate(self) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(None, true)?)
    }

    /// Return true if there are unapplied migrations.
    pub fn check(self) -> Result<bool, EngineError> {
        Ok(self.build_migrator()?.check()?)
    }

    /// Return the ordered list of migration ids that would be applied.
    pub fn plan(self) -> Result<Vec<String>, EngineError> {
        Ok(self.build_migrator()?.plan()?)
    }

    /// Return all migration ids with their applied/pending status.
    pub fn show_migrations(self) -> Result<Vec<(String, bool)>, EngineError> {
        Ok(self.build_migrator()?.show_migrations()?)
    }

    /// Compare the replayed schema against the live database and return any drift operations.
    /// An empty vec means the database is in sync with migrations.
    pub fn verify(self, schema: &str) -> Result<Vec<Operation>, EngineError> {
        Ok(self.build_migrator()?.verify(schema)?)
    }

    /// Introspect the live database and return the schema.
    pub fn inspect_db(self, schemas: &[&str]) -> Result<Schema, EngineError> {
        Ok(self.build_migrator()?.inspect_db(schemas)?)
    }

    /// Diff the stored schema against the replayed migration state and save a new migration if
    /// there are changes. Returns the migration if one was created, or `None` if the schema is
    /// already up to date.
    ///
    /// Requires `with_schema()` to have been called — returns `Err(EngineError::NoSchema)` if not.
    /// Any rename/ambiguity clarifications are resolved interactively via terminal prompts.
    pub fn make_migration(self, name: &str) -> Result<Option<Migration>, EngineError> {
        if PathBuf::from(self.migrations.dir) != self.config.migrations_dir {
            return Err(EngineError::MigrationsDirMismatch(
                self.config.migrations_dir.display().to_string(),
                self.migrations.dir,
            ));
        }
        let source = Box::new(YamlAdapter { directory: self.config.migrations_dir.clone() });
        let environment = Box::new(EngineEnvironment::new(Arc::new(self.config.clone()), self.tls));
        let migrator = Migrator::new(source, environment)?;
        let schema = self.schema.ok_or(EngineError::NoSchema)?;
        let engine = CliPromptEngine;
        let mut decisions: Vec<Decision> = vec![];
        loop {
            match migrator.make_migrations(name.to_string(), schema.clone(), false, &decisions) {
                Err(MigratorError::NeedsInput(clars)) => {
                    let new = engine.prompt(&clars).map_err(|e| EngineError::Config(e.to_string()))?;
                    decisions.extend(new);
                }
                Err(e) => return Err(EngineError::Migrator(e)),
                Ok(result) => return Ok(result),
            }
        }
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
        let source = Box::new(YamlAdapter { directory: config.migrations_dir.clone() });
        let environment = Box::new(EngineEnvironment::new(config, self.tls));
        let migrator = Migrator::new(source, environment)?;
        dispatch(migrator, self.schema, cmd)?;
        Ok(())
    }
}
