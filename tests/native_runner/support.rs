//! Isolated database setup and assertions shared only by native runner tests.

use std::future::Future;

use gaman::runner_factory::{LazyExecutor, NativeMigrationStore, NativeRunnerFactory};
use gaman::{Config, Migration};
use gaman_core::{
    ApplyCommand, Command, CommandError, CommandResult, DatabaseTrackingStore, Dialect,
    EmbeddedMigrations, EngineError, ExecutorError, MigrationRunner, MigrationStore,
};
use sqlx::postgres::{PgConnectOptions, PgSslMode};
use sqlx::{ConnectOptions, PgConnection};

pub(super) type Runner = MigrationRunner<NativeMigrationStore, DatabaseTrackingStore, LazyExecutor>;

pub(super) static ITEMS: EmbeddedMigrations = EmbeddedMigrations {
    dir: concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/native_runner/fixtures/items"
    ),
    files: &[
        (
            "0001_items.yaml",
            include_str!("fixtures/items/0001_items.yaml"),
        ),
        (
            "0002_insert.yaml",
            include_str!("fixtures/items/0002_insert.yaml"),
        ),
        (
            "0003_update.yaml",
            include_str!("fixtures/items/0003_update.yaml"),
        ),
        (
            "0004_delete.yaml",
            include_str!("fixtures/items/0004_delete.yaml"),
        ),
    ],
    children: &[],
};

pub(super) static PROFILES: EmbeddedMigrations = EmbeddedMigrations {
    dir: concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/native_runner/fixtures/profiles"
    ),
    files: &[
        (
            "0007_personas.yaml",
            include_str!("fixtures/profiles/0007_personas.yaml"),
        ),
        (
            "0008_model_profiles.yaml",
            include_str!("fixtures/profiles/0008_model_profiles.yaml"),
        ),
    ],
    children: &[],
};

/// Runs a scenario in a disposable database and cleans it up even after assertion failure.
pub(super) async fn with_database<F, Fut>(scenario: F)
where
    F: FnOnce(Config, PgConnection) -> Fut + 'static,
    Fut: Future<Output = ()> + 'static,
{
    let url = std::env::var("GAMAN_NATIVE_TEST_ADMIN_URL")
        .expect("set GAMAN_NATIVE_TEST_ADMIN_URL from a dedicated dbharness environment");
    let options: PgConnectOptions = url.parse().expect("parse dbharness admin URL");
    assert_eq!(options.get_host(), "127.0.0.1", "use loopback dbharness");
    let options = options.ssl_mode(PgSslMode::Disable);
    let directory = temporary_directory().await;
    let database = directory
        .path()
        .file_name()
        .expect("directory name")
        .to_str()
        .expect("ASCII database name")
        .to_owned();
    let mut admin = options.connect().await.expect("connect to dbharness admin");
    sql(&mut admin, &format!("CREATE DATABASE \"{database}\"")).await;
    let isolated = options.database(&database);
    let config = Config::new(
        isolated.to_url_lossy().to_string(),
        directory.path().join("migrations"),
        directory.path().join("schema.yaml"),
        Dialect::Postgres,
    );
    let outcome = tokio::task::LocalSet::new()
        .run_until(async move {
            tokio::task::spawn_local(async move {
                let observer = isolated.connect().await.expect("connect isolated observer");
                scenario(config, observer).await;
            })
            .await
        })
        .await;
    sql(
        &mut admin,
        &format!("DROP DATABASE \"{database}\" WITH (FORCE)"),
    )
    .await;
    tokio::task::spawn_blocking(move || directory.close())
        .await
        .expect("directory cleanup task")
        .expect("directory cleanup");
    assert!(
        outcome.is_ok(),
        "native runner scenario failed: {outcome:?}"
    );
}

/// Creates fixture storage without blocking the async test runtime on filesystem work.
async fn temporary_directory() -> tempfile::TempDir {
    tokio::task::spawn_blocking(|| tempfile::Builder::new().prefix("gaman_native_").tempdir())
        .await
        .expect("temporary directory task")
        .expect("temporary directory")
}

