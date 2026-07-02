use std::sync::{Arc, Mutex};

use super::*;
use crate::adapters::AdapterError;
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{BoxFuture, Introspectable};
use gaman_core::dialects::Dialect;
use gaman_core::operations::Operation;
use gaman_core::states::{
    Column, EnumDef, ExtensionDef, ForeignKey, FunctionDef, PrimaryKey, Schema, Table, TriggerDef,
    TriggerEvent, TriggerScope, TriggerTiming, Volatility,
};

#[derive(Default)]
struct MockSource {
    saved: Mutex<Vec<Migration>>,
    migrations: Vec<Migration>,
}

impl MigrationSource for MockSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        Ok(self.migrations.clone())
    }
    fn save(&self, m: &Migration) -> Result<(), AdapterError> {
        self.saved
            .lock()
            .expect("mock source mutex should not be poisoned")
            .push(m.clone());
        Ok(())
    }
}

struct TestEnvironment {
    config: Arc<Config>,
    dialect: Dialect,
}

impl TestEnvironment {
    fn new(dialect: Dialect) -> Self {
        Self {
            config: Arc::new(Config::default()),
            dialect,
        }
    }
}

impl Environment for TestEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        Box::pin(async {
            Err(EnvironmentError::Config(
                "executor is not available in the test environment".into(),
            ))
        })
    }

    fn dialect(&self) -> Dialect {
        self.dialect
    }
}

fn simple_column(name: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: "text".to_string(),
        nullable: true,
        default: None,
        primary_key: false,
        ..Default::default()
    }
}

fn simple_table(name: &str, cols: &[&str]) -> Table {
    Table {
        name: name.to_string(),
        schema: None,
        primary_key: None,
        columns: cols.iter().map(|c| simple_column(c)).collect(),
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    }
}

fn simple_pk_table(name: &str, cols: &[&str], pk_column: &str) -> Table {
    let mut table = simple_table(name, cols);
    table.primary_key = Some(PrimaryKey {
        name: table.pk_constraint_name(),
        columns: vec![pk_column.to_string()],
    });
    for column in &mut table.columns {
        if column.name == pk_column {
            column.primary_key = true;
            column.nullable = false;
        }
    }
    table
}

fn state_with_tables(tables: &[Table]) -> Schema {
    let mut s = Schema::default();
    for t in tables {
        s.tables.insert(t.name.clone(), t.clone());
    }
    s
}

fn verify_function(name: &str) -> FunctionDef {
    FunctionDef {
        name: name.to_string(),
        schema: None,
        arguments: String::new(),
        returns: "integer".to_string(),
        language: "sql".to_string(),
        body: "SELECT 1".to_string(),
        volatility: Volatility::Volatile,
        security_definer: false,
    }
}

fn verify_trigger(name: &str, function_name: &str) -> TriggerDef {
    TriggerDef {
        name: Some(name.to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: Some(function_name.to_string()),
        when: None,
        query: None,
        language: None,
    }
}

fn migration_with_ops(id: &str, deps: &[&str], ops: Vec<Operation>) -> Migration {
    Migration {
        id: id.to_string(),
        dependencies: deps.iter().map(|s| s.to_string()).collect(),
        operations: ops,
        atomic: true,
    }
}

#[test]
fn native_sql_migrate_matches_offline_planner() {
    let migrations = vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_raw",
            &["0001_create_users"],
            vec![Operation::Statement {
                up: "UPDATE users SET id = id".to_string(),
                down: Some("UPDATE users SET id = id".to_string()),
            }],
        ),
    ];
    let migrator = migrator_from(migrations.clone());
    let offline =
        gaman_core::OfflinePlanner::new(Dialect::Postgres).from_migrations(migrations.clone());

    assert_eq!(
        migrator.sql_migrate(&migrations).unwrap(),
        offline.sql_migrate(&migrations).unwrap()
    );
}

#[test]
fn native_sql_migrate_matches_offline_planner_for_generated_migration() {
    let migrations = vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_create_posts",
            &["0001_create_users"],
            vec![Operation::CreateTable {
                table: simple_table("posts", &["id"]),
            }],
        ),
    ];
    let generated = migration_with_ops(
        "0003_generated_posts",
        &["0001_create_users"],
        vec![Operation::CreateTable {
            table: simple_table("posts", &["id"]),
        }],
    );
    let migrator = migrator_from(migrations.clone());
    let offline = gaman_core::OfflinePlanner::new(Dialect::Postgres).from_migrations(migrations);

    assert_eq!(
        migrator
            .sql_migrate(std::slice::from_ref(&generated))
            .unwrap(),
        offline.sql_migrate(&[generated]).unwrap()
    );
}

#[test]
fn native_sql_rollback_matches_core_renderer() {
    let migrations = vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_raw",
            &["0001_create_users"],
            vec![Operation::Statement {
                up: "UPDATE users SET id = id".to_string(),
                down: Some("UPDATE users SET id = id".to_string()),
            }],
        ),
    ];
    let migrator = migrator_from(migrations.clone());
    let renderer = SqlPlanRenderer::new(Dialect::Postgres, migrations.clone()).unwrap();

    assert_eq!(
        migrator.sql_rollback(&migrations).unwrap(),
        renderer.render_rollback_migrations(&migrations).unwrap()
    );
}

