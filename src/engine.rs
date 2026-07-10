use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource, YamlAdapter};
use crate::conf::Config;
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{BoxFuture, connect_environment_executor};
use crate::migrator::{
    MigrationArtifact, MigrationListing, MigrationMovement, Migrator, MigratorError, RepairOptions,
    RepairReport,
};
use crate::schema_check::{SchemaCheckReport, SqlSchemaInput, check_sql_schema_with_executor};
use crate::schema_file::load_schema_path;
use gaman_core::clarifier::{Clarification, Decision};
use gaman_core::dialects::Dialect;
use gaman_core::migrations::Migration;
use gaman_core::operations::Operation;
use gaman_core::states::{Schema, SchemaBuilder, SchemaLoadError};

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
    #[error("migration error: {0}")]
    Migrator(#[from] MigratorError),
    #[error("database connection failed: {0}")]
    Connect(String),
    #[error("migration source error: {0}")]
    Adapter(#[from] AdapterError),
    #[error("{0}")]
    Config(String),
    #[error("no schema set — call with_schema() before make()")]
    NoSchema,
    #[error("schema load error: {0}")]
    SchemaLoad(#[source] Box<SchemaLoadError>),
    #[error("clarification needed")]
    NeedsInput(Vec<Clarification>),
    #[error("unknown inspected table '{0}'")]
    UnknownInspectedTable(String),
    #[error("inspected table name '{table}' is ambiguous: {matches}")]
    AmbiguousInspectedTable { table: String, matches: String },
    #[error(
        "migrations dir mismatch: config has '{0}', embedded was compiled from '{1}' — they must match for make"
    )]
    MigrationsDirMismatch(String, &'static str),
}

impl From<SchemaLoadError> for EngineError {
    fn from(error: SchemaLoadError) -> Self {
        Self::SchemaLoad(Box::new(error))
    }
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

    /// Constructs an engine backed by the configured migration directory.
    pub fn from_directory(config: Config) -> Self {
        Self {
            config,
            source: EngineSource::Directory,
            schema: None,
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
    /// The closure configures a dialect-aware builder. The engine performs the
    /// single authoritative build and preparation step after it returns.
    pub fn with_schema(
        mut self,
        configure: impl FnOnce(SchemaBuilder) -> SchemaBuilder,
    ) -> Result<Self, EngineError> {
        let dialect = self.config.dialect;
        self.schema = Some(configure(SchemaBuilder::new(dialect)).build()?);
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
            return Ok(load_schema_path(
                &self.config.schema_file,
                self.config.dialect,
            )?);
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

    /// Plans one migration generation attempt using caller-provided clarification decisions.
    fn make_migration_inner(
        &self,
        name: Option<&str>,
        dry_run: bool,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        let migrator = self.build_writable_migrator()?;
        let schema = self.target_schema()?;
        match migrator.make_migrations(name.map(str::to_string), schema, dry_run, decisions) {
            Err(MigratorError::NeedsInput(clarifications)) => {
                Err(EngineError::NeedsInput(clarifications))
            }
            Err(error) => Err(EngineError::Migrator(error)),
            Ok(result) => Ok(result),
        }
    }

    /// Apply all pending migrations. Safe to call on every startup.
    pub async fn apply(self) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.apply(None, false).await?)
    }

    /// Current configuration as seen by the migrator.
    pub fn config(&self) -> Config {
        self.config.clone()
    }

    /// Prepares each supplied SQL schema statement against the configured database.
    ///
    /// This validates database syntax without executing SQL, reading migration
    /// storage, installing tracking state, acquiring locks, or starting a
    /// transaction. The caller supplies in-memory source so the engine remains
    /// independent of filesystem and browser storage choices.
    pub async fn check_sql_schema(
        &self,
        files: impl IntoIterator<Item = SqlSchemaInput>,
    ) -> Result<SchemaCheckReport, EngineError> {
        let files = files.into_iter().collect::<Vec<_>>();
        if files.is_empty() {
            return Ok(SchemaCheckReport::default());
        }
        let environment = EngineEnvironment::new(Arc::new(self.config.clone()));
        let mut executor = environment.executor().await.map_err(|error| match error {
            EnvironmentError::Config(message) => EngineError::Config(message),
            EnvironmentError::Connect(message) => EngineError::Connect(message),
        })?;
        Ok(check_sql_schema_with_executor(&mut *executor, self.config.dialect, files).await)
    }

    /// Migrate forward or backward to `target` migration id.
    pub async fn apply_to(self, target: &str) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.apply(Some(target), false).await?)
    }

    /// Roll back applied migrations until `target` is the latest applied migration.
    pub async fn rollback_to(self, target: &str) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.rollback_to(target, false).await?)
    }

    /// Mark all pending migrations as applied without running any SQL.
    /// Useful for bootstrapping a database that was set up outside gaman.
    pub async fn fake_apply(self) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.apply(None, true).await?)
    }

    /// Mark pending migrations through `target` as applied without running SQL.
    pub async fn fake_apply_to(self, target: &str) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.apply(Some(target), true).await?)
    }

    /// Mark rollback migrations through `target` as reverted without running SQL.
    pub async fn fake_rollback_to(self, target: &str) -> Result<MigrationMovement, EngineError> {
        Ok(self.build_migrator()?.rollback_to(target, true).await?)
    }

    /// Return true if there are unapplied migrations.
    pub async fn check(self) -> Result<bool, EngineError> {
        Ok(self.build_migrator()?.check().await?)
    }

    /// Return the ordered list of migration ids that would be applied.
    pub async fn plan(self) -> Result<Vec<String>, EngineError> {
        Ok(self.build_migrator()?.plan().await?)
    }

    /// Returns all migration ids with their applied or pending status.
    pub async fn status(self) -> Result<Vec<(String, bool)>, EngineError> {
        Ok(self.build_migrator()?.status().await?)
    }

    /// Returns canonical migration artifacts without opening a database connection.
    pub fn show(&self) -> Result<Vec<MigrationArtifact>, EngineError> {
        Ok(self.build_migrator()?.artifacts()?)
    }

    /// Returns live migration listings with application status and canonical content.
    pub async fn status_listings(&self) -> Result<Vec<MigrationListing>, EngineError> {
        Ok(self.build_migrator()?.status_listings().await?)
    }

    /// Resolves an exact migration id or a unique migration id prefix.
    pub fn resolve_migration_id(&self, input: &str) -> Result<String, EngineError> {
        self.build_migrator()?
            .graph
            .resolve_id(input)
            .map_err(MigratorError::Graph)
            .map_err(EngineError::from)
    }

    /// Compare the replayed schema against the live database and return any drift operations.
    /// An empty vec means the database is in sync with migrations.
    pub async fn verify(self, schema: &str) -> Result<Vec<Operation>, EngineError> {
        Ok(self.build_migrator()?.verify(schema).await?)
    }

    /// Compare replayed schema against the live database and return detailed drift findings.
    pub async fn verify_report(
        self,
        schema: &str,
    ) -> Result<gaman_core::drift::VerificationReport, EngineError> {
        self.verify_report_schemas(&[schema]).await
    }

    /// Compares replayed state against one or more live schemas.
    pub async fn verify_report_schemas(
        self,
        schemas: &[&str],
    ) -> Result<gaman_core::drift::VerificationReport, EngineError> {
        Ok(self
            .build_migrator()?
            .verify_report_schemas(schemas)
            .await?)
    }

    /// Plan or apply one-off SQL that repairs verified drift without writing migrations.
    pub async fn repair(self, options: RepairOptions) -> Result<RepairReport, EngineError> {
        self.repair_schemas(&["public"], options).await
    }

    /// Plans or applies one-off drift repair across one or more schemas.
    pub async fn repair_schemas(
        self,
        schemas: &[&str],
        options: RepairOptions,
    ) -> Result<RepairReport, EngineError> {
        Ok(self
            .build_migrator()?
            .repair_schemas(schemas, options)
            .await?)
    }

    /// Introspect the live database and return the schema.
    pub async fn inspect(self, schemas: &[&str]) -> Result<Schema, EngineError> {
        Ok(self.build_migrator()?.inspect(schemas).await?)
    }

    /// Introspect the live database and return only `table` from the inspected schemas.
    pub async fn inspect_table(self, schemas: &[&str], table: &str) -> Result<Schema, EngineError> {
        select_inspected_table(self.inspect(schemas).await?, table)
    }

    /// Render SQL for all embedded or file-backed migrations in graph order.
    pub fn sql(&self) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migrations = Self::ordered_migrations(&migrator)?;
        Ok(migrator.sql_migrate(&migrations)?)
    }

    /// Render SQL for one known migration.
    pub fn sql_id(&self, id: &str) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migration = Self::migrations_by_ids(&migrator, &[id])?;
        Ok(migrator.sql_migrate(&migration)?)
    }

    /// Render SQL for supplied generated or unsaved migrations against this engine's history.
    pub(crate) fn sql_migrate_migrations(
        &self,
        migrations: &[Migration],
    ) -> Result<Vec<String>, EngineError> {
        Ok(self.build_migrator()?.sql_migrate(migrations)?)
    }

    /// Render SQL for supplied generated or unsaved migrations against history.
    pub fn sql_migrations(&self, migrations: &[Migration]) -> Result<Vec<String>, EngineError> {
        self.sql_migrate_migrations(migrations)
    }

    /// Render rollback SQL for known migrations. Passing an empty slice renders all migrations.
    pub fn sql_rollback(&self, ids: &[&str]) -> Result<Vec<String>, EngineError> {
        let migrator = self.build_migrator()?;
        let migrations = Self::migrations_by_ids(&migrator, ids)?;
        Ok(migrator.sql_rollback(&migrations)?)
    }

    /// Generates a migration without writing it, using caller-provided clarification decisions.
    pub fn make_dry_run_with_decisions(
        &self,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, true, decisions)
    }

    /// Generates a migration without writing it, failing if clarification is needed.
    pub fn make_dry_run_non_interactive(
        &self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, true, &[])
    }

    /// Generates a migration without writing it and returns structured clarification needs.
    pub fn make_dry_run(&self, name: Option<&str>) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, true, &[])
    }

    /// Check whether the configured schema has changes not yet captured in migrations.
    pub fn make_check(&self) -> Result<(), EngineError> {
        match self.make_migration_inner(Some("check"), true, &[])? {
            Some(_) => Err(EngineError::Config(
                "schema has changes not yet in a migration".into(),
            )),
            None => Ok(()),
        }
    }

    /// Generate and write a migration, failing if clarifications would be required.
    pub fn make_non_interactive(
        &self,
        name: Option<&str>,
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, false, &[])
    }

    /// Generate and write a migration using caller-provided clarification decisions.
    pub fn make_with_decisions(
        &self,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_migration_inner(name, false, decisions)
    }

    /// Create an empty migration with no operations. Useful as a shell to fill by hand.
    /// Writes to the embedded source dir after validating it matches config.migrations_dir.
    pub(crate) fn make_empty_migration(&self, name: &str) -> Result<Migration, EngineError> {
        Ok(self
            .build_writable_migrator()?
            .make_empty_migration(name.to_string())?)
    }

    /// Create an empty migration with no operations.
    pub fn make_empty(&self, name: &str) -> Result<Migration, EngineError> {
        self.make_empty_migration(name)
    }

    /// Create a merge migration for multiple graph heads.
    pub(crate) fn make_merge_migration(&self, name: &str) -> Result<Migration, EngineError> {
        Ok(self
            .build_writable_migrator()?
            .make_merge_migration(name.to_string())?)
    }

    /// Create a merge migration for multiple graph heads.
    pub fn make_merge(&self, name: &str) -> Result<Migration, EngineError> {
        self.make_merge_migration(name)
    }
}

