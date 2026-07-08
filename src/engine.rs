use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource, YamlAdapter};
use crate::cli::{CommandError, GamanArgs, dispatch};
use crate::conf::Config;
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{BoxFuture, connect_environment_executor};
use crate::migrator::{Migrator, MigratorError};
use crate::prompter::CliPromptEngine;
use crate::schema_file::load_schema_path;
use gaman_core::clarifier::{Clarification, Decision, PromptEngine, clarification_message};
use gaman_core::dialects::Dialect;
use gaman_core::migrations::Migration;
use gaman_core::operations::Operation;
use gaman_core::states::{IntoSchema, Schema, SchemaBuilder, SchemaLoadError};

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
    #[error("clarification needed")]
    NeedsInput(Vec<Clarification>),
    #[error(
        "migrations dir mismatch: config has '{0}', embedded was compiled from '{1}' — they must match for make_migration"
    )]
    MigrationsDirMismatch(String, &'static str),
}

pub struct MigrationEngine {
    config: Config,
    source: EngineSource,
    schema: Option<Schema>,
}

enum EngineSource {
    Embedded {
        migrations: &'static EmbeddedMigrations,
        extra: Vec<(&'static str, &'static EmbeddedMigrations)>,
    },
    Directory,
    Custom(Arc<dyn MigrationSource + Send + Sync>),
}

/// MigrationEngine's native environment adapter for opening live executors.
struct EngineEnvironment {
    config: Arc<Config>,
}

impl EngineEnvironment {
    fn new(config: Arc<Config>) -> Self {
        Self { config }
    }
}

impl Environment for EngineEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        Box::pin(async move {
            let url = self.config.database_url.as_str();
            connect_environment_executor(self.dialect(), url, self.config.tls)
                .await
                .map_err(EnvironmentError::from)
        })
    }

    fn dialect(&self) -> Dialect {
        self.config.dialect
    }
}

impl MigrationEngine {
    pub fn new(config: Config, migrations: &'static EmbeddedMigrations) -> Self {
        Self {
            config,
            source: EngineSource::Embedded {
                migrations,
                extra: Vec::new(),
            },
            schema: None,
        }
    }

    pub(crate) fn from_cli_config(config: Config, schema: Option<Schema>) -> Self {
        Self {
            config,
            source: EngineSource::Directory,
            schema,
        }
    }