#[tokio::test]
async fn live_apply_executes_same_operation_sql_as_sql_migrate() {
    let migrations = vec![migration_with_ops(
        "0001_create_users",
        &[],
        vec![Operation::CreateTable {
            table: simple_table("users", &["id"]),
        }],
    )];
    let migrator = migrator_from(migrations.clone());
    let expected = migrator.sql_migrate(&migrations).unwrap();
    let mut executor = RecordingExecutor::empty();

    migrator
        .migrate_with(&mut executor, None, false)
        .await
        .unwrap();

    let operation_sql: Vec<String> = executor
        .executed
        .into_iter()
        .filter(|sql| !sql.contains("gaman_migrations"))
        .collect();
    assert_eq!(operation_sql, expected);
}

fn migrator_from(migrations: Vec<Migration>) -> Migrator {
    let source = MockSource {
        migrations,
        ..MockSource::default()
    };
    Migrator::new(
        Box::new(source),
        Box::new(TestEnvironment::new(Dialect::Postgres)),
    )
    .unwrap()
}

fn assert_send<T: Send>(_: T) {}

fn assert_send_sync<T: Send + Sync>() {}

/// Verifies the live migrator and representative live futures can move across runtime threads.
#[test]
fn migrator_and_live_futures_are_send() {
    assert_send_sync::<Migrator>();

    let migrator = migrator_from(vec![]);
    assert_send(migrator.migrate(None, false));
    assert_send(migrator.inspect_db(&[]));
    assert_send(migrator.verify("public"));
}

/// Verifies a live migration future can be spawned onto Tokio's scheduler.
#[tokio::test]
async fn migrate_future_can_be_spawned() {
    let migrator = migrator_from(vec![]);
    let result = tokio::spawn(async move { migrator.migrate(None, false).await })
        .await
        .expect("spawned migration task should not panic");

    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("executor is not available")
    );
}

struct NullExecutor {
    applied: Vec<String>,
    lock_count: usize,
}

impl NullExecutor {
    fn empty() -> Self {
        Self {
            applied: vec![],
            lock_count: 0,
        }
    }
}

struct RecordingExecutor {
    executed: Vec<String>,
}

impl RecordingExecutor {
    fn empty() -> Self {
        Self { executed: vec![] }
    }
}

impl Executor for RecordingExecutor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.executed.push(sql.to_string());
        Box::pin(async { Ok(()) })
    }
    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

impl Executor for NullExecutor {
    fn execute<'a>(&'a mut self, _sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        let applied = self.applied.clone();
        Box::pin(async move { Ok(applied) })
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.lock_count += 1;
        Box::pin(async { Ok(()) })
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.lock_count -= 1;
        Box::pin(async { Ok(()) })
    }
}

struct FailingExecutor {
    applied: Vec<String>,
    fail_on: &'static str,
    lock_count: usize,
    rollback_count: usize,
}

impl FailingExecutor {
    fn new(fail_on: &'static str) -> Self {
        Self {
            applied: vec![],
            fail_on,
            lock_count: 0,
            rollback_count: 0,
        }
    }
}

impl Executor for FailingExecutor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        let should_fail = sql.contains(self.fail_on);
        Box::pin(async move {
            if should_fail {
                Err(ExecutorError::Execute("forced failure".to_string()))
            } else {
                Ok(())
            }
        })
    }
    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        let applied = self.applied.clone();
        Box::pin(async move { Ok(applied) })
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.rollback_count += 1;
        Box::pin(async { Ok(()) })
    }
    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.lock_count += 1;
        Box::pin(async { Ok(()) })
    }
    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.lock_count -= 1;
        Box::pin(async { Ok(()) })
    }
}

struct InspectingExecutor {
    live: Schema,
}

impl Executor for InspectingExecutor {
    fn execute<'a>(&'a mut self, _sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async { Ok(vec![]) })
    }
    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
    }
}

impl Introspectable for InspectingExecutor {
    fn inspect_db<'a>(
        &'a mut self,
        _schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, ExecutorError>> {
        let live = self.live.clone();
        Box::pin(async move { Ok(live) })
    }
}

/// Regression test: migrate() on a multi-migration chain must not fail at validate_plan.
#[tokio::test]
async fn migrate_succeeds_on_multi_migration_chain() {
    let users = simple_table("users", &["id"]);
    let posts = simple_table("posts", &["id"]);
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable { table: users }],
        ),
        migration_with_ops(
            "0002_create_posts",
            &["0001_create_users"],
            vec![Operation::CreateTable { table: posts }],
        ),
    ]);
    m.migrate_with(&mut NullExecutor::empty(), None, false)
        .await
        .expect("migrate must succeed on a valid multi-migration chain");
}

/// Verifies that migrate() acquires and then releases the advisory lock exactly once.
#[tokio::test]
async fn migrate_acquires_and_releases_lock() {
    let m = migrator_from(vec![migration_with_ops(
        "0001_init",
        &[],
        vec![Operation::CreateTable {
            table: simple_table("t", &["id"]),
        }],
    )]);
    let mut ex = NullExecutor::empty();
    m.migrate_with(&mut ex, None, false)
        .await
        .expect("migrate should succeed");
    assert_eq!(
        ex.lock_count, 0,
        "lock must be released after migrate completes"
    );
}

#[tokio::test]
async fn migrate_releases_lock_when_operation_fails() {
    let m = migrator_from(vec![migration_with_ops(
        "0001_init",
        &[],
        vec![Operation::Statement {
            up: "FAIL".to_string(),
            down: None,
        }],
    )]);
    let mut ex = FailingExecutor::new("FAIL");

    let err = m.migrate_with(&mut ex, None, false).await.unwrap_err();

    assert!(err.to_string().contains("forced failure"));
    assert_eq!(ex.lock_count, 0, "lock must be released after failure");
    assert_eq!(ex.rollback_count, 1, "atomic failure should roll back");
}

