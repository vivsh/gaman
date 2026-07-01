use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource, YamlAdapter};
use crate::cli::{CommandError, GamanArgs, dispatch, parse_dialect};
use crate::conf::Config;
use crate::dialects::Dialect;
use crate::disambiguator::{Decision, PromptEngine};
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{BoxFuture, connect_environment_executor};
use crate::migrations::Migration;
use crate::migrator::{Migrator, MigratorError};
use crate::operations::Operation;
use crate::prompter::CliPromptEngine;
use crate::states::{IntoSchema, Schema, SchemaBuilder, SchemaLoadError};

#[derive(Copy, Clone)]
pub struct EmbeddedMigrations {
    pub files: &'static [(&'static str, &'static str)],
    pub dir: &'static str,
    pub children: &'static [(&'static str, &'static EmbeddedMigrations)],
}

struct EmbedSource {
    root: &'static EmbeddedMigrations,
    extra: Vec<(&'static str, &'static EmbeddedMigrations)>,
}

impl EmbedSource {
    fn new(
        root: &'static EmbeddedMigrations,
        extra: Vec<(&'static str, &'static EmbeddedMigrations)>,
    ) -> Self {
        Self { root, extra }
    }

    fn collect(
        source: &'static EmbeddedMigrations,
        prefix: &str,
        out: &mut Vec<Migration>,
    ) -> Result<(), AdapterError> {
        for (id, content) in source.files {
            let qualified_id = if prefix.is_empty() {
                id.to_string()
            } else {
                format!("{prefix}/{id}")
            };
            let mut m: Migration =
                serde_yaml::from_str(content).map_err(|e| AdapterError::Parse {
                    path: qualified_id.clone(),
                    message: e.to_string(),
                })?;
            m.id = qualified_id;
            if !prefix.is_empty() {
                for dep in &mut m.dependencies {
                    if !dep.contains('/') {
                        *dep = format!("{prefix}/{dep}");
                    }
                }
            }
            out.push(m);
        }
        for (ns, child) in source.children {
            let child_prefix = if prefix.is_empty() {
                ns.to_string()
            } else {
                format!("{prefix}/{ns}")
            };
            Self::collect(child, &child_prefix, out)?;
        }
        Ok(())
    }
}

impl MigrationSource for EmbedSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        let mut out = Vec::new();
        Self::collect(self.root, "", &mut out)?;
        for (ns, child) in &self.extra {
            Self::collect(child, ns, &mut out)?;
        }
        Ok(out)
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
    #[error("schema load error: {0}")]
    SchemaLoad(#[from] SchemaLoadError),
    #[error(
        "migrations dir mismatch: config has '{0}', embedded was compiled from '{1}' — they must match for make_migration"
    )]
    MigrationsDirMismatch(String, &'static str),
}

pub struct MigrationEngine {
    config: Config,
    migrations: &'static EmbeddedMigrations,
    extra: Vec<(&'static str, &'static EmbeddedMigrations)>,
    schema: Option<Schema>,
    dialect: Option<Dialect>,
}

struct EngineEnvironment {
    config: Arc<Config>,
    dialect: Option<Dialect>,
}

impl EngineEnvironment {
    fn new(config: Arc<Config>, dialect: Option<Dialect>) -> Self {
        Self { config, dialect }
    }
}

impl Environment for EngineEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, EnvironmentError>> {
        Box::pin(async move {
            let url = self.config.database_url.as_deref().ok_or_else(|| {
                EnvironmentError::Config(
                    "database_url is not set — set DATABASE_URL or pass it in Config".into(),
                )
            })?;
            connect_environment_executor(self.dialect(), url, self.config.tls)
                .await
                .map_err(EnvironmentError::from)
        })
    }

    fn dialect(&self) -> Dialect {
        self.dialect
            .or_else(|| self.config.dialect())
            .unwrap_or(Dialect::Postgres)
    }
}

impl MigrationEngine {
    pub fn new(config: Config, migrations: &'static EmbeddedMigrations) -> Self {
        Self {
            config,
            migrations,
            extra: Vec::new(),
            schema: None,
            dialect: None,
        }
    }

    pub fn with_dialect(mut self, dialect: Dialect) -> Self {
        self.dialect = Some(dialect);
        self
    }

    pub fn add_migrations(mut self, ns: &'static str, m: &'static EmbeddedMigrations) -> Self {
        self.extra.push((ns, m));
        self
    }

    /// Set the target schema. The closure receives a `SchemaBuilder` and can return either a
    /// `Schema` directly (infallible) or a `Result<Schema, SchemaLoadError>` (for file loading).
    /// Calling this more than once replaces the previous schema — last call wins.
    pub fn with_schema<R: IntoSchema>(
        mut self,
        f: impl FnOnce(SchemaBuilder) -> R,
    ) -> Result<Self, EngineError> {
        let dialect = self
            .dialect
            .or_else(|| self.config.dialect())
            .unwrap_or(Dialect::Postgres);
        self.schema = Some(
            f(SchemaBuilder::new(dialect))
                .into_schema()?
                .prepare(dialect)
                .map_err(|err| EngineError::Config(err.to_string()))?,
        );
        Ok(self)
    }

