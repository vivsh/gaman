use std::sync::Arc;

use gaman::adapters::VecAdapter;
use gaman::dialects::Dialect;
use gaman::executor::{Executor, ExecutorError};
use gaman::migrations::Migration;
use gaman::migrator::Migrator;
use gaman::conf::Config;
use gaman::states::SchemaState;

use super::fixtures;

/// Implemented by each database-specific test file.
/// All test scenario functions take `&mut dyn DbHarness` so the same test
/// logic runs for every supported database.
pub trait DbHarness {
    fn dialect(&self) -> Dialect;
    /// Returns a mutable reference to an executor backed by an open transaction.
    /// After each test, the transaction is rolled back (via `reset`).
    fn executor(&mut self) -> &mut dyn Executor;
    /// Drop and recreate the isolated test schema, rolling back any open transaction.
    fn reset(&mut self);
    /// Count rows in a table. Returns 0 if the table does not exist.
    fn raw_count(&mut self, table: &str) -> usize;
    /// Returns true if the given table exists in the test schema.
    fn table_exists(&mut self, table: &str) -> bool;
    /// Count rows in the migration tracking table.
    fn tracking_count(&mut self) -> usize;
    /// Returns the name of the current isolated test schema (e.g. "gaman_test_0").
    /// Used to pass the correct schema name to inspect_schema.
    fn current_schema(&self) -> String {
        "public".to_string()
    }
    /// Introspect the live database for the given schema and return a SchemaState.
    /// Backends that do not support introspection return None and the test is skipped.
    fn inspect_schema(&mut self, schema: &str) -> Option<SchemaState> {
        let _ = schema;
        None
    }
}

fn migrator(migrations: Vec<Migration>, dialect: Dialect) -> Migrator {
    let config = Arc::new(Config::default());
    let source = Box::new(VecAdapter::new(migrations));
    Migrator::new(config, source, dialect).expect("migrator construction failed")
}

pub fn test_forward_apply(h: &mut dyn DbHarness) {
    h.reset();
    let m = migrator(fixtures::three_migration_chain(), h.dialect());
    m.migrate(h.executor(), None, None, false).expect("migrate failed");

    assert_eq!(h.tracking_count(), 3);
    assert!(h.table_exists("users"));
    assert!(h.table_exists("posts"));
}

pub fn test_rollback_to_target(h: &mut dyn DbHarness) {
    h.reset();
    let chain = fixtures::three_migration_chain();
    let m = migrator(chain.clone(), h.dialect());
    m.migrate(h.executor(), None, None, false).expect("forward migrate failed");

    m.migrate(h.executor(), None, Some("0001_create_users"), false).expect("rollback failed");

    assert_eq!(h.tracking_count(), 1);
    assert!(h.table_exists("users"));
    assert!(!h.table_exists("posts"));
}

pub fn test_fake_apply(h: &mut dyn DbHarness) {
    h.reset();
    let m = migrator(fixtures::three_migration_chain(), h.dialect());
    m.migrate(h.executor(), None, None, true).expect("fake migrate failed");

    assert_eq!(h.tracking_count(), 3);
    // DDL was not executed — tables should not exist
    assert!(!h.table_exists("users"));
    assert!(!h.table_exists("posts"));
}

pub fn test_fake_rollback(h: &mut dyn DbHarness) {
    h.reset();
    let chain = fixtures::three_migration_chain();
    let m = migrator(chain.clone(), h.dialect());
    // Real forward apply first
    m.migrate(h.executor(), None, None, false).expect("forward failed");
    assert_eq!(h.tracking_count(), 3);

    // Fake rollback to first migration
    m.migrate(h.executor(), None, Some("0001_create_users"), true).expect("fake rollback failed");
    assert_eq!(h.tracking_count(), 1);
    // Tables still exist because DDL was not reversed
    assert!(h.table_exists("posts"));
}

pub fn test_bootstrap_idempotent(h: &mut dyn DbHarness) {
    h.reset();
    let m = migrator(vec![], h.dialect());
    m.install(h.executor()).expect("first install failed");
    m.install(h.executor()).expect("second install failed");
}