#[tokio::test]
async fn migrate_releases_lock_when_tracking_table_has_unknown_applied_id() {
    let m = migrator_from(vec![migration_with_ops(
        "0001_init",
        &[],
        vec![Operation::CreateTable {
            table: simple_table("t", &["id"]),
        }],
    )]);
    let mut ex = FailingExecutor::new("never");
    ex.applied = vec!["0009_missing".to_string()];

    let err = m.migrate_with(&mut ex, None, false).await.unwrap_err();

    assert!(err.to_string().contains("not present locally"));
    assert_eq!(
        ex.lock_count, 0,
        "lock must be released after validation failure"
    );
}

#[tokio::test]
async fn migrate_does_not_rollback_non_atomic_failure() {
    let mut migration = migration_with_ops(
        "0001_init",
        &[],
        vec![Operation::Statement {
            up: "FAIL".to_string(),
            down: None,
        }],
    );
    migration.atomic = false;
    let m = migrator_from(vec![migration]);
    let mut ex = FailingExecutor::new("FAIL");

    let err = m.migrate_with(&mut ex, None, false).await.unwrap_err();

    assert!(err.to_string().contains("forced failure"));
    assert_eq!(ex.lock_count, 0, "lock must be released after failure");
    assert_eq!(
        ex.rollback_count, 0,
        "non-atomic failure should not roll back"
    );
}
// we build the Migrator manually to keep a reference to the inner saved vec.
fn migrator_with_source(migrations: Vec<Migration>) -> (Migrator, Arc<MockSourceShared>) {
    let shared = Arc::new(MockSourceShared::default());
    let source = ArcMockSource {
        shared: Arc::clone(&shared),
        migrations,
    };
    let migrator = Migrator::new(
        Box::new(source),
        Box::new(TestEnvironment::new(Dialect::Postgres)),
    )
    .unwrap();
    (migrator, shared)
}

#[derive(Default)]
struct MockSourceShared {
    saved: Mutex<Vec<Migration>>,
}

struct ArcMockSource {
    shared: Arc<MockSourceShared>,
    migrations: Vec<Migration>,
}

impl MigrationSource for ArcMockSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        Ok(self.migrations.clone())
    }
    fn save(&self, m: &Migration) -> Result<(), AdapterError> {
        self.shared
            .saved
            .lock()
            .expect("mock source mutex should not be poisoned")
            .push(m.clone());
        Ok(())
    }
}

#[test]
fn no_changes_returns_none() {
    let m = migrator_from(vec![]);
    assert!(
        m.make_migrations(Some("x".into()), Schema::default(), false, &[])
            .unwrap()
            .is_none()
    );
}

#[test]
fn auto_name_single_create_table() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(None, current, true, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.id.ends_with("_users"),
        "expected id ending in '_users', got '{}'",
        mig.id
    );
}

#[test]
fn auto_name_two_tables() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[
        simple_table("users", &["id"]),
        simple_table("posts", &["id"]),
    ]);
    let mig = m
        .make_migrations(None, current, true, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.id.contains("users_posts") || mig.id.contains("posts_users"),
        "expected 'users_posts' or 'posts_users' in id, got '{}'",
        mig.id
    );
}

#[test]
fn new_table_creates_migration() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let result = m
        .make_migrations(Some("initial".into()), current, false, &[])
        .unwrap();
    let mig = result.unwrap();
    assert_eq!(mig.operations.len(), 1);
    assert!(
        matches!(&mig.operations[0], Operation::CreateTable { table } if table.name == "users")
    );
}

#[test]
fn removed_table_creates_migration() {
    let users = simple_table("users", &["id"]);
    let m = migrator_from(vec![migration_with_ops(
        "0001_initial",
        &[],
        vec![Operation::CreateTable { table: users }],
    )]);
    let result = m
        .make_migrations(Some("drop_users".into()), Schema::default(), false, &[])
        .unwrap();
    let mig = result.unwrap();
    assert_eq!(mig.operations.len(), 1);
    assert!(matches!(&mig.operations[0], Operation::DropTable { table } if table.name == "users"));
}