    /// Construct an engine over a caller-provided migration source.
    ///
    /// This keeps storage outside the engine: callers can provide filesystem,
    /// in-memory, browser buffer, or other storage implementations.
    pub fn from_source<S: MigrationSource + Send + Sync + 'static>(
        config: Config,
        source: S,
    ) -> Self {
        Self::from_shared_source(config, Arc::new(source))
    }

    /// Construct an engine over a shared caller-provided migration source.
    pub fn from_shared_source(
        config: Config,
        source: Arc<dyn MigrationSource + Send + Sync>,
    ) -> Self {
        Self {
            config,
            source: EngineSource::Custom(source),
            schema: None,
        }
    }

    pub fn add_migrations(mut self, ns: &'static str, m: &'static EmbeddedMigrations) -> Self {
        if let EngineSource::Embedded { extra, .. } = &mut self.source {
            extra.push((ns, m));
        }
        self
    }

    /// Set the target schema.
    ///
    /// The closure receives a `SchemaBuilder` and can return either a `Schema`
    /// directly or a `Result<Schema, SchemaLoadError>`. Calling this more than
    /// once replaces the previous schema.
    pub fn with_schema<R: IntoSchema>(
        mut self,
        f: impl FnOnce(SchemaBuilder) -> R,
    ) -> Result<Self, EngineError> {
        let dialect = self.config.dialect;
        self.schema = Some(
            f(SchemaBuilder::new(dialect))
                .into_schema()?
                .prepare(dialect)
                .map_err(|err| EngineError::Config(err.to_string()))?,
        );
        Ok(self)
    }

    fn build_migrator(&self) -> Result<Migrator, EngineError> {
        let source: Box<dyn MigrationSource + Send + Sync> = match &self.source {
            EngineSource::Embedded { migrations, extra } => {
                Box::new(EmbedSource::new(migrations, extra.clone()))
            }
            EngineSource::Directory => Box::new(YamlAdapter {
                directory: self.config.migrations_dir.clone(),
            }),
            EngineSource::Custom(source) => Box::new(Arc::clone(source)),
        };
        let environment = Box::new(EngineEnvironment::new(Arc::new(self.config.clone())));
        Ok(Migrator::new(source, environment)?)
    }

    fn writable_migrations_dir(&self) -> Result<PathBuf, EngineError> {
        let EngineSource::Embedded { migrations, .. } = &self.source else {
            return Ok(self.config.migrations_dir.clone());
        };
        let embedded_dir = absolute_path(Path::new(migrations.dir))?;
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
                migrations.dir,
            ))
        }
    }

    fn build_writable_migrator(&self) -> Result<Migrator, EngineError> {
        let source: Box<dyn MigrationSource + Send + Sync> = match &self.source {
            EngineSource::Embedded { .. } | EngineSource::Directory => Box::new(YamlAdapter {
                directory: self.writable_migrations_dir()?,
            }),
            EngineSource::Custom(source) => Box::new(Arc::clone(source)),
        };
        let environment = Box::new(EngineEnvironment::new(Arc::new(self.config.clone())));
        Ok(Migrator::new(source, environment)?)
    }

    fn target_schema(&self) -> Result<Schema, EngineError> {
        if let Some(schema) = &self.schema {
            return Ok(schema.clone());
        }
        if matches!(&self.source, EngineSource::Directory) {
            return Ok(load_schema_path(&self.config.schema_file)?);
        }
        Err(EngineError::NoSchema)
    }

    fn ordered_migrations(migrator: &Migrator) -> Result<Vec<Migration>, EngineError> {
        let order = migrator
            .graph
            .topological_order()
            .map_err(MigratorError::Graph)?;
        Ok(order
            .iter()
            .filter_map(|id| migrator.graph.get(id).cloned())
            .collect())
    }

    fn migrations_by_ids(migrator: &Migrator, ids: &[&str]) -> Result<Vec<Migration>, EngineError> {
        if ids.is_empty() {
            return Self::ordered_migrations(migrator);
        }
        let mut migrations = Vec::with_capacity(ids.len());
        for id in ids {
            let migration = migrator
                .graph
                .get(id)
                .cloned()
                .ok_or_else(|| EngineError::Config(format!("unknown migration id '{id}'")))?;
            migrations.push(migration);
        }
        Ok(migrations)
    }

    fn make_migration_inner(
        self,
        name: Option<&str>,
        dry_run: bool,
        clarification_mode: ClarificationMode<'_>,
    ) -> Result<Option<Migration>, EngineError> {
        let migrator = self.build_writable_migrator()?;
        let schema = self.target_schema()?;
        let mut decisions: Vec<Decision> = match clarification_mode {
            ClarificationMode::Decisions(decisions) => decisions.to_vec(),
            ClarificationMode::Interactive | ClarificationMode::Disabled(_) => Vec::new(),
        };
        loop {
            match migrator.make_migrations(
                name.map(str::to_string),
                schema.clone(),
                dry_run,
                &decisions,
            ) {
                Err(MigratorError::NeedsInput(clarifications)) => match clarification_mode {
                    ClarificationMode::Interactive => {
                        let new = CliPromptEngine
                            .prompt(&clarifications)
                            .map_err(|e| EngineError::Config(e.to_string()))?;
                        decisions.extend(new);
                    }
                    ClarificationMode::Disabled(mode) => {
                        return Err(EngineError::Config(clarifications_disabled_message(
                            mode,
                            &clarifications,
                        )));
                    }
                    ClarificationMode::Decisions(_) => {
                        return Err(EngineError::NeedsInput(clarifications));
                    }
                },
                Err(e) => return Err(EngineError::Migrator(e)),
                Ok(result) => return Ok(result),
            }
        }
    }

    /// Apply all pending migrations. Returns the number applied. Safe to call on every startup.
    pub async fn migrate(self) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(None, false).await?)
    }

    /// Current configuration as seen by the migrator.
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

    /// Mark pending migrations through `target` as applied without running SQL.
    pub async fn fake_migrate_to(self, target: &str) -> Result<usize, EngineError> {
        Ok(self.build_migrator()?.migrate(Some(target), true).await?)
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

    /// Introspect the live database and return only `table` from the inspected schemas.
    pub async fn inspect_table(self, schemas: &[&str], table: &str) -> Result<Schema, EngineError> {
        let mut schema = self.inspect_db(schemas).await?;
        schema.tables.retain(|name, _| name == table);
        Ok(schema)
    }

    /// Render SQL for all embedded or file-backed migrations in graph order.
    pub fn sql_migrate(&self) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migrations = Self::ordered_migrations(&migrator)?;
        Ok(migrator.sql_migrate(&migrations)?)
    }

    /// Render SQL for one known migration.
    pub fn sql_migrate_id(&self, id: &str) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migration = Self::migrations_by_ids(&migrator, &[id])?;
        Ok(migrator.sql_migrate(&migration)?)
    }

    /// Render SQL for supplied generated or unsaved migrations against this engine's history.
    pub fn sql_migrate_migrations(
        &self,
        migrations: &[Migration],
    ) -> Result<Vec<String>, EngineError> {
        Ok(self.build_migrator()?.sql_migrate(migrations)?)
    }

    /// Render rollback SQL for known migrations. Passing an empty slice renders all migrations.
    pub fn sql_rollback(&self, ids: &[&str]) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migrations = Self::migrations_by_ids(&migrator, ids)?;
        Ok(migrator.sql_rollback(&migrations)?)
    }

    /// Diff the stored schema against the replayed migration state and save a new migration if
    /// there are changes. Returns the migration if one was created, or `None` if the schema is
    /// already up to date.
    ///
    /// Requires `with_schema()` to have been called — returns `Err(EngineError::NoSchema)` if not.
    /// Any rename/ambiguity clarifications are resolved interactively via terminal prompts.
    pub fn make_migration(self, name: &str) -> Result<Option<Migration>, EngineError> {
        self.make_migration_named(Some(name))
    }

    /// Diff the stored schema against replayed history and save a new migration if changed.
    /// When `name` is `None`, the migration name is derived from the generated operations.
    pub fn make_migration_named(
        self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, false, ClarificationMode::Interactive)
    }

    /// Generate a migration without writing it to disk.
    pub fn make_migration_dry_run(
        self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, true, ClarificationMode::Interactive)
    }

    /// Generate a migration without writing it, failing if clarifications would be required.
    pub fn make_migration_dry_run_non_interactive(
        self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(
            name,
            true,
            ClarificationMode::Disabled("make_migration --dry-run --non-interactive"),
        )
    }

    /// Check whether the configured schema has changes not yet captured in migrations.
    pub fn make_migration_check(self) -> Result<(), EngineError> {
        match self.make_migration_inner(
            Some("check"),
            true,
            ClarificationMode::Disabled("make_migration --check"),
        )? {
            Some(_) => Err(EngineError::Config(
                "schema has changes not yet in a migration".into(),
            )),
            None => Ok(()),
        }
    }

    /// Generate and write a migration, failing if clarifications would be required.
    pub fn make_migration_non_interactive(
        self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(
            name,
            false,
            ClarificationMode::Disabled("make_migration --non-interactive"),
        )
    }

    /// Generate and write a migration using caller-provided clarification decisions.
    pub fn make_migration_with_decisions(
        self,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, false, ClarificationMode::Decisions(decisions))
    }

    /// Create an empty migration with no operations. Useful as a shell to fill by hand.
    /// Writes to the embedded source dir after validating it matches config.migrations_dir.
    pub fn make_empty_migration(self, name: &str) -> Result<Migration, EngineError> {
        Ok(self
            .build_writable_migrator()?
            .make_empty_migration(name.to_string())?)
    }

    /// Create a merge migration for multiple graph heads.
    pub fn make_merge_migration(self, name: &str) -> Result<Migration, EngineError> {
        Ok(self
            .build_writable_migrator()?
            .make_merge_migration(name.to_string())?)
    }

    /// Parse `std::env::args()` and dispatch the corresponding subcommand using
    /// the embedded migration source and the optionally provided schema.
    /// Supports the full CLI interface: make_migration, migrate, verify_db, etc.
    pub async fn handle_args(self) -> Result<(), EngineError> {
        let _ = dotenvy::dotenv();
        let args: GamanArgs = argh::from_env();
        let mut config = self.config;
        let cmd = args.apply_to(&mut config)?;
        let engine = MigrationEngine::from_cli_config(config, self.schema);
        dispatch(engine, cmd).await?;
        Ok(())
    }
}