pub fn test_partial_failure_rolls_back(h: &mut dyn DbHarness) {
    h.reset();
    let mut bad_chain = fixtures::three_migration_chain();
    // Inject a syntactically invalid SQL statement into the third migration
    bad_chain[2].operations.push(gaman::operations::Operation::Statement {
        up: "THIS IS NOT VALID SQL !!!".into(),
        down: None,
    });
    let m = migrator(bad_chain, h.dialect());
    let result = m.migrate(h.executor(), None, None, false);
    assert!(result.is_err(), "expected migrate to fail on bad SQL");

    // Per-migration transactions: migrations 1 and 2 committed, migration 3 rolled back.
    assert!(!h.table_exists("posts"));
    assert_eq!(h.tracking_count(), 2);
    h.reset();
    assert_eq!(h.tracking_count(), 0);
}

pub fn test_duplicate_record_skipped(h: &mut dyn DbHarness) {
    h.reset();
    let chain = fixtures::three_migration_chain();
    let m = migrator(chain, h.dialect());
    // Apply once
    m.migrate(h.executor(), None, None, false).expect("first apply failed");
    assert_eq!(h.tracking_count(), 3);

    // Apply again — nothing should change (already applied)
    m.migrate(h.executor(), None, None, false).expect("second apply failed");
    assert_eq!(h.tracking_count(), 3);
}

pub fn test_drifted_tracking_reapplied(h: &mut dyn DbHarness) {
    h.reset();
    let chain = fixtures::three_migration_chain();
    let m = migrator(chain, h.dialect());
    m.migrate(h.executor(), None, None, false).expect("apply failed");
    assert_eq!(h.tracking_count(), 3);

    // Simulate drift: manually remove the second tracking row while its DDL is still applied.
    h.executor()
        .execute("DELETE FROM gaman_migrations WHERE id = '0002_add_email'")
        .expect("manual delete failed");
    assert_eq!(h.tracking_count(), 2);

    // Reconcile with fake=true: re-records the missing row without re-executing DDL.
    // Attempting fake=false here would fail because the column already exists.
    m.migrate(h.executor(), None, None, true).expect("fake reconcile failed");
    assert_eq!(h.tracking_count(), 3);
}

pub fn test_invalid_graph_rejected(h: &mut dyn DbHarness) {
    h.reset();
    let mut broken = fixtures::three_migration_chain();
    // Point second migration at a non-existent dependency
    broken[1].dependencies = vec!["9999_ghost".into()];
    // Migrator::new loads the graph and should fail on unknown dependency
    let config = Arc::new(Config::default());
    let source = Box::new(VecAdapter::new(broken));
    let result = Migrator::new(config, source, h.dialect());
    assert!(result.is_err(), "expected graph error for unknown dependency");
}

/// A `MockExecutor` that records all SQL sent to it without hitting a DB.
/// Useful for unit-testing CLI paths.
#[derive(Default)]
pub struct MockExecutor {
    pub log: Vec<String>,
    /// If set, the next `execute` call returns this error.
    pub inject_error: Option<String>,
}

impl MockExecutor {
    pub fn new() -> Self { Self::default() }
}

impl Executor for MockExecutor {
    fn execute(&mut self, sql: &str) -> Result<(), ExecutorError> {
        if let Some(msg) = self.inject_error.take() {
            return Err(ExecutorError::Execute(msg));
        }
        self.log.push(sql.to_string());
        Ok(())
    }

    fn fetch_strings(&mut self, _sql: &str) -> Result<Vec<String>, ExecutorError> {
        Ok(vec![])
    }

