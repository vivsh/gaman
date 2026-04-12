use std::collections::HashSet;
use std::sync::Arc;

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource};
use crate::conf::Config;
use crate::dialects::{Dialect, DialectError};
use crate::diff::{DiffEngine, DiffError};
use crate::executor::{Executor, ExecutorError, Introspectable, Invoker, InvokerError};
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::{ReplayError, SchemaState};

#[derive(Debug, Error)]
pub enum MigratorError {
    #[error("failed to load migration files: {0}")]
    Adapter(#[from] AdapterError),
    #[error("migration dependency error: {0}")]
    Graph(#[from] GraphError),
    #[error("schema diff failed: {0}")]
    Diff(#[from] DiffError),
    #[error("dialect error: {0}")]
    Dialect(#[from] DialectError),
    #[error("database operation failed: {0}")]
    Executor(#[from] ExecutorError),
    #[error("migration replay failed")]
    Replay(#[from] ReplayError),
    #[error("subprocess invocation failed: {0}")]
    Invoke(#[from] InvokerError),
    #[error("configuration error: {0}")]
    Config(String),
}

/// Central orchestrator for all migration actions.
/// Holds shared config, the migration graph, the diff engine, and the SQL dialect.
/// The CLI constructs one instance and calls its methods directly.
pub struct Migrator {
    pub config: Arc<Config>,
    pub source: Box<dyn MigrationSource>,
    pub graph: MigrationGraph,
    pub diff: DiffEngine,
    pub dialect: Dialect,
}

impl Migrator {
    pub fn new(config: Arc<Config>, source: Box<dyn MigrationSource>, dialect: Dialect) -> Result<Self, MigratorError> {
        let mut graph = MigrationGraph::new();
        let migrations = source.load_all()?;
        for migration in migrations {
            graph.add(migration)?;
        }
        // Validate dependency integrity eagerly so broken repos fail at construction, not at migrate-time.
        graph.topological_order()?;
        Ok(Self {
            config,
            source,
            graph,
            diff: DiffEngine::new(),
            dialect,
        })
    }

    /// Generate a new migration by diffing `current` against the replayed previous state.
    /// Refuses if there are multiple heads — resolve with `make_merge_migration` first.
    /// Returns `None` when there are no changes.
    pub fn make_migrations(
        &self,
        name: String,
        current: SchemaState,
        dry_run: bool,
    ) -> Result<Option<Migration>, MigratorError> {
        self.graph.detect_conflict()?;
        current.validate().map_err(MigratorError::Config)?;
        let previous = self.replay()?;
        let ops = self.diff.diff(&current, &previous)?;
        if ops.is_empty() {
            return Ok(None);
        }
        let id = format!("{:04}_{}", self.graph.next_number(), name);
        MigrationGraph::validate_id(&id).map_err(MigratorError::Graph)?;
        let mut dependencies: Vec<String> = self.graph.heads().iter().map(|s| s.to_string()).collect();
        dependencies.sort();
        let migration = Migration { id, dependencies, operations: ops };
        if !dry_run {
            self.source.save(&migration)?;
        }
        Ok(Some(migration))
    }

    fn replay(&self) -> Result<SchemaState, MigratorError> {
        let order = self.graph.topological_order()?;
        let mut state = SchemaState::default();
        for id in order {
            if let Some(migration) = self.graph.get(id) {
                for (i, op) in migration.operations.iter().enumerate() {
                    state.apply(op).map_err(|e| ReplayError::WithContext {
                        migration: id.to_string(),
                        op_num: i + 1,
                        inner: Box::new(e),
                    })?;
                }
            }
        }
        Ok(state)
    }

    /// Generate an empty migration with no operations.
    /// Dependencies are set to the current graph heads so it slots in at the tip.
    /// The id is auto-prefixed with the next sequential number: `{n:04}_{name}`.
    pub fn make_empty_migration(&self, name: String) -> Result<Migration, MigratorError> {
        self.graph.topological_order()?;
        let id = format!("{:04}_{}", self.graph.next_number(), name);
        MigrationGraph::validate_id(&id).map_err(MigratorError::Graph)?;
        let mut dependencies: Vec<String> = self.graph.heads().iter().map(|s| s.to_string()).collect();
        dependencies.sort();
        let migration = Migration { id, dependencies, operations: vec![] };
        self.source.save(&migration)?;
        Ok(migration)
    }

    /// Detect conflicts and produce a merge migration that unifies all heads.
    /// Errors if there are zero or one heads (nothing to merge).
    pub fn make_merge_migration(&self, name: String) -> Result<Migration, MigratorError> {
        self.graph.topological_order()?;
        let id = format!("{:04}_{}", self.graph.next_number(), name);
        MigrationGraph::validate_id(&id).map_err(MigratorError::Graph)?;
        let migration = self.graph.create_merge_migration(id)?;
        self.source.save(&migration)?;
        Ok(migration)
    }

    /// Translate the operations in `migrations` to SQL statements using `self.dialect`.
    /// The caller controls direction — pass operations as-is for forward, or pre-mapped
    /// inverses in reverse order for backward. Does not include tracking INSERT/DELETE.
    pub fn sql_migrate(&self, migrations: &[Migration]) -> Result<Vec<String>, MigratorError> {
        let mut stmts = Vec::new();
        for migration in migrations {
            for op in &migration.operations {
                stmts.extend(self.dialect.operation_to_sql(op)?);
            }
        }
        Ok(stmts)
    }

    /// Validate a list of ordered migrations before any SQL is sent to the database.
    /// Checks:
    /// - All migrations that will be reverted are fully reversible.
    /// - FK targets referenced by CreateTable/AddForeignKey exist in the
    ///   incrementally-replayed state at the point they are applied.
    /// - No two operations in the forward path produce duplicate index or
    ///   constraint names on the same table.
    pub fn validate_plan(
        &self,
        migrations: &[Migration],
        direction_forward: bool,
    ) -> Result<(), MigratorError> {
        if !direction_forward {
            for m in migrations {
                for (i, op) in m.operations.iter().enumerate() {
                    if op.inverse().is_none() {
                        return Err(MigratorError::Config(format!(
                            "migration '{}' (operation {}): operation '{}' has no inverse",
                            m.id, i + 1, op.type_name()
                        )));
                    }
                }
            }
            return Ok(());
        }

        let mut state = SchemaState::default();
        let mut index_names: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut constraint_names: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();

        for m in migrations {
            for (i, op) in m.operations.iter().enumerate() {
                match op {
                    crate::operations::Operation::CreateTable { table } => {
                        for fk in &table.foreign_keys {
                            if !state.tables.contains_key(&fk.to_table) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): foreign key '{}' references unknown table '{}'",
                                    m.id, i + 1, fk.name, fk.to_table
                                )));
                            }
                        }
                        for idx in &table.indexes {
                            let entry = index_names.entry(table.name.clone()).or_default();
                            if !entry.insert(idx.name.clone()) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): duplicate index name '{}' on table '{}'",
                                    m.id, i + 1, idx.name, table.name
                                )));
                            }
                        }
                        for c in &table.constraints {
                            let entry = constraint_names.entry(table.name.clone()).or_default();
                            if !entry.insert(c.name().to_string()) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): duplicate constraint name '{}' on table '{}'",
                                    m.id, i + 1, c.name(), table.name
                                )));
                            }
                        }
                    }
                    crate::operations::Operation::AddForeignKey { table_name: _, foreign_key } => {
                        if !state.tables.contains_key(&foreign_key.to_table) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): foreign key '{}' references unknown table '{}'",
                                m.id, i + 1, foreign_key.name, foreign_key.to_table
                            )));
                        }
                    }
                    crate::operations::Operation::AddIndex { table_name, index } => {
                        let entry = index_names.entry(table_name.clone()).or_default();
                        if !entry.insert(index.name.clone()) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): duplicate index name '{}' on table '{}'",
                                m.id, i + 1, index.name, table_name
                            )));
                        }
                    }
                    crate::operations::Operation::AddConstraint { table_name, constraint } => {
                        let entry = constraint_names.entry(table_name.clone()).or_default();
                        if !entry.insert(constraint.name().to_string()) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): duplicate constraint name '{}' on table '{}'",
                                m.id, i + 1, constraint.name(), table_name
                            )));
                        }
                    }
                    _ => {}
                }
                state.apply(op).map_err(|e| ReplayError::WithContext {
                    migration: m.id.clone(),
                    op_num: i + 1,
                    inner: Box::new(e),
                })?;
            }
        }
        Ok(())
    }

    /// Create the migration tracking table if it does not already exist.
    /// Safe to call repeatedly — uses CREATE TABLE IF NOT EXISTS internally.
    pub fn install(&self, executor: &mut dyn Executor) -> Result<(), MigratorError> {
        for sql in self.dialect.create_tracking_table_sql() {
            executor.execute(&sql)?;
        }
        Ok(())
    }

    fn run_ops(
        &self,
        ops: &[Operation],
        executor: &mut dyn Executor,
        invoker: Option<&dyn Invoker>,
    ) -> Result<(), MigratorError> {
        for op in ops {
            if let Operation::Invoke { up, .. } = op {
                let inv = invoker.ok_or(InvokerError::NoInvoker)?;
                if inv.must_commit() {
                    executor.commit()?;
                    inv.invoke(up, executor)?;
                    executor.begin()?;
                } else {
                    inv.invoke(up, executor)?;
                }
            } else {
                for sql in self.dialect.operation_to_sql(op)? {
                    executor.execute(&sql)?;
                }
            }
        }
        Ok(())
    }

    /// Apply all unapplied migrations in topological order.
    /// If `target` is given, apply or roll back to that migration id.
    /// If `fake` is true, record migrations as applied without executing them.
    /// Refuses if there are multiple heads — resolve with `make_merge_migration` first.
    /// Calls `install` internally so the tracking table is always present.
    /// Each migration runs in its own transaction; a failure rolls back only that migration.
    pub fn migrate(
        &self,
        executor: &mut dyn Executor,
        invoker: Option<&dyn Invoker>,
        target: Option<&str>,
        fake: bool,
    ) -> Result<(), MigratorError> {
        self.graph.detect_conflict()?;
        let all_ordered = self.graph.topological_order()?;

        let all_migrations: Vec<_> = all_ordered
            .iter()
            .filter_map(|id| self.graph.get(id))
            .cloned()
            .collect();
        self.validate_plan(&all_migrations, true)?;

        self.install(executor)?;
        executor.acquire_lock()?;

        if let Some(target_id) = target {
            if self.graph.get(target_id).is_none() {
                return Err(MigratorError::Config(format!("unknown target migration '{target_id}'")));
            }

            let order = all_ordered;
            let applied: HashSet<String> = executor.fetch_strings(self.dialect.applied_migrations_sql())?.into_iter().collect();

            let target_pos = order.iter().position(|id| *id == target_id)
                .expect("target exists in graph so must be in topo order");

            let mut to_revert: Vec<&str> = order[target_pos + 1..]
                .iter()
                .filter(|id| applied.contains(*id as &str))
                .copied()
                .collect();
            to_revert.reverse();

            for id in to_revert {
                let migration = self.graph.get(id).expect("applied id must exist in graph");
                executor.begin()?;
                if !fake {
                    let mut inv_ops = Vec::with_capacity(migration.operations.len());
                    for op in &migration.operations {
                        match op.inverse() {
                            Some(inv) => inv_ops.push(inv),
                            None => {
                                let _ = executor.rollback();
                                return Err(MigratorError::Config(format!(
                                    "migration '{id}' is not reversible: operation '{}' has no inverse",
                                    op.type_name()
                                )));
                            }
                        }
                    }
                    inv_ops.reverse();
                    if let Err(e) = self.run_ops(&inv_ops, executor, invoker) {
                        let _ = executor.rollback();
                        return Err(e);
                    }
                }
                if let Err(e) = executor.execute(&self.dialect.unrecord_sql(id)) {
                    let _ = executor.rollback();
                    return Err(e.into());
                }
                executor.commit()?;
            }

            let pending: Vec<&str> = order[..=target_pos]
                .iter()
                .filter(|id| !applied.contains(*id as &str))
                .copied()
                .collect();
            for id in pending {
                let migration = self.graph.get(id).expect("pending id must exist in graph");
                executor.begin()?;
                if !fake
                    && let Err(e) = self.run_ops(&migration.operations, executor, invoker) {
                        let _ = executor.rollback();
                        return Err(e);
                    }
                if let Err(e) = executor.execute(&self.dialect.record_sql(id)) {
                    let _ = executor.rollback();
                    return Err(e.into());
                }
                executor.commit()?;
            }

            executor.release_lock()?;
            return Ok(());
        }

        let applied: HashSet<String> = executor.fetch_strings(self.dialect.applied_migrations_sql())?.into_iter().collect();
        let pending: Vec<String> = all_ordered.iter()
            .filter(|id| !applied.contains(**id))
            .map(|id| id.to_string())
            .collect();
        for id in &pending {
            let migration = self.graph.get(id).expect("pending id must exist in graph");
            executor.begin()?;
            if !fake
                && let Err(e) = self.run_ops(&migration.operations, executor, invoker) {
                    let _ = executor.rollback();
                    return Err(e);
                }
            if let Err(e) = executor.execute(&self.dialect.record_sql(id)) {
                let _ = executor.rollback();
                return Err(e.into());
            }
            executor.commit()?;
        }
        executor.release_lock()?;
        Ok(())
    }

    /// Return the ordered list of migration ids that would be applied.
    /// Refuses on conflict — the graph must have a single head to produce a linear plan.
    /// Calls `install` internally so the tracking table is always present.
    pub fn plan(&self, executor: &mut dyn Executor) -> Result<Vec<String>, MigratorError> {
        self.graph.detect_conflict()?;
        self.install(executor)?;
        let order = self.graph.topological_order()?;
        let applied: HashSet<String> = executor.fetch_strings(self.dialect.applied_migrations_sql())?.into_iter().collect();
        let pending = order.iter()
            .filter(|id| !applied.contains(**id))
            .map(|id| id.to_string())
            .collect();
        Ok(pending)
    }

    /// Return true if there are unapplied migrations, false otherwise.
    pub fn check(&self, executor: &mut dyn Executor) -> Result<bool, MigratorError> {
        self.plan(executor).map(|pending| !pending.is_empty())
    }

    /// Return all migration ids in topological order paired with whether each has been applied.
    pub fn show_migrations(&self, executor: &mut dyn Executor) -> Result<Vec<(String, bool)>, MigratorError> {
        self.graph.detect_conflict()?;
        self.install(executor)?;
        let order = self.graph.topological_order()?;
        let applied: HashSet<String> = executor.fetch_strings(self.dialect.applied_migrations_sql())?.into_iter().collect();
        Ok(order.iter().map(|id| (id.to_string(), applied.contains(*id))).collect())
    }

    /// Compare the replayed schema state against the live database and return any differences.
    /// An empty vec means the database matches migrations exactly — no drift.
    /// Scoped to tables/columns/indexes/FKs/constraints only; views and functions are excluded
    /// because their canonical representation differs too much between YAML and pg_catalog.
    pub fn verify(&self, executor: &mut (impl Executor + Introspectable), schema: &str) -> Result<Vec<Operation>, MigratorError> {
        let mut replay = self.replay()?;
        normalize_state_types(&mut replay, &self.dialect);

        let mut live = executor
            .inspect_db(&[schema])
            .map_err(MigratorError::Executor)?;
        normalize_state_types(&mut live, &self.dialect);

        // Strip views and functions — too many representation differences to compare reliably.
        live.views.clear();
        replay.views.clear();
        live.functions.clear();
        replay.functions.clear();

        Ok(self.diff.diff(&replay, &live)?)
    }
}