enum ClarificationMode<'a> {
    Interactive,
    Disabled(&'static str),
    Decisions(&'a [Decision]),
}

pub(crate) fn clarifications_disabled_message(
    mode: &str,
    clarifications: &[Clarification],
) -> String {
    let mut message = format!(
        "{mode} requires {} clarification(s), but prompts are disabled",
        clarifications.len()
    );
    for clarification in clarifications {
        let prompt = clarification_message(clarification);
        message.push_str(&format!(
            "\n  - {}: {}",
            clarification.id, prompt.description
        ));
    }
    message
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::adapters::MigrationSource;

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

    const SQL_FILES: &[(&str, &str)] = &[
        (
            "0001_create_users",
            r#"
id: ignored
dependencies: []
operations:
  - type: create_table
    table:
      name: users
      columns:
        - name: id
          type: integer
          primary_key: true
        - name: username
          type: text
atomic: true
"#,
        ),
        (
            "0002_add_email",
            r#"
id: ignored
dependencies: [0001_create_users]
operations:
  - type: add_column
    table_name: users
    column:
      name: email
      type: text
      nullable: true
atomic: true
"#,
        ),
    ];

    const SQL_MIGRATIONS: EmbeddedMigrations = EmbeddedMigrations {
        files: SQL_FILES,
        dir: "migrations",
        children: &[],
    };

    #[derive(Clone, Default)]
    struct MemorySource {
        migrations: Arc<Mutex<Vec<Migration>>>,
    }

    impl MigrationSource for MemorySource {
        fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
            Ok(self.migrations.lock().unwrap().clone())
        }

        fn save(&self, migration: &Migration) -> Result<(), AdapterError> {
            self.migrations.lock().unwrap().push(migration.clone());
            Ok(())
        }
    }

    fn collect_ids(source: &'static EmbeddedMigrations) -> Vec<String> {
        let mut out = Vec::new();
        EmbedSource::collect(source, "", &mut out).unwrap();
        out.into_iter().map(|m| m.id).collect()
    }

    fn directory_engine(dir: &tempfile::TempDir, schema: Schema) -> MigrationEngine {
        let config = Config {
            migrations_dir: dir.path().join("migrations"),
            ..Config::default()
        };
        MigrationEngine::from_cli_config(config, Some(schema))
    }

    struct HandWrittenUser;

    impl crate::schema::IntoTable for HandWrittenUser {
        fn into_table(dialect: &Dialect) -> crate::schema::Table {
            crate::schema::TableBuilder::new("users")
                .column_from_type::<i64>(dialect, "id", |c| c.primary_key())
                .column_from_type::<String>(dialect, "email", |c| c.not_null())
                .unique_columns(&["email"])
                .build()
        }
    }

    /// Verifies public `IntoTable` trait implementations still feed `MigrationEngine`.
    #[test]
    fn hand_written_into_table_builds_engine_schema() {
        let dir = tempfile::tempdir().unwrap();
        let schema = crate::schema::Schema::builder(Dialect::Postgres)
            .table::<HandWrittenUser>()
            .build();

        let migration = directory_engine(&dir, schema)
            .make_migration_dry_run(Some("add_users"))
            .unwrap()
            .expect("migration");

        assert_eq!(migration.id, "0001_add_users");
        assert_eq!(migration.operations.len(), 1);
    }

    /// Renders SQL from caller-provided migration storage.
    #[test]
    fn from_source_renders_sql_from_custom_storage() {
        let source = MemorySource::default();
        source
            .save(
                &serde_yaml::from_str(
                    r#"
id: 0001_create_users
dependencies: []
operations:
  - type: create_table
    table:
      name: users
      columns:
        - name: id
          type: integer
          primary_key: true
atomic: true
"#,
                )
                .unwrap(),
            )
            .unwrap();

        let sql = MigrationEngine::from_source(Config::default(), source)
            .sql_migrate()
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("CREATE TABLE"));
    }

    /// Saves generated migrations through caller-provided migration storage.
    #[test]
    fn from_source_writes_generated_migration_to_custom_storage() {
        let source = MemorySource::default();
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: id
        type: integer
"#,
        )
        .unwrap();
        let engine = MigrationEngine::from_source(
            Config::default().with_dialect(Dialect::Postgres),
            source.clone(),
        )
        .with_schema(|_| schema)
        .unwrap();

        let migration = engine
            .make_migration_non_interactive(Some("add_users"))
            .unwrap()
            .expect("migration");

        let saved = source.load_all().unwrap();
        assert_eq!(migration.id, "0001_add_users");
        assert_eq!(saved.len(), 1);
        assert_eq!(saved[0].id, "0001_add_users");
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

    /// Renders all embedded migrations through the public engine SQL API.
    #[test]
    fn sql_migrate_renders_all_embedded_migrations() {
        let sql = MigrationEngine::new(Config::default(), &SQL_MIGRATIONS)
            .sql_migrate()
            .unwrap();

        assert_eq!(sql.len(), 2);
        assert!(sql[0].contains("CREATE TABLE"));
        assert!(sql[1].contains("ADD COLUMN"));
    }

    /// Renders one selected embedded migration with replayed dependency state.
    #[test]
    fn sql_migrate_id_renders_one_migration() {
        let sql = MigrationEngine::new(Config::default(), &SQL_MIGRATIONS)
            .sql_migrate_id("0002_add_email")
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("ADD COLUMN"));
    }

    /// Reports an unknown migration id from the public engine SQL API.
    #[test]
    fn sql_migrate_id_rejects_unknown_id() {
        let err = MigrationEngine::new(Config::default(), &SQL_MIGRATIONS)
            .sql_migrate_id("missing")
            .unwrap_err();

        assert!(err.to_string().contains("unknown migration id 'missing'"));
    }

    /// Renders supplied generated migrations against embedded dependency history.
    #[test]
    fn sql_migrate_migrations_uses_embedded_baseline() {
        let generated: Migration = serde_yaml::from_str(
            r#"
id: 0003_add_nickname
dependencies: [0001_create_users]
operations:
  - type: add_column
    table_name: users
    column:
      name: nickname
      type: text
      nullable: true
atomic: true
"#,
        )
        .unwrap();

        let sql = MigrationEngine::new(Config::default(), &SQL_MIGRATIONS)
            .sql_migrate_migrations(&[generated])
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("nickname"));
    }

    /// Renders rollback SQL through the public engine rollback API.
    #[test]
    fn sql_rollback_renders_known_migration() {
        let sql = MigrationEngine::new(Config::default(), &SQL_MIGRATIONS)
            .sql_rollback(&["0002_add_email"])
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("DROP COLUMN"));
    }

    /// Verifies dry-run migration generation returns a migration without writing a file.
    #[test]
    fn make_migration_dry_run_does_not_write_file() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: id
        type: integer