/// Executes setup or deliberately induced drift only inside the isolated test database.
pub(super) async fn sql(connection: &mut PgConnection, statement: &str) {
    sqlx::raw_sql(statement)
        .execute(connection)
        .await
        .expect("fixture SQL");
}

/// Asserts externally observed text values without accessing the runner's connection.
pub(super) async fn rows(connection: &mut PgConnection, query: &str, expected: &[&str]) {
    let actual = sqlx::query_scalar::<_, String>(query)
        .fetch_all(connection)
        .await
        .expect("observe fixture data");
    assert_eq!(actual, expected, "{query}");
}

/// Asserts the durable tracking table contains exactly the expected migration IDs.
pub(super) async fn records(connection: &mut PgConnection, expected: &[&str]) {
    rows(
        connection,
        "SELECT id FROM gaman_migrations ORDER BY id",
        expected,
    )
    .await;
}

/// Loads one shared fixture as the same typed migration consumed by application hosts.
pub(super) fn fixture(tree: &EmbeddedMigrations, index: usize) -> Migration {
    let (filename, content) = tree.files[index];
    let mut migration = Migration::from_yaml_str(content).expect("parse migration fixture");
    migration.id = filename
        .strip_suffix(".yaml")
        .expect("YAML fixture filename")
        .into();
    migration
}

/// Saves through the native migration store rather than writing migration files directly.
pub(super) async fn save(config: &Config, migration: &Migration) {
    gaman::runner_factory::DirectoryMigrationStore::new(&config.migrations_dir)
        .save(migration)
        .await
        .expect("save fixture migration");
}

/// Builds the real directory-backed factory with the requested prefix of fixture history.
pub(super) async fn directory(config: &Config, tree: &EmbeddedMigrations, count: usize) -> Runner {
    for index in 0..count {
        save(config, &fixture(tree, index)).await;
    }
    NativeRunnerFactory::from_directory(config.clone()).build()
}

/// Builds the real embedded factory using immutable shared migration assets.
pub(super) fn embedded(mut config: Config, tree: &'static EmbeddedMigrations) -> Runner {
    config.migrations_dir = tree.dir.into();
    NativeRunnerFactory::from_embedded(config, tree)
        .expect("embedded factory")
        .build()
}

/// Runs real forward or reverse movement, with fake application disabled.
pub(super) async fn apply(
    runner: &mut Runner,
    target: &str,
) -> Result<CommandResult, CommandError> {
    runner
        .run_command(&Command::Apply(ApplyCommand::Execute {
            target: Some(target.to_owned()),
            fake: false,
            fake_verified: false,
            schemas: vec![],
        }))
        .await
}

/// Asserts migration, direction, ordinal, and SQL context while preserving the typed cause.
pub(super) fn execution<'a>(
    error: &'a CommandError,
    id: &str,
    direction: &str,
    ordinal: usize,
    signature: &str,
) -> &'a ExecutorError {
    let CommandError::Migration(EngineError::MigrationExecution {
        migration,
        direction: actual_direction,
        statement_ordinal,
        statement,
        source,
    }) = error
    else {
        panic!("expected structured migration failure: {error:?}")
    };
    assert_eq!(migration, id);
    assert_eq!(*actual_direction, direction);
    assert_eq!(*statement_ordinal, ordinal);
    assert!(statement.signature.starts_with(signature), "{statement:?}");
    source
}

/// Verifies complete owned schema and row state through native inspection and row queries.
pub(super) async fn verified(runner: &mut Runner) {
    let result = runner
        .run_command(&Command::Verify {
            schemas: vec!["public".into()],
        })
        .await
        .expect("verify through native factory");
    let CommandResult::Verify(report) = result else {
        panic!("expected verification report")
    };
    assert!(report.findings.is_empty(), "{report:?}");
    assert!(report.pending_migrations.is_empty(), "{report:?}");
}

/// Proves a failed migration released its lock while its original connection is still open.
pub(super) async fn lock_released(config: &Config, target: &str) {
    let mut fresh = NativeRunnerFactory::from_directory(config.clone()).build();
    tokio::time::timeout(std::time::Duration::from_secs(5), apply(&mut fresh, target))
        .await
        .expect("migration lock must be released")
        .expect("fresh runner can apply");
}