fn normalize_state_types(state: &mut SchemaState, dialect: &crate::dialects::Dialect) {
    for table in state.tables.values_mut() {
        for col in table.columns.iter_mut() {
            let normalized = dialect.normalize_type(&col.col_type).to_string();
            col.col_type = normalized;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::sync::Arc;

    use super::*;
    use crate::adapters::AdapterError;
    use crate::dialects::Dialect;
    use crate::operations::Operation;
    use crate::states::{Column, SchemaState, Table};

    #[derive(Default)]
    struct MockSource {
        saved: RefCell<Vec<Migration>>,
        migrations: Vec<Migration>,
    }

    impl MigrationSource for MockSource {
        fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
            Ok(self.migrations.clone())
        }
        fn save(&self, m: &Migration) -> Result<(), AdapterError> {
            self.saved.borrow_mut().push(m.clone());
            Ok(())
        }
    }

    fn simple_column(name: &str) -> Column {
        Column { name: name.to_string(), col_type: "text".to_string(), nullable: true, default: None, primary_key: false, ..Default::default() }
    }

    fn simple_table(name: &str, cols: &[&str]) -> Table {
        Table {
            name: name.to_string(),
            schema: None,
            columns: cols.iter().map(|c| simple_column(c)).collect(),
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    }

    fn state_with_tables(tables: &[Table]) -> SchemaState {
        let mut s = SchemaState::default();
        for t in tables {
            s.tables.insert(t.name.clone(), t.clone());
        }
        s
    }

    fn migration_with_ops(id: &str, deps: &[&str], ops: Vec<Operation>) -> Migration {
        Migration {
            id: id.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            operations: ops,
        }
    }

    fn migrator_from(migrations: Vec<Migration>) -> Migrator {
        let source = MockSource { migrations, ..MockSource::default() };
        Migrator::new(Arc::new(Config::default()), Box::new(source), Dialect::Postgres).unwrap()
    }

    struct NullExecutor {
        applied: Vec<String>,
        lock_count: usize,
    }

    impl NullExecutor {
        fn empty() -> Self { Self { applied: vec![], lock_count: 0 } }
    }

    impl Executor for NullExecutor {
        fn execute(&mut self, _sql: &str) -> Result<(), ExecutorError> {
            Ok(())
        }
        fn fetch_strings(&mut self, _sql: &str) -> Result<Vec<String>, ExecutorError> {
            Ok(self.applied.clone())
        }
        fn begin(&mut self) -> Result<(), ExecutorError> { Ok(()) }
        fn commit(&mut self) -> Result<(), ExecutorError> { Ok(()) }
        fn rollback(&mut self) -> Result<(), ExecutorError> { Ok(()) }
        fn acquire_lock(&mut self) -> Result<(), ExecutorError> { self.lock_count += 1; Ok(()) }
        fn release_lock(&mut self) -> Result<(), ExecutorError> { self.lock_count -= 1; Ok(()) }
    }

    /// Regression test: migrate() on a multi-migration chain must not fail at validate_plan.
    /// Before the fix, validate_plan called self.replay() to build the full state, then tried
    /// to apply all migrations again — every CreateTable hit a duplicate and returned an error.
    #[test]
    fn migrate_succeeds_on_multi_migration_chain() {
        let users = simple_table("users", &["id"]);
        let posts = simple_table("posts", &["id"]);
        let m = migrator_from(vec![
            migration_with_ops("0001_create_users", &[], vec![Operation::CreateTable { table: users }]),
            migration_with_ops("0002_create_posts", &["0001_create_users"], vec![Operation::CreateTable { table: posts }]),
        ]);
        m.migrate(&mut NullExecutor::empty(), None, None, false)
            .expect("migrate must succeed on a valid multi-migration chain");
    }

    /// Verifies that migrate() acquires and then releases the advisory lock exactly once,
    /// leaving lock_count at zero after a successful run.
    #[test]
    fn migrate_acquires_and_releases_lock() {
        let m = migrator_from(vec![
            migration_with_ops("0001_init", &[], vec![Operation::CreateTable { table: simple_table("t", &["id"]) }]),
        ]);
        let mut ex = NullExecutor::empty();
        m.migrate(&mut ex, None, None, false).expect("migrate should succeed");
        assert_eq!(ex.lock_count, 0, "lock must be released after migrate completes");
    }
    // we build the Migrator manually to keep a reference to the inner saved vec.
    fn migrator_with_source(migrations: Vec<Migration>) -> (Migrator, Arc<MockSourceShared>) {
        let shared = Arc::new(MockSourceShared::default());
        let source = ArcMockSource { shared: Arc::clone(&shared), migrations };
        let mut graph = MigrationGraph::new();
        for m in source.migrations.clone() {
            graph.add(m).unwrap();
        }
        let migrator = Migrator {
            config: Arc::new(Config::default()),
            source: Box::new(source),
            graph,
            diff: DiffEngine::new(),
            dialect: Dialect::Postgres,
        };
        (migrator, shared)
    }

    #[derive(Default)]
    struct MockSourceShared {
        saved: RefCell<Vec<Migration>>,
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
            self.shared.saved.borrow_mut().push(m.clone());
            Ok(())
        }
    }

    #[test]
    fn no_changes_returns_none() {
        let m = migrator_from(vec![]);
        assert!(m.make_migrations("x".into(), SchemaState::default(), false).unwrap().is_none());
    }

    #[test]
    fn new_table_creates_migration() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let result = m.make_migrations("initial".into(), current, false).unwrap();
        let mig = result.unwrap();
        assert_eq!(mig.operations.len(), 1);
        assert!(matches!(&mig.operations[0], Operation::CreateTable { table } if table.name == "users"));
    }

    #[test]
    fn removed_table_creates_migration() {
        let users = simple_table("users", &["id"]);
        let m = migrator_from(vec![migration_with_ops(
            "0001_initial", &[],
            vec![Operation::CreateTable { table: users }],
        )]);
        let result = m.make_migrations("drop_users".into(), SchemaState::default(), false).unwrap();
        let mig = result.unwrap();
        assert_eq!(mig.operations.len(), 1);
        assert!(matches!(&mig.operations[0], Operation::DropTable { table } if table.name == "users"));
    }

    #[test]
    fn added_column_creates_migration() {
        let users_v1 = simple_table("users", &["id"]);
        let m = migrator_from(vec![migration_with_ops(
            "0001_initial", &[],
            vec![Operation::CreateTable { table: users_v1 }],
        )]);
        let current = state_with_tables(&[simple_table("users", &["id", "email"])]);
        let mig = m.make_migrations("add_email".into(), current, false).unwrap().unwrap();
        assert!(mig.operations.iter().any(|op| matches!(op, Operation::AddColumn { column, .. } if column.name == "email")));
    }

    #[test]
    fn dropped_column_creates_migration() {
        let users_v1 = simple_table("users", &["id", "email"]);
        let m = migrator_from(vec![migration_with_ops(
            "0001_initial", &[],
            vec![Operation::CreateTable { table: users_v1 }],
        )]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("drop_email".into(), current, false).unwrap().unwrap();
        assert!(mig.operations.iter().any(|op| matches!(op, Operation::DropColumn { column, .. } if column.name == "email")));
    }

    #[test]
    fn multiple_ops_in_single_migration() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[
            simple_table("users", &["id"]),
            simple_table("posts", &["id"]),
        ]);
        let mig = m.make_migrations("multi".into(), current, false).unwrap().unwrap();
        assert_eq!(mig.operations.len(), 2);
    }

    #[test]
    fn migration_number_increments() {
        let m = migrator_from(vec![migration_with_ops("0001_initial", &[], vec![])]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("add_users".into(), current, false).unwrap().unwrap();
        assert!(mig.id.starts_with("0002_"));
    }

    #[test]
    fn migration_number_empty_graph() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("initial".into(), current, false).unwrap().unwrap();
        assert!(mig.id.starts_with("0001_"));
    }

    #[test]
    fn migration_dependencies_set_to_head() {
        let m = migrator_from(vec![migration_with_ops("0001_initial", &[], vec![])]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("add_users".into(), current, false).unwrap().unwrap();
        assert_eq!(mig.dependencies, vec!["0001_initial"]);
    }

    #[test]
    fn migration_dependencies_empty_graph() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("initial".into(), current, false).unwrap().unwrap();
        assert!(mig.dependencies.is_empty());
    }

    #[test]
    fn dry_run_does_not_save() {
        let (m, shared) = migrator_with_source(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        m.make_migrations("initial".into(), current, true).unwrap().unwrap();
        assert!(shared.saved.borrow().is_empty());
    }

    #[test]
    fn dry_run_still_returns_migration() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let result = m.make_migrations("initial".into(), current, true).unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn conflict_returns_error() {
        // Two heads: 0001_a and 0001_b both have no dependencies
        let m = migrator_from(vec![
            migration_with_ops("0001_a", &[], vec![]),
            migration_with_ops("0001_b", &[], vec![]),
        ]);
        let err = m.make_migrations("x".into(), SchemaState::default(), false).unwrap_err();
        assert!(matches!(err, MigratorError::Graph(GraphError::Conflict)));
    }

    #[test]
    fn replay_error_propagates() {
        // Migration tries to drop a table that was never created
        let bad = migration_with_ops(
            "0001_bad", &[],
            vec![Operation::DropTable { table: simple_table("ghost", &["id"]) }],
        );
        let m = migrator_from(vec![bad]);
        let err = m.make_migrations("x".into(), SchemaState::default(), false).unwrap_err();
        assert!(matches!(err, MigratorError::Replay(_)));
    }

    #[test]
    fn make_migrations_name_used_in_id() {
        let m = migrator_from(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        let mig = m.make_migrations("my_migration".into(), current, false).unwrap().unwrap();
        assert!(mig.id.contains("my_migration"));
    }

    #[test]
    fn incremental_diff() {
        let users = simple_table("users", &["id"]);
        let posts = simple_table("posts", &["id"]);
        let m = migrator_from(vec![
            migration_with_ops("0001_create_users", &[], vec![Operation::CreateTable { table: users }]),
            migration_with_ops("0002_create_posts", &["0001_create_users"], vec![Operation::CreateTable { table: posts }]),
        ]);
        let current = state_with_tables(&[
            simple_table("users", &["id"]),
            simple_table("posts", &["id"]),
            simple_table("comments", &["id"]),
        ]);
        let mig = m.make_migrations("add_comments".into(), current, false).unwrap().unwrap();
        assert_eq!(mig.dependencies, vec!["0002_create_posts"]);
        assert_eq!(mig.operations.len(), 1);
        assert!(matches!(&mig.operations[0], Operation::CreateTable { table } if table.name == "comments"));
    }

    /// Validates that passing a current state with two primary key columns returns a Config error.
    #[test]
    fn make_migrations_rejects_multiple_pk_columns() {
        let m = migrator_from(vec![]);
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![
                Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
                Column { name: "alt_id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
            ],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let mut current = SchemaState::default();
        current.tables.insert("users".to_string(), table);
        let err = m.make_migrations("bad".into(), current, false).unwrap_err();
        assert!(matches!(err, MigratorError::Config(_)));
    }

    /// Validates that save() is called exactly once when dry_run is false.
    #[test]
    fn save_called_when_not_dry_run() {
        let (m, shared) = migrator_with_source(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        m.make_migrations("initial".into(), current, false).unwrap().unwrap();
        assert_eq!(shared.saved.borrow().len(), 1);
    }

    #[test]
    fn validate_plan_rejects_irreversible_on_backward() {
        let m = migrator_from(vec![]);
        let migrations = vec![Migration {
            id: "0001_x".into(),
            dependencies: vec![],
            operations: vec![Operation::Statement { up: "SELECT 1".into(), down: None }],
        }];
        let err = m.validate_plan(&migrations, false).unwrap_err();
        assert!(matches!(err, MigratorError::Config(_)));
    }

    #[test]
    fn validate_plan_accepts_reversible_on_backward() {
        let m = migrator_from(vec![]);
        let migrations = vec![Migration {
            id: "0001_x".into(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable { table: simple_table("users", &["id"]) }],
        }];
        assert!(m.validate_plan(&migrations, false).is_ok());
    }

    #[test]
    fn validate_plan_rejects_fk_to_unknown_table() {
        use crate::states::ForeignKey;
        let m = migrator_from(vec![]);
        let table = Table {
            name: "posts".into(),
            schema: None,
            columns: vec![simple_column("id")],
            foreign_keys: vec![ForeignKey {
                name: "posts_user_id_fk".into(),
                from_column: "user_id".into(),
                to_table: "users".into(),
                to_column: "id".into(),
            }],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let migrations = vec![Migration {
            id: "0001_x".into(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable { table }],
        }];
        let err = m.validate_plan(&migrations, true).unwrap_err();
        assert!(matches!(err, MigratorError::Config(s) if s.contains("unknown table")));
    }

    #[test]
    fn validate_plan_accepts_fk_to_existing_table_in_prior_migration() {
        use crate::states::ForeignKey;
        // Both migrations must be passed so validate_plan can build state incrementally.
        let m = migrator_from(vec![]);
        let users = simple_table("users", &["id"]);
        let posts = Table {
            name: "posts".into(),
            schema: None,
            columns: vec![simple_column("id")],
            foreign_keys: vec![ForeignKey {
                name: "posts_user_id_fk".into(),
                from_column: "user_id".into(),
                to_table: "users".into(),
                to_column: "id".into(),
            }],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let migrations = vec![
            Migration {
                id: "0001_create_users".into(),
                dependencies: vec![],
                operations: vec![Operation::CreateTable { table: users }],
            },
            Migration {
                id: "0002_create_posts".into(),
                dependencies: vec!["0001_create_users".into()],
                operations: vec![Operation::CreateTable { table: posts }],
            },
        ];
        assert!(m.validate_plan(&migrations, true).is_ok());
    }

    #[test]
    fn validate_plan_rejects_duplicate_index_name() {
        use crate::states::Index;
        // All migrations must be passed together so validate_plan has full context.
        let m = migrator_from(vec![]);
        let migrations = vec![
            Migration {
                id: "0001_create_users".into(),
                dependencies: vec![],
                operations: vec![Operation::CreateTable { table: simple_table("users", &["id"]) }],
            },
            Migration {
                id: "0002_idx_a".into(),
                dependencies: vec!["0001_create_users".into()],
                operations: vec![Operation::AddIndex {
                    table_name: "users".into(),
                    index: Index { name: "users_name_idx".into(), columns: vec!["id".into()], unique: false, predicate: None },
                }],
            },
            Migration {
                id: "0003_idx_b".into(),
                dependencies: vec!["0002_idx_a".into()],
                operations: vec![Operation::AddIndex {
                    table_name: "users".into(),
                    index: Index { name: "users_name_idx".into(), columns: vec!["id".into()], unique: false, predicate: None },
                }],
            },
        ];
        let err = m.validate_plan(&migrations, true).unwrap_err();
        assert!(matches!(err, MigratorError::Config(s) if s.contains("duplicate index")));
    }

    #[test]
    fn graph_rejects_invalid_migration_id() {
        use crate::graphs::MigrationGraph;
        let err = MigrationGraph::validate_id("0001 bad id!").unwrap_err();
        assert!(matches!(err, crate::graphs::GraphError::InvalidId(_)));
        assert!(MigrationGraph::validate_id("0001_good_id").is_ok());
    }

    #[test]
    fn validate_duplicate_column_in_schema_state() {        use crate::states::Column;
        let table = Table {
            name: "users".into(),
            schema: None,
            columns: vec![
                Column { name: "id".into(), col_type: "integer".into(), nullable: false, default: None, primary_key: true, ..Default::default() },
                Column { name: "id".into(), col_type: "text".into(), nullable: true, default: None, primary_key: false, ..Default::default() },
            ],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let mut state = SchemaState::default();
        state.tables.insert("users".into(), table);
        let err = state.validate().unwrap_err();
        assert!(err.contains("duplicate column"));
    }

    /// migrate() with an Invoke op and no invoker returns MigratorError::Invoke(NoInvoker).
    #[test]
    fn invoke_op_without_invoker_returns_error() {
        let m = migrator_from(vec![migration_with_ops(
            "0001_seed", &[],
            vec![Operation::Invoke { up: "echo hello".into(), down: None }],
        )]);
        let err = m.migrate(&mut NullExecutor::empty(), None, None, false).unwrap_err();
        assert!(matches!(err, MigratorError::Invoke(InvokerError::NoInvoker)));
    }

    /// migrate() with a MockInvoker that does not commit records the invoke call and the correct
    /// begin/commit sequence around the migration.
    #[test]
    fn invoke_op_with_no_commit_invoker_is_called() {
        use std::cell::Cell;

        struct NoCommitInvoker {
            called: Cell<bool>,
        }
        impl Invoker for NoCommitInvoker {
            fn must_commit(&self) -> bool { false }
            fn invoke(&self, _command: &str, _tx: &mut dyn Executor) -> Result<(), InvokerError> {
                self.called.set(true);
                Ok(())
            }
        }

        let inv = NoCommitInvoker { called: Cell::new(false) };
        let m = migrator_from(vec![migration_with_ops(
            "0001_seed", &[],
            vec![Operation::Invoke { up: "noop".into(), down: None }],
        )]);
        m.migrate(&mut NullExecutor::empty(), Some(&inv), None, false).unwrap();
        assert!(inv.called.get(), "invoker must have been called");
    }

    /// migrate() with must_commit invoker emits COMMIT then BEGIN around the invoke call.
    #[test]
    fn invoke_op_with_commit_invoker_commits_before_invoke() {
        use std::cell::RefCell;

        struct CommitInvoker {
            events: RefCell<Vec<&'static str>>,
        }
        impl Invoker for CommitInvoker {
            fn must_commit(&self) -> bool { true }
            fn invoke(&self, _command: &str, _tx: &mut dyn Executor) -> Result<(), InvokerError> {
                self.events.borrow_mut().push("invoke");
                Ok(())
            }
        }

        struct RecordingExecutor {
            events: RefCell<Vec<&'static str>>,
        }
        impl Executor for RecordingExecutor {
            fn execute(&mut self, _sql: &str) -> Result<(), ExecutorError> { Ok(()) }
            fn fetch_strings(&mut self, _sql: &str) -> Result<Vec<String>, ExecutorError> { Ok(vec![]) }
            fn begin(&mut self) -> Result<(), ExecutorError> { self.events.borrow_mut().push("begin"); Ok(()) }
            fn commit(&mut self) -> Result<(), ExecutorError> { self.events.borrow_mut().push("commit"); Ok(()) }
            fn rollback(&mut self) -> Result<(), ExecutorError> { self.events.borrow_mut().push("rollback"); Ok(()) }
        }

        let shared_events: RefCell<Vec<&'static str>> = RefCell::new(vec![]);
        let inv = CommitInvoker { events: RefCell::new(vec![]) };

        let mut exec = RecordingExecutor { events: RefCell::new(vec![]) };
        let m = migrator_from(vec![migration_with_ops(
            "0001_seed", &[],
            vec![Operation::Invoke { up: "noop".into(), down: None }],
        )]);
        m.migrate(&mut exec, Some(&inv), None, false).unwrap();

        let exec_events = exec.events.borrow();
        let inv_events = inv.events.borrow();
        // Expect: begin (migration start), commit (before invoke), begin (after invoke), commit (end of migration)
        assert!(exec_events.iter().position(|&e| e == "commit").unwrap()
            < exec_events.iter().rposition(|&e| e == "begin").unwrap(),
            "commit must come before the re-begin for must_commit invoker");
        assert_eq!(inv_events.as_slice(), &["invoke"]);
        let _ = shared_events;
    }

    #[test]
    fn show_migrations_marks_applied_and_pending() {
        let users = simple_table("users", &["id"]);
        let m = migrator_from(vec![
            migration_with_ops("0001_create_users", &[], vec![Operation::CreateTable { table: users }]),
            migration_with_ops("0002_add_email", &["0001_create_users"], vec![]),
        ]);
        let mut exec = NullExecutor { applied: vec!["0001_create_users".to_string()], lock_count: 0 };
        let rows = m.show_migrations(&mut exec).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0], ("0001_create_users".to_string(), true));
        assert_eq!(rows[1], ("0002_add_email".to_string(), false));
    }

    #[test]
    fn show_migrations_empty_graph() {
        let m = migrator_from(vec![]);
        let mut exec = NullExecutor::empty();
        let rows = m.show_migrations(&mut exec).unwrap();
        assert!(rows.is_empty());
    }
}