    fn build_migrator(&self) -> Result<Migrator, EngineError> {
        let source = Box::new(EmbedSource::new(self.migrations, self.extra.clone()));
        let environment = Box::new(EngineEnvironment::new(
            Arc::new(self.config.clone()),
            self.dialect,
        ));
        Ok(Migrator::new(source, environment)?)
    }

    fn writable_migrations_dir(&self) -> Result<PathBuf, EngineError> {
        let embedded_dir = absolute_path(Path::new(self.migrations.dir))?;
        let config_dir = Path::new(&self.config.migrations_dir);

        let matches = if config_dir.is_absolute() {
            absolute_path(config_dir)? == embedded_dir
        } else {
            absolute_path(config_dir)? == embedded_dir || embedded_dir.ends_with(config_dir)
        };

        if matches {
            Ok(embedded_dir)
        } else {
            Err(EngineError::MigrationsDirMismatch(
                self.config.migrations_dir.display().to_string(),
                self.migrations.dir,
            ))
        }
    }

    /// Apply all pending migrations. Returns the number applied. Safe to call on every startup.
    pub async fn migrate(self) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(None, false).await?)
    }

    /// Current configuraiton as seen by the migrator
    pub fn config(&self) -> Config {
        self.config.clone()
    }

    /// Migrate forward or backward to `target` migration id.
    pub async fn migrate_to(self, target: &str) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(Some(target), false).await?)
    }

    /// Mark all pending migrations as applied without running any SQL.
    /// Useful for bootstrapping a database that was set up outside gaman.
    pub async fn fake_migrate(self) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(None, true).await?)
    }

    /// Return true if there are unapplied migrations.
    pub async fn check(self) -> Result<bool, EngineError> {
        Ok(self.build_migrator()?.check().await?)
    }

    /// Return the ordered list of migration ids that would be applied.
    pub async fn plan(self) -> Result<Vec<String>, EngineError> {
        Ok(self.build_migrator()?.plan().await?)
    }

    /// Return all migration ids with their applied/pending status.
    pub async fn show_migrations(self) -> Result<Vec<(String, bool)>, EngineError> {
        Ok(self.build_migrator()?.show_migrations().await?)
    }

    /// Compare the replayed schema against the live database and return any drift operations.
    /// An empty vec means the database is in sync with migrations.
    pub async fn verify(self, schema: &str) -> Result<Vec<Operation>, EngineError> {
        Ok(self.build_migrator()?.verify(schema).await?)
    }

    /// Introspect the live database and return the schema.
    pub async fn inspect_db(self, schemas: &[&str]) -> Result<Schema, EngineError> {
        Ok(self.build_migrator()?.inspect_db(schemas).await?)
    }

    /// Diff the stored schema against the replayed migration state and save a new migration if
    /// there are changes. Returns the migration if one was created, or `None` if the schema is
    /// already up to date.
    ///
    /// Requires `with_schema()` to have been called — returns `Err(EngineError::NoSchema)` if not.
    /// Any rename/ambiguity clarifications are resolved interactively via terminal prompts.
    pub fn make_migration(self, name: &str) -> Result<Option<Migration>, EngineError> {
        let migrations_dir = self.writable_migrations_dir()?;
        let source = Box::new(YamlAdapter {
            directory: migrations_dir,
        });
        let environment = Box::new(EngineEnvironment::new(
            Arc::new(self.config.clone()),
            self.dialect,
        ));
        let migrator = Migrator::new(source, environment)?;
        let schema = self.schema.ok_or(EngineError::NoSchema)?;
        let engine = CliPromptEngine;
        let mut decisions: Vec<Decision> = vec![];
        loop {
            match migrator.make_migrations(
                Some(name.to_string()),
                schema.clone(),
                false,
                &decisions,
            ) {
                Err(MigratorError::NeedsInput(clars)) => {
                    let new = engine
                        .prompt(&clars)
                        .map_err(|e| EngineError::Config(e.to_string()))?;
                    decisions.extend(new);
                }
                Err(e) => return Err(EngineError::Migrator(e)),
                Ok(result) => return Ok(result),
            }
        }
    }

    /// Create an empty migration with no operations. Useful as a shell to fill by hand.
    /// Writes to the embedded source dir after validating it matches config.migrations_dir.
    pub fn make_empty_migration(self, name: &str) -> Result<Migration, EngineError> {
        let migrations_dir = self.writable_migrations_dir()?;
        let source = Box::new(YamlAdapter {
            directory: migrations_dir,
        });
        let environment = Box::new(EngineEnvironment::new(
            Arc::new(self.config.clone()),
            self.dialect,
        ));
        let migrator = Migrator::new(source, environment)?;
        Ok(migrator.make_empty_migration(name.to_string())?)
    }

    /// Parse `std::env::args()` and dispatch the corresponding subcommand using    /// the embedded migration source and the optionally provided schema.
    /// Supports the full CLI interface: make_migration, migrate, verify_db, etc.
    pub async fn handle_args(self) -> Result<(), EngineError> {
        let _ = dotenvy::dotenv();
        let args: GamanArgs = argh::from_env();
        let mut config = self.config;
        let (cmd, cli_dialect) = args.apply_to(&mut config);
        let dialect = parse_dialect(cli_dialect)?.or(self.dialect);
        let config = Arc::new(config);
        let source = Box::new(YamlAdapter {
            directory: config.migrations_dir.clone(),
        });
        let environment = Box::new(EngineEnvironment::new(config, dialect));
        let migrator = Migrator::new(source, environment)?;
        dispatch(migrator, self.schema, cmd).await?;
        Ok(())
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, EngineError> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map_err(|e| EngineError::Config(format!("failed to resolve current directory: {e}")))?
            .join(path)
    };
    Ok(normalize_path(&path))
}