    fn begin(&mut self) -> Result<(), ExecutorError> {
        self.log.push("BEGIN".into());
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ExecutorError> {
        self.log.push("COMMIT".into());
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ExecutorError> {
        self.log.push("ROLLBACK".into());
        Ok(())
    }
}

// --- schema comparison helpers ---

fn strip_schema(key: &str) -> &str {
    key.rfind('.').map(|i| &key[i + 1..]).unwrap_or(key)
}

// Checks that every table key present in `replay` also exists in `inspected`,
// and that columns, FK destinations, and index names match. We intentionally
// avoid comparing default values and col_type strings byte-for-byte because
// the live DB often represents those differently (e.g. `integer` vs `int4`).
fn assert_tables_compatible(replay: &SchemaState, inspected: &SchemaState) {
    // Build a bare-name → table map so we can look up regardless of schema prefix.
    let inspected_by_name: std::collections::HashMap<&str, _> = inspected
        .tables
        .iter()
        .map(|(k, v)| (strip_schema(k.as_str()), v))
        .collect();

    for (key, replay_table) in &replay.tables {
        let bare = strip_schema(key.as_str());
        let live = inspected_by_name.get(bare).unwrap_or_else(|| {
            panic!("table '{bare}' present in replay state but missing from inspect_db output")
        });

        // Column names, in order
        let replay_cols: Vec<&str> = replay_table.columns.iter().map(|c| c.name.as_str()).collect();
        let live_cols: Vec<&str> = live.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            replay_cols, live_cols,
            "column names mismatch for table '{key}': replay={replay_cols:?} live={live_cols:?}"
        );

        // nullable flag per column
        for rc in &replay_table.columns {
            if let Some(lc) = live.columns.iter().find(|c| c.name == rc.name) {
                assert_eq!(
                    rc.nullable, lc.nullable,
                    "nullable mismatch on '{key}.{}': replay={} live={}",
                    rc.name, rc.nullable, lc.nullable
                );
            }
        }

        // FK destinations (to_table + to_column) — strip schema prefix so
        // replay's "users" matches inspect_db's "gaman_test_0.users".
        let replay_fk_targets: Vec<(&str, &str)> = replay_table
            .foreign_keys
            .iter()
            .map(|fk| (strip_schema(fk.to_table.as_str()), fk.to_column.as_str()))
            .collect();
        let live_fk_targets: Vec<(&str, &str)> = live
            .foreign_keys
            .iter()
            .map(|fk| (strip_schema(fk.to_table.as_str()), fk.to_column.as_str()))
            .collect();
        assert_eq!(
            replay_fk_targets, live_fk_targets,
            "FK targets mismatch for table '{key}'"
        );

        // Index names
        let mut replay_idx: Vec<&str> = replay_table.indexes.iter().map(|i| i.name.as_str()).collect();
        let mut live_idx: Vec<&str> = live.indexes.iter().map(|i| i.name.as_str()).collect();
        replay_idx.sort_unstable();
        live_idx.sort_unstable();
        assert_eq!(
            replay_idx, live_idx,
            "index names mismatch for table '{key}'"
        );
    }

    // Every table in replay must appear in inspected (already checked above);
    // also verify no extra tables snuck in the other direction (excluding gaman_migrations).
    let replay_by_name: std::collections::HashSet<&str> = replay
        .tables
        .keys()
        .map(|k| strip_schema(k.as_str()))
        .collect();
    for key in inspected.tables.keys() {
        let bare = strip_schema(key.as_str());
        assert!(
            replay_by_name.contains(bare),
            "table '{bare}' found in inspect_db output but not in replay state"
        );
    }
}

/// Apply the three-migration chain, replay the state from the graph, introspect
/// the live DB, then assert the two representations are structurally consistent.
pub fn test_replay_matches_inspect_db(h: &mut dyn DbHarness) {
    h.reset();

    let chain = fixtures::three_migration_chain();
    let m = migrator(chain, h.dialect());
    m.migrate(h.executor(), None, None, false).expect("migrate failed");

    // Replay: reconstruct state by applying all operations in topological order.
    let order = m.graph.topological_order().expect("topological order failed");
    let mut replay = SchemaState::default();
    for id in &order {
        if let Some(migration) = m.graph.get(id) {
            for op in &migration.operations {
                replay.apply(op).expect("replay apply failed");
            }
        }
    }

    // Normalise inline FKs / check constraints so the replay state is canonical.
    replay.normalize();

    // Introspect — skip if the backend doesn't support it.
    let schema = h.current_schema();
    let inspected = match h.inspect_schema(&schema) {
        Some(s) => s,
        None => return,
    };

    assert_tables_compatible(&replay, &inspected);
}