"#,
        )
        .unwrap();

        let migration = directory_engine(&dir, schema)
            .make_migration_dry_run(Some("add_users"))
            .unwrap()
            .expect("migration");

        assert_eq!(migration.id, "0001_add_users");
        assert!(!dir.path().join("migrations/0001_add_users.yaml").exists());
    }

    /// Verifies migration check succeeds when schema and replayed state match.
    #[test]
    fn make_migration_check_succeeds_without_changes() {
        let dir = tempfile::tempdir().unwrap();
        directory_engine(&dir, Schema::default())
            .make_migration_check()
            .unwrap();
    }

    /// Verifies migration check fails when a migration would be generated.
    #[test]
    fn make_migration_check_fails_with_pending_changes() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: id
        type: integer
"#,
        )
        .unwrap();

        let err = directory_engine(&dir, schema)
            .make_migration_check()
            .unwrap_err();

        assert!(err.to_string().contains("schema has changes"));
    }

    /// Verifies non-interactive migration generation reports clarification needs clearly.
    #[test]
    fn make_migration_non_interactive_fails_on_clarification() {
        let dir = tempfile::tempdir().unwrap();
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: code
        type: project_code
"#,
        )
        .unwrap();

        let err = directory_engine(&dir, schema)
            .make_migration_non_interactive(Some("add_users"))
            .unwrap_err();

        assert!(err.to_string().contains("--non-interactive"));
        assert!(err.to_string().contains("project_code"));
    }

    /// Verifies caller-provided decisions can resolve public engine migration generation.
    #[test]
    fn make_migration_with_decisions_uses_caller_answers() {
        use gaman_core::clarifier::Answer;

        let dir = tempfile::tempdir().unwrap();
        let schema = Schema::from_yaml_str(
            r#"
tables:
  users:
    columns:
      - name: code
        type: project_code
"#,
        )
        .unwrap();
        let decisions = vec![Decision {
            clarification_id: "unknown_type:users:code".to_string(),
            answer: Answer::KeepType,
        }];

        let migration = directory_engine(&dir, schema)
            .make_migration_with_decisions(Some("add_users"), &decisions)
            .unwrap()
            .expect("migration");

        assert_eq!(migration.id, "0001_add_users");
        assert!(dir.path().join("migrations/0001_add_users.yaml").exists());
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
