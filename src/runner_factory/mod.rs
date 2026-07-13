//! Native adapters that construct the shared core command runner.

use std::sync::Arc;

use gaman_core::{
    BoxFuture, DatabaseTrackingStore, EmbeddedMigrations, MigrationRunner, MigrationStore,
    StoreError,
};

use crate::conf::Config;
use crate::migration_store::validate_embedded_directory;

mod lazy_executor;

pub use crate::migration_store::{DirectoryMigrationStore, EmbeddedMigrationStore};
pub use lazy_executor::LazyExecutor;

/// Native error returned while constructing a runner or using its filesystem adapters.
#[derive(Debug, thiserror::Error)]
pub enum NativeRunnerError {
    /// The configured embedded migration source cannot safely write migrations.
    #[error(
        "migrations dir mismatch: config has '{configured}', embedded was compiled from '{embedded}'"
    )]
    EmbeddedDirectoryMismatch {
        configured: String,
        embedded: String,
    },
    /// The process working directory could not be resolved for path comparison.
    #[error("cannot resolve current directory: {0}")]
    CurrentDirectory(String),
}

/// Native host factory for a lazily connected [`MigrationRunner`].
pub struct NativeRunnerFactory {
    config: Config,
    migrations: NativeMigrationStore,
}

impl NativeRunnerFactory {
    /// Creates a factory backed by the configured migration directory.
    pub fn from_directory(config: Config) -> Self {
        let migrations =
            NativeMigrationStore::Directory(DirectoryMigrationStore::new(&config.migrations_dir));
        Self { config, migrations }
    }

    /// Creates a factory over one caller-supplied core migration store.
    pub fn from_store(config: Config, migrations: Arc<dyn MigrationStore>) -> Self {
        Self {
            config,
            migrations: NativeMigrationStore::Custom(migrations),
        }
    }

    /// Creates a factory backed by compiled migration history with filesystem persistence.
    pub fn from_embedded(
        config: Config,
        migrations: &'static EmbeddedMigrations,
    ) -> Result<Self, NativeRunnerError> {
        validate_embedded_directory(&config.migrations_dir, migrations.dir)?;
        Ok(Self {
            migrations: NativeMigrationStore::Embedded(EmbeddedMigrationStore::new(
                migrations,
                DirectoryMigrationStore::new(config.migrations_dir.clone()),
            )),
            config,
        })
    }

    /// Returns the validated native configuration used by this factory.
    pub fn config(&self) -> &Config {
        &self.config
    }

    /// Builds one host-neutral command runner without opening a database connection.
    pub fn build(
        self,
    ) -> MigrationRunner<NativeMigrationStore, DatabaseTrackingStore, LazyExecutor> {
        let dialect = self.config.dialect;
        MigrationRunner::new(
            dialect,
            self.migrations,
            DatabaseTrackingStore,
            LazyExecutor::new(self.config),
        )
    }
}

/// Migration source variants used by the native host factory.
#[derive(Clone)]
pub enum NativeMigrationStore {
    /// One directory of canonical migration YAML files.
    Directory(DirectoryMigrationStore),
    /// Compiled history with generated migrations persisted to its source directory.
    Embedded(EmbeddedMigrationStore),
    /// A caller-owned core store.
    Custom(Arc<dyn MigrationStore>),
}

impl MigrationStore for NativeMigrationStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<gaman_core::Migration>, StoreError>> {
        match self {
            Self::Directory(store) => store.load_all(),
            Self::Embedded(store) => store.load_all(),
            Self::Custom(store) => store.load_all(),
        }
    }

    fn save<'a>(
        &'a self,
        migration: &'a gaman_core::Migration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        match self {
            Self::Directory(store) => store.save(migration),
            Self::Embedded(store) => store.save(migration),
            Self::Custom(store) => store.save(migration),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gaman_core::Dialect;

    static CHILD: EmbeddedMigrations = EmbeddedMigrations {
        files: &[
            ("0001_auth.yaml", "operations: []\n"),
            (
                "0002_profile.yaml",
                "dependencies:\n- 0001_auth\noperations: []\n",
            ),
        ],
        dir: "migrations/auth",
        children: &[],
    };
    static TREE: EmbeddedMigrations = EmbeddedMigrations {
        files: &[("0001_root.yaml", "operations: []\n")],
        dir: "migrations",
        children: &[("auth", &CHILD)],
    };
    static DUPLICATE: EmbeddedMigrations = EmbeddedMigrations {
        files: &[],
        dir: "migrations",
        children: &[("auth", &CHILD), ("auth", &CHILD)],
    };

    /// Verifies embedded trees qualify child IDs and local dependencies recursively.
    #[tokio::test]
    async fn embedded_store_qualifies_child_history() {
        let directory = tempfile::tempdir().expect("temporary migration directory");
        let store =
            EmbeddedMigrationStore::new(&TREE, DirectoryMigrationStore::new(directory.path()));

        let migrations = store.load_all().await.expect("load embedded tree");
        let ids = migrations
            .iter()
            .map(|migration| migration.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            ids,
            vec!["0001_root", "auth/0001_auth", "auth/0002_profile"]
        );
        assert_eq!(migrations[2].dependencies, vec!["auth/0001_auth"]);
    }

    /// Verifies duplicate fully-qualified embedded migration IDs are rejected.
    #[tokio::test]
    async fn embedded_store_rejects_duplicate_ids() {
        let directory = tempfile::tempdir().expect("temporary migration directory");
        let store =
            EmbeddedMigrationStore::new(&DUPLICATE, DirectoryMigrationStore::new(directory.path()));

        let error = store.load_all().await.expect_err("duplicate IDs must fail");
        assert!(
            error
                .to_string()
                .contains("duplicate embedded migration id")
        );
    }

    /// Verifies the native factory rejects embedded and configured directory mismatches.
    #[test]
    fn embedded_factory_rejects_directory_mismatch() {
        let config = Config::new(
            "sqlite::memory:".to_string(),
            "different-migrations".into(),
            "schema.yaml".into(),
            Dialect::Sqlite,
        );

        let error = match NativeRunnerFactory::from_embedded(config, &TREE) {
            Ok(_) => panic!("directory mismatch must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("migrations dir mismatch"));
    }
}
