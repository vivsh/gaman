use std::path::PathBuf;
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource};
use crate::conf::Config;
use crate::dialects::Dialect;
use crate::executor::postgres::PostgresExecutor;
use crate::migrator::{Migrator, MigratorError};
use crate::migrations::Migration;

#[derive(Debug, Error)]
pub enum EmbedError {
    #[error("migration error: {0}")]
    Migrator(#[from] MigratorError),
    #[error("database connection failed: {0}")]
    Connect(String),
    #[error("migration source error: {0}")]
    Adapter(#[from] AdapterError),
}

/// In-memory migration source backed by static content, typically produced by
/// `include_migrations!` from the `gaman-macros` crate.
pub struct EmbedSource {
    migrations: &'static [(&'static str, &'static str)],
}

impl EmbedSource {
    pub fn new(migrations: &'static [(&'static str, &'static str)]) -> Self {
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

/// Connects to Postgres and applies all pending migrations from an embedded source.
///
/// The typical usage is with [`include_migrations!`](https://docs.rs/gaman-macros) to
/// bundle migration YAML files directly into the binary at compile time:
///
/// ```no_run
/// use gaman::embed::Runner;
/// use gaman_macros::include_migrations;
///
/// Runner::new("postgres://localhost/mydb", include_migrations!("migrations"))
///     .run()
///     .expect("migrations failed");
/// ```
pub struct Runner {
    database_url: String,
    source: EmbedSource,
}

impl Runner {
    pub fn new(
        database_url: impl Into<String>,
        migrations: &'static [(&'static str, &'static str)],
    ) -> Self {
        Self {
            database_url: database_url.into(),
            source: EmbedSource::new(migrations),
        }
    }

    /// Connect and apply all pending migrations. Safe to call on every startup —
    /// already-applied migrations are skipped via the tracking table.
    pub fn run(self) -> Result<(), EmbedError> {
        let client = postgres::Client::connect(&self.database_url, postgres::NoTls)
            .map_err(|e| EmbedError::Connect(e.to_string()))?;
        let mut executor = PostgresExecutor::new(client);
        let config = Arc::new(Config::new(
            Some(self.database_url),
            PathBuf::new(),
            PathBuf::new(),
        ));
        let migrator = Migrator::new(config, Box::new(self.source), Dialect::Postgres)?;
        migrator.migrate(&mut executor, None, None, false)?;
        Ok(())
    }
}