#[test]
fn added_column_creates_migration() {
    let users_v1 = simple_table("users", &["id"]);
    let m = migrator_from(vec![migration_with_ops(
        "0001_initial",
        &[],
        vec![Operation::CreateTable { table: users_v1 }],
    )]);
    let current = state_with_tables(&[simple_table("users", &["id", "email"])]);
    let mig = m
        .make_migrations(Some("add_email".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.operations
            .iter()
            .any(|op| matches!(op, Operation::AddColumn { column, .. } if column.name == "email"))
    );
}

#[test]
fn dropped_column_creates_migration() {
    let users_v1 = simple_table("users", &["id", "email"]);
    let m = migrator_from(vec![migration_with_ops(
        "0001_initial",
        &[],
        vec![Operation::CreateTable { table: users_v1 }],
    )]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("drop_email".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.operations
            .iter()
            .any(|op| matches!(op, Operation::DropColumn { column, .. } if column.name == "email"))
    );
}

#[test]
fn multiple_ops_in_single_migration() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[
        simple_table("users", &["id"]),
        simple_table("posts", &["id"]),
    ]);
    let mig = m
        .make_migrations(Some("multi".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.operations.len(), 2);
}

#[test]
fn migration_number_increments() {
    let m = migrator_from(vec![migration_with_ops("0001_initial", &[], vec![])]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("add_users".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(mig.id.starts_with("0002_"));
}

#[test]
fn migration_number_empty_graph() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("initial".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(mig.id.starts_with("0001_"));
}

#[test]
fn migration_dependencies_set_to_head() {
    let m = migrator_from(vec![migration_with_ops("0001_initial", &[], vec![])]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("add_users".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.dependencies, vec!["0001_initial"]);
}

#[test]
fn migration_dependencies_empty_graph() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("initial".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(mig.dependencies.is_empty());
}

#[test]
fn dry_run_does_not_save() {
    let (m, shared) = migrator_with_source(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    m.make_migrations(Some("initial".into()), current, true, &[])
        .unwrap()
        .unwrap();
    assert!(
        shared
            .saved
            .lock()
            .expect("mock source mutex should not be poisoned")
            .is_empty()
    );
}

#[test]
fn dry_run_still_returns_migration() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let result = m
        .make_migrations(Some("initial".into()), current, true, &[])
        .unwrap();
    assert!(result.is_some());
}

#[test]
fn conflict_returns_error() {
    // Two heads: 0001_a and 0001_b both have no dependencies
    let m = migrator_from(vec![
        migration_with_ops("0001_a", &[], vec![]),
        migration_with_ops("0001_b", &[], vec![]),
    ]);
    let err = m
        .make_migrations(Some("x".into()), Schema::default(), false, &[])
        .unwrap_err();
    assert!(matches!(err, MigratorError::Graph(GraphError::Conflict)));
}

#[test]
fn replay_error_propagates() {
    // Migration tries to drop a table that was never created
    let bad = migration_with_ops(
        "0001_bad",
        &[],
        vec![Operation::DropTable {
            table: simple_table("ghost", &["id"]),
        }],
    );
    let m = migrator_from(vec![bad]);
    let err = m
        .make_migrations(Some("x".into()), Schema::default(), false, &[])
        .unwrap_err();
    assert!(matches!(err, MigratorError::Replay(_)));
}

#[test]
fn make_migrations_name_used_in_id() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("my_migration".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(mig.id.contains("my_migration"));
}

#[test]
fn incremental_diff() {
    let users = simple_table("users", &["id"]);
    let posts = simple_table("posts", &["id"]);
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable { table: users }],
        ),
        migration_with_ops(
            "0002_create_posts",
            &["0001_create_users"],
            vec![Operation::CreateTable { table: posts }],
        ),
    ]);
    let current = state_with_tables(&[
        simple_table("users", &["id"]),
        simple_table("posts", &["id"]),
        simple_table("comments", &["id"]),
    ]);
    let mig = m
        .make_migrations(Some("add_comments".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.dependencies, vec!["0002_create_posts"]);
    assert_eq!(mig.operations.len(), 1);
    assert!(
        matches!(&mig.operations[0], Operation::CreateTable { table } if table.name == "comments")
    );
}

/// Verifies verify_with() handles schema scoping correctly for non-public schemas.
#[tokio::test]
async fn verify_treats_requested_schema_as_default_namespace() {
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_users",
        &[],
        vec![Operation::CreateTable {
            table: simple_table("users", &["id"]),
        }],
    )]);

    let mut live_users = simple_table("users", &["id"]);
    live_users.schema = Some("isolated".to_string());
    let live = state_with_tables(&[live_users]);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "isolated")
        .await
        .expect("verify should succeed");

    assert!(drift.is_empty(), "expected no drift, got: {drift:?}");
}

#[tokio::test]
async fn verify_treats_same_schema_fk_target_as_default_namespace() {
    let mut posts = simple_table("posts", &["id", "user_id"]);
    posts.foreign_keys.push(ForeignKey::single(
        "posts_user_id_fkey",
        "user_id",
        "users",
        "id",
    ));
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_tables",
        &[],
        vec![
            Operation::CreateTable {
                table: simple_pk_table("users", &["id"], "id"),
            },
            Operation::CreateTable { table: posts },
        ],
    )]);

    let mut live_users = simple_pk_table("users", &["id"], "id");
    live_users.schema = Some("isolated".to_string());
    let mut live_posts = simple_table("posts", &["id", "user_id"]);
    live_posts.schema = Some("isolated".to_string());
    live_posts.foreign_keys.push(ForeignKey::single(
        "posts_user_id_fkey",
        "user_id",
        "isolated.users",
        "id",
    ));
    let live = state_with_tables(&[live_users, live_posts]);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "isolated")
        .await
        .expect("verify should succeed");

    assert!(drift.is_empty(), "expected no drift, got: {drift:?}");
}

#[tokio::test]
async fn verify_ignores_function_body_only_drift() {
    let function = verify_function("compute");
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_function",
        &[],
        vec![Operation::CreateFunction {
            function: function.clone(),
        }],
    )]);

    let mut live = Schema::default();
    let mut live_function = function;
    live_function.body = "SELECT 2".to_string();
    live.functions
        .insert(live_function.name.clone(), live_function);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(drift.is_empty(), "expected no drift, got: {drift:?}");
}

#[tokio::test]
async fn verify_detects_function_signature_drift() {
    let function = verify_function("compute");
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_function",
        &[],
        vec![Operation::CreateFunction {
            function: function.clone(),
        }],
    )]);

    let mut live = Schema::default();
    let mut live_function = function;
    live_function.returns = "text".to_string();
    live.functions
        .insert(live_function.name.clone(), live_function);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(
        matches!(drift.as_slice(), [Operation::AlterFunction { .. }]),
        "expected function signature drift, got: {drift:?}"
    );
}