fn normalize_path(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    normalized.push("..");
                }
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHILD_FILES: &[(&str, &str)] = &[(
        "0001_init",
        "id: 0001_init\ndependencies: []\noperations: []\natomic: true\n",
    )];

    const CHILD: EmbeddedMigrations = EmbeddedMigrations {
        files: CHILD_FILES,
        dir: "auth/migrations",
        children: &[],
    };

    const ROOT_FILES: &[(&str, &str)] = &[(
        "0001_initial",
        "id: 0001_initial\ndependencies: []\noperations: []\natomic: true\n",
    )];

    const ROOT: EmbeddedMigrations = EmbeddedMigrations {
        files: ROOT_FILES,
        dir: "migrations",
        children: &[("auth", &CHILD)],
    };

    fn collect_ids(source: &'static EmbeddedMigrations) -> Vec<String> {
        let mut out = Vec::new();
        EmbedSource::collect(source, "", &mut out).unwrap();
        out.into_iter().map(|m| m.id).collect()
    }

    /// Root-only migrations get plain ids with no prefix.
    #[test]
    fn collect_root_only() {
        const SRC: EmbeddedMigrations = EmbeddedMigrations {
            files: ROOT_FILES,
            dir: "migrations",
            children: &[],
        };
        let ids = collect_ids(&SRC);
        assert_eq!(ids, vec!["0001_initial"]);
    }

    /// Child migrations are prefixed with the namespace key.
    #[test]
    fn collect_with_child_prefixes() {
        let ids = collect_ids(&ROOT);
        assert!(ids.contains(&"0001_initial".to_string()), "root id present");
        assert!(
            ids.contains(&"auth/0001_init".to_string()),
            "child id prefixed"
        );
    }

    /// Dependencies within a child are rewritten to include the namespace prefix.
    #[test]
    fn collect_child_deps_rewritten() {
        const DEP_FILES: &[(&str, &str)] = &[
            (
                "0001_a",
                "id: 0001_a\ndependencies: []\noperations: []\natomic: true\n",
            ),
            (
                "0002_b",
                "id: 0002_b\ndependencies: [0001_a]\noperations: []\natomic: true\n",
            ),
        ];
        const DEP_CHILD: EmbeddedMigrations = EmbeddedMigrations {
            files: DEP_FILES,
            dir: "auth/migrations",
            children: &[],
        };
        const WITH_DEPS: EmbeddedMigrations = EmbeddedMigrations {
            files: &[],
            dir: "migrations",
            children: &[("auth", &DEP_CHILD)],
        };
        let mut out = Vec::new();
        EmbedSource::collect(&WITH_DEPS, "", &mut out).unwrap();
        let b = out
            .iter()
            .find(|m| m.id == "auth/0002_b")
            .expect("auth/0002_b present");
        assert_eq!(
            b.dependencies,
            vec!["auth/0001_a"],
            "dep rewritten with namespace"
        );
    }

    /// Already-qualified deps (containing '/') are not double-prefixed.
    #[test]
    fn collect_does_not_double_prefix_qualified_deps() {
        const CROSS_FILES: &[(&str, &str)] = &[(
            "0001_a",
            "id: 0001_a\ndependencies: [other/0001_x]\noperations: []\natomic: true\n",
        )];
        const CROSS_CHILD: EmbeddedMigrations = EmbeddedMigrations {
            files: CROSS_FILES,
            dir: "auth/migrations",
            children: &[],
        };
        const CROSS_ROOT: EmbeddedMigrations = EmbeddedMigrations {
            files: &[],
            dir: "migrations",
            children: &[("auth", &CROSS_CHILD)],
        };
        let mut out = Vec::new();
        EmbedSource::collect(&CROSS_ROOT, "", &mut out).unwrap();
        let m = out.iter().find(|m| m.id == "auth/0001_a").unwrap();
        assert_eq!(
            m.dependencies,
            vec!["other/0001_x"],
            "fully qualified dep not re-prefixed"
        );
    }

    /// EmbeddedMigrations is Copy — can be used in a const context.
    #[test]
    fn embedded_migrations_is_copy() {
        let a = ROOT;
        let b = a;
        assert_eq!(a.dir, b.dir);
    }
}