/// Selects an exact qualified table or one unambiguous bare table name.
fn select_inspected_table(mut schema: Schema, input: &str) -> Result<Schema, EngineError> {
    let selected = if schema.tables.contains_key(input) {
        input.to_string()
    } else {
        let matches: Vec<String> = schema
            .tables
            .iter()
            .filter(|(_, table)| table.name == input)
            .map(|(name, _)| name.clone())
            .collect();
        match matches.as_slice() {
            [] => return Err(EngineError::UnknownInspectedTable(input.to_string())),
            [name] => name.clone(),
            _ => {
                return Err(EngineError::AmbiguousInspectedTable {
                    table: input.to_string(),
                    matches: matches.join(", "),
                });
            }
        }
    };
    schema.tables.retain(|name, _| name == &selected);
    Ok(schema)
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

    fn test_config() -> Config {
        Config::new(
            "postgres:///".to_string(),
            "migrations".into(),
            "schema.yaml".into(),
            Dialect::Postgres,
        )
    }

    fn directory_engine(dir: &tempfile::TempDir, schema: Schema) -> MigrationEngine {
        let config = Config {
            migrations_dir: dir.path().join("migrations"),
            ..test_config()
        };
        MigrationEngine {
            config,
            source: EngineSource::Directory,
            schema: Some(schema),
        }
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
            .build()
            .unwrap();

        let migration = directory_engine(&dir, schema)
            .make_dry_run(Some("add_users"))
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

        let sql = MigrationEngine::from_source(test_config(), source)
            .sql()
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
            Dialect::Postgres,
        )
        .unwrap();
        let engine = MigrationEngine::from_source(test_config(), source.clone())
            .with_schema(|builder| {
                schema
                    .tables
                    .into_values()
                    .fold(builder, SchemaBuilder::table_def)
            })
            .unwrap();

        let migration = engine
            .make_non_interactive(Some("add_users"))
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
        let sql = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
            .sql()
            .unwrap();

        assert_eq!(sql.len(), 2);
        assert!(sql[0].contains("CREATE TABLE"));
        assert!(sql[1].contains("ADD COLUMN"));
    }

    /// Verifies artifact inspection uses the migration graph without opening an executor.
    #[test]
    fn show_returns_embedded_artifacts_offline() {
        let artifacts = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
            .show()
            .unwrap();

        assert_eq!(artifacts.len(), 2);
        assert_eq!(artifacts[0].id, "0001_create_users");
        assert!(!artifacts[0].content.is_empty());
    }

    /// Selects a custom-schema table by its unambiguous bare name.
    #[test]
    fn inspected_table_selection_resolves_unique_bare_name() {
        let table = crate::schema::TableBuilder::new("users")
            .schema("billing")
            .column("id", "integer", |column| column)
            .build();
        let mut schema = Schema::default();
        schema.tables.insert("billing.users".to_string(), table);

        let selected = select_inspected_table(schema, "users").unwrap();

        assert!(selected.tables.contains_key("billing.users"));
    }

    /// Rejects an ambiguous bare table name across inspected schemas.
    #[test]
    fn inspected_table_selection_rejects_ambiguous_bare_name() {
        let mut schema = Schema::default();
        for namespace in ["billing", "auth"] {
            let table = crate::schema::TableBuilder::new("users")
                .schema(namespace)
                .column("id", "integer", |column| column)
                .build();
            schema.tables.insert(format!("{namespace}.users"), table);
        }

        let error = select_inspected_table(schema, "users").unwrap_err();

        assert!(matches!(error, EngineError::AmbiguousInspectedTable { .. }));
    }

    /// Rejects a table selector that is absent from inspected state.
    #[test]
    fn inspected_table_selection_rejects_unknown_name() {
        let error = select_inspected_table(Schema::default(), "missing").unwrap_err();

        assert!(matches!(error, EngineError::UnknownInspectedTable(name) if name == "missing"));
    }

    /// Renders one selected embedded migration with replayed dependency state.
    #[test]
    fn sql_migrate_id_renders_one_migration() {
        let sql = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
            .sql_id("0002_add_email")
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("ADD COLUMN"));
    }

    /// Reports an unknown migration id from the public engine SQL API.
    #[test]
    fn sql_migrate_id_rejects_unknown_id() {
        let err = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
            .sql_id("missing")
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

        let sql = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
            .sql_migrations(&[generated])
            .unwrap();

        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("nickname"));
    }

    /// Renders rollback SQL through the public engine rollback API.
    #[test]
    fn sql_rollback_renders_known_migration() {
        let sql = MigrationEngine::new(test_config(), &SQL_MIGRATIONS)
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
            Dialect::Postgres,
        )
        .unwrap();

        let migration = directory_engine(&dir, schema)
            .make_dry_run(Some("add_users"))
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
            .make_check()
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
            Dialect::Postgres,
        )
        .unwrap();

        let err = directory_engine(&dir, schema).make_check().unwrap_err();

        assert!(err.to_string().contains("schema has changes"));
    }

    /// Verifies non-interactive migration generation returns structured clarification needs.
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
            Dialect::Postgres,
        )
        .unwrap();

        let err = directory_engine(&dir, schema)
            .make_non_interactive(Some("add_users"))
            .unwrap_err();

        assert!(matches!(
            err,
            EngineError::NeedsInput(clarifications)
                if clarifications.iter().any(|clarification| clarification.id == "unknown_type:users:code")
        ));
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
            Dialect::Postgres,
        )
        .unwrap();
        let decisions = vec![Decision {
            clarification_id: "unknown_type:users:code".to_string(),
            answer: Answer::KeepType,
        }];

        let migration = directory_engine(&dir, schema)
            .make_with_decisions(Some("add_users"), &decisions)
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