#[tokio::test]
async fn verify_ignores_trigger_query_only_drift() {
    let mut table = simple_table("events", &["id"]);
    let mut trigger = verify_trigger("events_audit_trg", "audit_fn");
    trigger.function_name = None;
    trigger.query = Some("SELECT 1".to_string());
    table.triggers.push(trigger);
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_table",
        &[],
        vec![Operation::CreateTable {
            table: table.clone(),
        }],
    )]);

    let mut live_table = table;
    live_table.triggers[0].query = Some("SELECT 2".to_string());
    let live = state_with_tables(&[live_table]);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(drift.is_empty(), "expected no drift, got: {drift:?}");
}

#[tokio::test]
async fn verify_detects_trigger_wiring_drift() {
    let mut table = simple_table("events", &["id"]);
    table
        .triggers
        .push(verify_trigger("events_audit_trg", "audit_fn"));
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_table",
        &[],
        vec![Operation::CreateTable {
            table: table.clone(),
        }],
    )]);

    let mut live_table = table;
    live_table.triggers[0].function_name = Some("other_fn".to_string());
    let live = state_with_tables(&[live_table]);
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(
        matches!(drift.as_slice(), [Operation::AlterTrigger { .. }]),
        "expected trigger wiring drift, got: {drift:?}"
    );
}

#[tokio::test]
async fn verify_detects_enum_label_drift() {
    let enum_def = EnumDef {
        name: "status".to_string(),
        schema: None,
        values: vec!["pending".to_string(), "done".to_string()],
    };
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_enum",
        &[],
        vec![Operation::CreateEnum {
            enum_def: enum_def.clone(),
        }],
    )]);

    let mut live = Schema::default();
    live.enums.insert(
        enum_def.name.clone(),
        EnumDef {
            values: vec!["pending".to_string(), "failed".to_string()],
            ..enum_def
        },
    );
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(
        drift
            .iter()
            .any(|op| matches!(op, Operation::DropEnum { .. }))
            && drift
                .iter()
                .any(|op| matches!(op, Operation::CreateEnum { .. })),
        "expected enum label drift, got: {drift:?}"
    );
}

#[tokio::test]
async fn verify_detects_extension_version_drift() {
    let extension = ExtensionDef {
        name: "pgcrypto".to_string(),
        schema: None,
        version: Some("1.0".to_string()),
    };
    let migrator = migrator_from(vec![migration_with_ops(
        "0001_create_extension",
        &[],
        vec![Operation::CreateExtension {
            extension: extension.clone(),
        }],
    )]);

    let mut live = Schema::default();
    live.extensions.insert(
        extension.name.clone(),
        ExtensionDef {
            version: Some("1.1".to_string()),
            ..extension
        },
    );
    let mut executor = InspectingExecutor { live };

    let drift = migrator
        .verify_with(&mut executor, "public")
        .await
        .expect("verify should succeed");

    assert!(
        drift
            .iter()
            .any(|op| matches!(op, Operation::DropExtension { .. }))
            && drift
                .iter()
                .any(|op| matches!(op, Operation::CreateExtension { .. })),
        "expected extension version drift, got: {drift:?}"
    );
}

/// Validates that composite primary key columns generate a canonical table primary key.
#[test]
fn make_migrations_accepts_composite_pk_columns() {
    let m = migrator_from(vec![]);
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            Column {
                name: "id".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "alt_id".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut current = Schema::default();
    current.tables.insert("users".to_string(), table);
    let migration = m
        .make_migrations(Some("composite_pk".into()), current, true, &[])
        .unwrap()
        .expect("migration");
    let Operation::CreateTable { table } = &migration.operations[0] else {
        panic!("expected create table");
    };
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.name, "users_pkey");
    assert_eq!(pk.columns, ["id", "alt_id"]);
}

#[test]
fn make_migrations_asks_for_new_unknown_type_before_writing() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id", "age"])]);
    let mut current = current;
    current.tables.get_mut("users").unwrap().columns[1].col_type = "intger".to_string();

    let err = m
        .make_migrations(Some("add_users".into()), current, true, &[])
        .unwrap_err();
    let MigratorError::NeedsInput(clarifications) = err else {
        panic!("expected needs input");
    };

    assert!(matches!(
        &clarifications[0].kind,
        gaman_core::disambiguator::ClarificationKind::UnknownType {
            type_name,
            suggested,
            ..
        } if type_name == "intger" && suggested.contains(&"integer".to_string())
    ));
}

#[test]
fn make_migrations_keeps_approved_unknown_type() {
    use gaman_core::disambiguator::Answer;

    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id", "code"])]);
    let mut current = current;
    current.tables.get_mut("users").unwrap().columns[1].col_type = "project_code".to_string();
    let decisions = vec![Decision {
        clarification_id: "unknown_type:users:code".to_string(),
        answer: Answer::KeepType,
    }];

    let migration = m
        .make_migrations(Some("add_users".into()), current, true, &decisions)
        .expect("planning should succeed")
        .expect("migration should be generated");
    let Operation::CreateTable { table } = &migration.operations[0] else {
        panic!("expected create table");
    };

    assert_eq!(table.columns[1].col_type, "project_code");
}

/// Validates that save() is called exactly once when dry_run is false.
#[test]
fn save_called_when_not_dry_run() {
    let (m, shared) = migrator_with_source(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    m.make_migrations(Some("initial".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        shared
            .saved
            .lock()
            .expect("mock source mutex should not be poisoned")
            .len(),
        1
    );
}

#[test]
fn validate_plan_rejects_irreversible_on_backward() {
    let m = migrator_from(vec![]);
    let migrations = vec![Migration {
        id: "0001_x".into(),
        dependencies: vec![],
        operations: vec![Operation::Statement {
            up: "SELECT 1".into(),
            down: None,
        }],
        atomic: true,
    }];
    let err = m.validate_plan(&migrations, false).unwrap_err();
    assert!(matches!(err, MigratorError::SqlPlan(_)));
}

#[test]
fn validate_plan_accepts_reversible_on_backward() {
    let m = migrator_from(vec![]);
    let migrations = vec![Migration {
        id: "0001_x".into(),
        dependencies: vec![],
        operations: vec![Operation::CreateTable {
            table: simple_table("users", &["id"]),
        }],
        atomic: true,
    }];
    assert!(m.validate_plan(&migrations, false).is_ok());
}

#[test]
fn validate_plan_rejects_fk_to_unknown_table() {
    use gaman_core::states::ForeignKey;
    let m = migrator_from(vec![]);
    let table = Table {
        name: "posts".into(),
        schema: None,
        primary_key: None,
        columns: vec![simple_column("id")],
        foreign_keys: vec![ForeignKey::single(
            "posts_user_id_fk",
            "user_id",
            "users",
            "id",
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let migrations = vec![Migration {
        id: "0001_x".into(),
        dependencies: vec![],
        operations: vec![Operation::CreateTable { table }],
        atomic: true,
    }];
    let err = m.validate_plan(&migrations, true).unwrap_err();
    assert!(matches!(err, MigratorError::Config(s) if s.contains("unknown table")));
}

#[test]
fn validate_plan_accepts_fk_to_existing_table_in_prior_migration() {
    use gaman_core::states::ForeignKey;
    // Both migrations must be passed so validate_plan can build state incrementally.
    let m = migrator_from(vec![]);
    let users = simple_pk_table("users", &["id"], "id");
    let posts = Table {
        name: "posts".into(),
        schema: None,
        primary_key: None,
        columns: vec![simple_column("id"), simple_column("user_id")],
        foreign_keys: vec![ForeignKey::single(
            "posts_user_id_fk",
            "user_id",
            "users",
            "id",
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let migrations = vec![
        Migration {
            id: "0001_create_users".into(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable { table: users }],
            atomic: true,
        },
        Migration {
            id: "0002_create_posts".into(),
            dependencies: vec!["0001_create_users".into()],
            operations: vec![Operation::CreateTable { table: posts }],
            atomic: true,
        },
    ];
    assert!(m.validate_plan(&migrations, true).is_ok());
}

#[test]
fn validate_plan_rejects_invalid_composite_fk_metadata() {
    use gaman_core::states::ForeignKey;
    let m = migrator_from(vec![]);
    let users = simple_pk_table("users", &["id"], "id");
    let posts = Table {
        name: "posts".into(),
        schema: None,
        primary_key: None,
        columns: vec![
            simple_column("id"),
            simple_column("tenant_id"),
            simple_column("user_id"),
        ],
        foreign_keys: vec![ForeignKey::new(
            "posts_user_id_fk",
            ["tenant_id", "user_id"],
            "users",
            ["id"],
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let migrations = vec![
        Migration {
            id: "0001_create_users".into(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable { table: users }],
            atomic: true,
        },
        Migration {
            id: "0002_create_posts".into(),
            dependencies: vec!["0001_create_users".into()],
            operations: vec![Operation::CreateTable { table: posts }],
            atomic: true,
        },
    ];

    let err = m.validate_plan(&migrations, true).unwrap_err();
    assert!(
        matches!(err, MigratorError::Config(s) if s.contains("source and target column counts differ"))
    );
}

#[test]
fn validate_plan_rejects_duplicate_index_name() {
    use gaman_core::states::Index;
    // All migrations must be passed together so validate_plan has full context.
    let m = migrator_from(vec![]);
    let migrations = vec![
        Migration {
            id: "0001_create_users".into(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
            atomic: true,
        },
        Migration {
            id: "0002_idx_a".into(),
            dependencies: vec!["0001_create_users".into()],
            operations: vec![Operation::AddIndex {
                table_name: "users".into(),
                index: Index {
                    name: "users_name_idx".into(),
                    columns: vec!["id".into()],
                    unique: false,
                    predicate: None,
                },
                concurrent: false,
            }],
            atomic: true,
        },
        Migration {
            id: "0003_idx_b".into(),
            dependencies: vec!["0002_idx_a".into()],
            operations: vec![Operation::AddIndex {
                table_name: "users".into(),
                index: Index {
                    name: "users_name_idx".into(),
                    columns: vec!["id".into()],
                    unique: false,
                    predicate: None,
                },
                concurrent: false,
            }],
            atomic: true,
        },
    ];
    let err = m.validate_plan(&migrations, true).unwrap_err();
    assert!(matches!(err, MigratorError::Config(s) if s.contains("duplicate index")));
}

#[test]
fn graph_rejects_invalid_migration_id() {
    use gaman_core::graphs::MigrationGraph;
    let err = MigrationGraph::validate_id("0001 bad id!").unwrap_err();
    assert!(matches!(err, gaman_core::graphs::GraphError::InvalidId(_)));
    assert!(MigrationGraph::validate_id("0001_good_id").is_ok());
}

#[test]
fn validate_duplicate_column_in_schema_state() {
    use gaman_core::states::Column;
    let table = Table {
        name: "users".into(),
        schema: None,
        primary_key: None,
        columns: vec![
            Column {
                name: "id".into(),
                col_type: "integer".into(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "id".into(),
                col_type: "text".into(),
                nullable: true,
                default: None,
                primary_key: false,
                ..Default::default()
            },
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut state = Schema::default();
    state.tables.insert("users".into(), table);
    let err = state.validate().unwrap_err();
    assert!(err.contains("duplicate column"));
}

#[tokio::test]
async fn show_migrations_marks_applied_and_pending() {
    let users = simple_table("users", &["id"]);
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable { table: users }],
        ),
        migration_with_ops("0002_add_email", &["0001_create_users"], vec![]),
    ]);
    let mut exec = NullExecutor {
        applied: vec!["0001_create_users".to_string()],
        lock_count: 0,
    };
    let rows = m.show_migrations_with(&mut exec).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0], ("0001_create_users".to_string(), true));
    assert_eq!(rows[1], ("0002_add_email".to_string(), false));
}

#[tokio::test]
async fn show_migrations_empty_graph() {
    let m = migrator_from(vec![]);
    let mut exec = NullExecutor::empty();
    let rows = m.show_migrations_with(&mut exec).await.unwrap();
    assert!(rows.is_empty());
}

fn table_with_fk(name: &str, fk_to: &str) -> Table {
    use gaman_core::states::ForeignKey;
    Table {
        name: name.to_string(),
        schema: None,
        primary_key: None,
        columns: vec![simple_column("id"), simple_column("ref_id")],
        foreign_keys: vec![ForeignKey::single(
            format!("{name}_{fk_to}_fkey"),
            "ref_id".to_string(),
            fk_to.to_string(),
            "id".to_string(),
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    }
}

/// New migration on an empty graph has no dependencies.
#[test]
fn deps_empty_graph_yields_no_deps() {
    let m = migrator_from(vec![]);
    let current = state_with_tables(&[simple_table("users", &["id"])]);
    let mig = m
        .make_migrations(Some("initial".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.dependencies.is_empty(),
        "no migrations exist, so no deps expected"
    );
}

/// When one root migration exists and the new migration only creates a new table
/// with no cross-namespace FK, deps = last root migration.
#[test]
fn deps_single_root_migration_is_dep() {
    let m = migrator_from(vec![migration_with_ops(
        "0001_create_users",
        &[],
        vec![Operation::CreateTable {
            table: simple_table("users", &["id"]),
        }],
    )]);
    let current = state_with_tables(&[
        simple_table("users", &["id"]),
        simple_table("posts", &["id"]),
    ]);
    let mig = m
        .make_migrations(Some("add_posts".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.dependencies, vec!["0001_create_users"]);
}

/// A FK pointing to a table in the same (root) namespace does not add extra deps
/// beyond the last root migration — both source and target share namespace "".
#[test]
fn deps_fk_to_root_ns_table_is_just_root_dep() {
    let users = simple_pk_table("users", &["id"], "id");
    let m = migrator_from(vec![migration_with_ops(
        "0001_create_users",
        &[],
        vec![Operation::CreateTable { table: users }],
    )]);
    let posts = table_with_fk("posts", "users");
    let current = state_with_tables(&[simple_pk_table("users", &["id"], "id"), posts]);
    let mig = m
        .make_migrations(Some("add_posts".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.dependencies, vec!["0001_create_users"]);
}

/// A FK pointing to a table owned by a namespaced migration adds that namespace's
/// last migration id to deps, alongside the last root migration.
/// auth/0001_create_users depends on 0001_root_init (as it would have been generated),
/// making auth/0001_create_users the single head.
#[test]
fn deps_fk_to_namespaced_table_adds_namespace_dep() {
    let auth_users = simple_pk_table("auth_users", &["id"], "id");
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_root_init",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("settings", &["id"]),
            }],
        ),
        migration_with_ops(
            "auth/0001_create_users",
            &["0001_root_init"],
            vec![Operation::CreateTable { table: auth_users }],
        ),
    ]);
    let posts = table_with_fk("posts", "auth_users");
    let current = state_with_tables(&[
        simple_table("settings", &["id"]),
        simple_pk_table("auth_users", &["id"], "id"),
        posts,
    ]);
    let mig = m
        .make_migrations(Some("add_posts".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.dependencies.contains(&"0001_root_init".to_string()),
        "must include last root migration"
    );
    assert!(
        mig.dependencies
            .contains(&"auth/0001_create_users".to_string()),
        "must include last auth migration"
    );
}

/// When the new migration references tables from two different namespaces,
/// both namespace heads appear in deps. The namespaced migrations are chained from root
/// (as they would be in practice) to produce a single head.
#[test]
fn deps_multiple_cross_namespace_fks_include_all() {
    let auth_users = simple_pk_table("auth_users", &["id"], "id");
    let billing_plans = simple_pk_table("billing_plans", &["id"], "id");
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_root",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("root_t", &["id"]),
            }],
        ),
        migration_with_ops(
            "auth/0001_users",
            &["0001_root"],
            vec![Operation::CreateTable { table: auth_users }],
        ),
        migration_with_ops(
            "billing/0001_plans",
            &["auth/0001_users"],
            vec![Operation::CreateTable {
                table: billing_plans,
            }],
        ),
    ]);
    let t = {
        use gaman_core::states::ForeignKey;
        Table {
            name: "subscriptions".to_string(),
            schema: None,
            primary_key: None,
            columns: vec![
                simple_column("id"),
                simple_column("user_id"),
                simple_column("plan_id"),
            ],
            foreign_keys: vec![
                ForeignKey::single("sub_user_fkey", "user_id", "auth_users", "id"),
                ForeignKey::single("sub_plan_fkey", "plan_id", "billing_plans", "id"),
            ],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    };
    let current = state_with_tables(&[
        simple_table("root_t", &["id"]),
        simple_pk_table("auth_users", &["id"], "id"),
        simple_pk_table("billing_plans", &["id"], "id"),
        t,
    ]);
    let mig = m
        .make_migrations(Some("add_subscriptions".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(mig.dependencies.contains(&"0001_root".to_string()));
    assert!(mig.dependencies.contains(&"auth/0001_users".to_string()));
    assert!(mig.dependencies.contains(&"billing/0001_plans".to_string()));
}

/// When there are no root migrations but namespaced ones exist, the new root migration
/// has no deps (last_per_ns[""] is absent).
#[test]
fn deps_no_root_migrations_yields_empty_deps() {
    let auth_users = simple_table("auth_users", &["id"]);
    let m = migrator_from(vec![migration_with_ops(
        "auth/0001_create_users",
        &[],
        vec![Operation::CreateTable { table: auth_users }],
    )]);
    let current = state_with_tables(&[
        simple_table("auth_users", &["id"]),
        simple_table("settings", &["id"]),
    ]);
    let mig = m
        .make_migrations(Some("add_settings".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        !mig.dependencies
            .contains(&"auth/0001_create_users".to_string()),
        "new root migration should not dep on unrelated auth namespace"
    );
    assert!(
        mig.dependencies.is_empty(),
        "no root ns migration exists, so deps must be empty"
    );
}

/// The last migration in a namespace, not just the first one, is used as the dep.
#[test]
fn deps_uses_last_not_first_migration_in_namespace() {
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_add_email",
            &["0001_create_users"],
            vec![Operation::AddColumn {
                table_name: "users".into(),
                column: simple_column("email"),
            }],
        ),
    ]);
    let current = state_with_tables(&[
        simple_table("users", &["id", "email"]),
        simple_table("posts", &["id"]),
    ]);
    let mig = m
        .make_migrations(Some("add_posts".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(
        mig.dependencies,
        vec!["0002_add_email"],
        "must dep on last migration, not first"
    );
}

/// An empty migration always deps on the last root migration only.
#[test]
fn make_empty_migration_deps_on_last_root() {
    let (m, _) = migrator_with_source(vec![
        migration_with_ops(
            "0001_init",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_more",
            &["0001_init"],
            vec![Operation::AddColumn {
                table_name: "users".into(),
                column: simple_column("email"),
            }],
        ),
    ]);
    let mig = m.make_empty_migration("placeholder".into()).unwrap();
    assert_eq!(mig.dependencies, vec!["0002_more"]);
}

/// namespace_of correctly strips the last segment, including nested namespaces.
#[test]
fn namespace_of_handles_nested_and_root() {
    assert_eq!(namespace_of("0001_init"), "");
    assert_eq!(namespace_of("auth/0001_users"), "auth");
    assert_eq!(namespace_of("auth/sub/0001_users"), "auth/sub");
}

/// A new migration that only modifies existing root-ns entities still gets
/// exactly the last root migration as its sole dep.
#[test]
fn deps_alter_existing_table_uses_last_root_dep() {
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("users", &["id"]),
            }],
        ),
        migration_with_ops(
            "0002_add_email",
            &["0001_create_users"],
            vec![Operation::AddColumn {
                table_name: "users".into(),
                column: simple_column("email"),
            }],
        ),
    ]);
    let current = state_with_tables(&[simple_table("users", &["id", "email", "name"])]);
    let mig = m
        .make_migrations(Some("add_name".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert_eq!(mig.dependencies, vec!["0002_add_email"]);
}

/// When a namespaced migration has multiple entries, only the last one is used.
/// auth/0001_users chains from root; auth/0002_groups is the single head.
#[test]
fn deps_namespaced_ns_uses_last_migration() {
    let m = migrator_from(vec![
        migration_with_ops(
            "0001_root",
            &[],
            vec![Operation::CreateTable {
                table: simple_table("root_t", &["id"]),
            }],
        ),
        migration_with_ops(
            "auth/0001_users",
            &["0001_root"],
            vec![Operation::CreateTable {
                table: simple_pk_table("auth_users", &["id"], "id"),
            }],
        ),
        migration_with_ops(
            "auth/0002_groups",
            &["auth/0001_users"],
            vec![Operation::CreateTable {
                table: simple_table("auth_groups", &["id"]),
            }],
        ),
    ]);
    let posts = table_with_fk("posts", "auth_users");
    let current = state_with_tables(&[
        simple_table("root_t", &["id"]),
        simple_pk_table("auth_users", &["id"], "id"),
        simple_table("auth_groups", &["id"]),
        posts,
    ]);
    let mig = m
        .make_migrations(Some("add_posts".into()), current, false, &[])
        .unwrap()
        .unwrap();
    assert!(
        mig.dependencies.contains(&"auth/0002_groups".to_string()),
        "must use last auth migration, not auth/0001_users"
    );
    assert!(
        !mig.dependencies.contains(&"auth/0001_users".to_string()),
        "must not include non-last auth migration"
    );
}
