use std::collections::{BTreeMap, HashSet, VecDeque};
use std::future::Future;
use std::sync::{Arc, Mutex};

use gaman_core::managed_rows::{ManagedRow, ManagedRows, ManagedValue};
use gaman_core::schema::Schema;
use gaman_core::{
    ApplyCommand, BoxFuture, Command, Dialect, Executor, ExecutorError, InspectionError, Migration,
    MigrationRunner, MigrationStore, OfflinePlanner, RepairOptions, SchemaInspector, StoreError,
    TrackingError, TrackingStore,
};

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("build test runtime")
        .block_on(future)
}

#[derive(Clone)]
struct SharedStore(Arc<Mutex<Vec<Migration>>>);

impl MigrationStore for SharedStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
        let migrations = self.0.lock().expect("migration store").clone();
        Box::pin(async move { Ok(migrations) })
    }

    fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
        self.0
            .lock()
            .expect("migration store")
            .push(migration.clone());
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct SharedTracking {
    applied: Arc<Mutex<HashSet<String>>>,
    records: Arc<Mutex<Vec<String>>>,
    unrecords: Arc<Mutex<Vec<String>>>,
}

impl TrackingStore for SharedTracking {
    fn install<'a>(
        &'a self,
        _: Dialect,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async { Ok(()) })
    }

    fn applied_ids<'a>(
        &'a self,
        _: Dialect,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>> {
        let applied = self.applied.lock().expect("tracking state").clone();
        Box::pin(async move { Ok(applied) })
    }

    fn record<'a>(
        &'a self,
        _: Dialect,
        id: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        self.records
            .lock()
            .expect("record log")
            .push(id.to_string());
        self.applied
            .lock()
            .expect("tracking state")
            .insert(id.to_string());
        Box::pin(async { Ok(()) })
    }

    fn unrecord<'a>(
        &'a self,
        _: Dialect,
        id: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        self.unrecords
            .lock()
            .expect("unrecord log")
            .push(id.to_string());
        self.applied.lock().expect("tracking state").remove(id);
        Box::pin(async { Ok(()) })
    }
}

#[derive(Clone)]
struct CheckedExecutor {
    affected: SharedAffectedResults,
    fetched: SharedFetchedResults,
    inspected: Schema,
    log: Arc<Mutex<Vec<String>>>,
}

type SharedAffectedResults = Arc<Mutex<VecDeque<Result<u64, ExecutorError>>>>;
type SharedFetchedResults = Arc<Mutex<VecDeque<Result<Vec<String>, ExecutorError>>>>;

impl Executor for CheckedExecutor {
    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.log.lock().expect("executor log").push(sql.to_string());
        Box::pin(async { Ok(()) })
    }

    fn execute_affected<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<u64, ExecutorError>> {
        self.log.lock().expect("executor log").push(sql.to_string());
        let result = self
            .affected
            .lock()
            .expect("affected results")
            .pop_front()
            .unwrap_or(Ok(1));
        Box::pin(async move { result })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        self.log.lock().expect("executor log").push(sql.to_string());
        let result = self
            .fetched
            .lock()
            .expect("fetch results")
            .pop_front()
            .unwrap_or_else(|| Ok(Vec::new()));
        Box::pin(async move { result })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.log
            .lock()
            .expect("executor log")
            .push("BEGIN".to_string());
        Box::pin(async { Ok(()) })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.log
            .lock()
            .expect("executor log")
            .push("COMMIT".to_string());
        Box::pin(async { Ok(()) })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        self.log
            .lock()
            .expect("executor log")
            .push("ROLLBACK".to_string());
        Box::pin(async { Ok(()) })
    }
}

impl SchemaInspector for CheckedExecutor {
    fn inspect<'a>(
        &'a mut self,
        _: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        let inspected = self.inspected.clone();
        Box::pin(async move { Ok(inspected) })
    }
}

struct UnsupportedExecutor;

impl Executor for UnsupportedExecutor {
    fn execute<'a>(&'a mut self, _: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async { Ok(()) })
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

impl SchemaInspector for UnsupportedExecutor {
    fn inspect<'a>(
        &'a mut self,
        _: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        Box::pin(async { Ok(Schema::default()) })
    }
}

/// Builds a structural migration followed by one checked managed-row insertion.
fn managed_migrations() -> (Migration, Migration) {
    let table = Schema::from_sql_str(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL)",
        Dialect::Postgres,
    )
    .expect("table schema");
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(table.clone(), &[])
        .expect("initial plan")
        .expect("initial migration");
    let mut managed = table;
    managed.managed_rows.insert(
        "task_lanes".to_string(),
        ManagedRows {
            rows: vec![ManagedRow {
                values: BTreeMap::from([
                    ("id".to_string(), ManagedValue("approval".into())),
                    ("name".to_string(), ManagedValue("review".into())),
                ]),
            }],
        },
    );
    let managed = managed.prepare(Dialect::Postgres).expect("managed schema");
    let row = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial.clone()])
        .make_migration(managed, &[])
        .expect("row plan")
        .expect("row migration");
    (initial, row)
}

/// Returns the inspected table shape used by deterministic live-contract tests.
fn table_schema() -> Schema {
    Schema::from_sql_str(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL)",
        Dialect::Postgres,
    )
    .expect("table schema")
}

/// Builds a checked executor with deterministic affected-row and query outcomes.
fn executor(
    affected: impl IntoIterator<Item = Result<u64, ExecutorError>>,
    fetched: impl IntoIterator<Item = Result<Vec<String>, ExecutorError>>,
    log: Arc<Mutex<Vec<String>>>,
) -> CheckedExecutor {
    CheckedExecutor {
        affected: Arc::new(Mutex::new(affected.into_iter().collect())),
        fetched: Arc::new(Mutex::new(fetched.into_iter().collect())),
        inspected: table_schema(),
        log,
    }
}

fn tracking(applied: impl IntoIterator<Item = String>) -> SharedTracking {
    SharedTracking {
        applied: Arc::new(Mutex::new(applied.into_iter().collect())),
        records: Arc::new(Mutex::new(Vec::new())),
        unrecords: Arc::new(Mutex::new(Vec::new())),
    }
}

fn apply_command(target: Option<String>) -> Command {
    Command::Apply(ApplyCommand::Execute {
        target,
        fake: false,
        fake_verified: false,
        schemas: Vec::new(),
    })
}

/// Verifies one affected managed row commits and records the migration through
/// the complete runner and engine lifecycle.
#[test]
fn exactly_one_affected_row_commits() {
    block_on(async {
        let (initial, row) = managed_migrations();
        let tracking = tracking([initial.id.clone()]);
        let records = Arc::clone(&tracking.records);
        let log = Arc::new(Mutex::new(Vec::new()));
        let executor = executor([Ok(1)], [], Arc::clone(&log));
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(vec![initial, row.clone()]))),
            tracking,
            executor,
        );

        runner
            .run_command(&apply_command(None))
            .await
            .expect("apply managed row");
        assert_eq!(records.lock().expect("records").as_slice(), [row.id]);
        let log = log.lock().expect("executor log");
        assert!(log.iter().any(|entry| entry.starts_with("INSERT INTO")));
        assert!(log.iter().any(|entry| entry == "COMMIT"));
    });
}

/// Verifies stale and non-unique managed writes roll back and never record
/// migration success.
#[test]
fn affected_row_mismatches_rollback_without_recording() {
    for affected in [0, 2] {
        block_on(async {
            let (initial, row) = managed_migrations();
            let tracking = tracking([initial.id.clone()]);
            let records = Arc::clone(&tracking.records);
            let log = Arc::new(Mutex::new(Vec::new()));
            let executor = executor([Ok(affected)], [], Arc::clone(&log));
            let mut runner = MigrationRunner::new(
                Dialect::Postgres,
                SharedStore(Arc::new(Mutex::new(vec![initial, row]))),
                tracking,
                executor,
            );

            let error = runner
                .run_command(&apply_command(None))
                .await
                .expect_err("affected-row mismatch");
            assert!(error.to_string().contains(if affected == 0 {
                "precondition"
            } else {
                "integrity"
            }));
            assert!(records.lock().expect("records").is_empty());
            assert!(
                log.lock()
                    .expect("executor log")
                    .iter()
                    .any(|entry| entry == "ROLLBACK")
            );
        });
    }
}

/// Verifies the default external-executor capability fails only when a managed
/// row operation is applied and leaves tracking unchanged.
#[test]
fn unsupported_affected_execution_fails_clearly() {
    block_on(async {
        let (initial, row) = managed_migrations();
        let tracking = tracking([initial.id.clone()]);
        let records = Arc::clone(&tracking.records);
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(vec![initial, row]))),
            tracking,
            UnsupportedExecutor,
        );

        let error = runner
            .run_command(&apply_command(None))
            .await
            .expect_err("unsupported checked execution");
        assert!(
            error
                .to_string()
                .contains("affected-row execution is not supported")
        );
        assert!(records.lock().expect("records").is_empty());
    });
}

/// Verifies rollback uses the same checked precondition and does not unrecord a
/// migration when its expected managed row is stale.
#[test]
fn rollback_stale_row_keeps_tracking_record() {
    block_on(async {
        let (initial, row) = managed_migrations();
        let tracking = tracking([initial.id.clone(), row.id.clone()]);
        let unrecords = Arc::clone(&tracking.unrecords);
        let log = Arc::new(Mutex::new(Vec::new()));
        let executor = executor([Ok(0)], [], Arc::clone(&log));
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(vec![initial.clone(), row]))),
            tracking,
            executor,
        );

        let error = runner
            .run_command(&apply_command(Some(initial.id)))
            .await
            .expect_err("stale rollback");
        assert!(error.to_string().contains("precondition"));
        assert!(unrecords.lock().expect("unrecords").is_empty());
        assert!(
            log.lock()
                .expect("executor log")
                .iter()
                .any(|entry| entry == "ROLLBACK")
        );
    });
}

fn observed(name: &str) -> String {
    serde_json::json!({"id": "approval", "name": name}).to_string()
}

fn repair_options(apply: bool) -> RepairOptions {
    RepairOptions {
        apply,
        allow_pending: false,
        allow_partial: false,
        sql_only: false,
    }
}

/// Verifies runner verification projects observed-value repairs, dry-run does
/// not write, applied repair converges, and a concurrent mutation fails safely.
#[test]
fn verify_and_repair_use_checked_observed_state() {
    block_on(async {
        let (initial, row) = managed_migrations();
        let migrations = vec![initial.clone(), row.clone()];
        let applied = [initial.id.clone(), row.id.clone()];

        let dry_log = Arc::new(Mutex::new(Vec::new()));
        let mut dry_runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(migrations.clone()))),
            tracking(applied.clone()),
            executor([], [Ok(vec![observed("tampered")])], Arc::clone(&dry_log)),
        );
        let result = dry_runner
            .run_command(&Command::Repair {
                schemas: Vec::new(),
                options: repair_options(false),
            })
            .await
            .expect("dry-run repair");
        assert!(matches!(result, gaman_core::CommandResult::Repair(report) if !report.applied));
        assert!(
            !dry_log
                .lock()
                .expect("dry log")
                .iter()
                .any(|entry| entry.starts_with("UPDATE"))
        );

        let apply_log = Arc::new(Mutex::new(Vec::new()));
        let mut apply_runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(migrations.clone()))),
            tracking(applied.clone()),
            executor(
                [Ok(1)],
                [Ok(vec![observed("tampered")]), Ok(vec![observed("review")])],
                Arc::clone(&apply_log),
            ),
        );
        let result = apply_runner
            .run_command(&Command::Repair {
                schemas: Vec::new(),
                options: repair_options(true),
            })
            .await
            .expect("applied repair");
        assert!(
            matches!(result, gaman_core::CommandResult::Repair(report) if report.applied && report.verification.findings.is_empty())
        );
        assert!(
            apply_log
                .lock()
                .expect("apply log")
                .iter()
                .any(|entry| entry.starts_with("UPDATE"))
        );

        let mut concurrent_runner = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(migrations))),
            tracking(applied),
            executor(
                [Ok(0)],
                [Ok(vec![observed("tampered")])],
                Arc::new(Mutex::new(Vec::new())),
            ),
        );
        let error = concurrent_runner
            .run_command(&Command::Repair {
                schemas: Vec::new(),
                options: repair_options(true),
            })
            .await
            .expect_err("concurrent repair must fail");
        assert!(error.to_string().contains("precondition"));
    });
}

/// Verifies matching existing managed data can be fake-adopted while missing
/// data is refused without recording the migration.
#[test]
fn verified_fake_adopts_only_matching_managed_rows() {
    block_on(async {
        let (initial, row) = managed_migrations();
        let migrations = vec![initial.clone(), row.clone()];

        let accepted_tracking = tracking([initial.id.clone()]);
        let accepted_records = Arc::clone(&accepted_tracking.records);
        let mut accepted = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(migrations.clone()))),
            accepted_tracking,
            executor(
                [],
                [Ok(vec![observed("review")])],
                Arc::new(Mutex::new(Vec::new())),
            ),
        );
        accepted
            .run_command(&Command::Apply(ApplyCommand::Execute {
                target: Some(row.id.clone()),
                fake: false,
                fake_verified: true,
                schemas: Vec::new(),
            }))
            .await
            .expect("verified fake adoption");
        assert_eq!(
            accepted_records
                .lock()
                .expect("accepted records")
                .as_slice(),
            std::slice::from_ref(&row.id)
        );

        let refused_tracking = tracking([initial.id]);
        let refused_records = Arc::clone(&refused_tracking.records);
        let mut refused = MigrationRunner::new(
            Dialect::Postgres,
            SharedStore(Arc::new(Mutex::new(migrations))),
            refused_tracking,
            executor([], [Ok(Vec::new())], Arc::new(Mutex::new(Vec::new()))),
        );
        let error = refused
            .run_command(&Command::Apply(ApplyCommand::Execute {
                target: Some(row.id),
                fake: false,
                fake_verified: true,
                schemas: Vec::new(),
            }))
            .await
            .expect_err("missing managed row must refuse adoption");
        assert!(error.to_string().contains("drift finding"));
        assert!(refused_records.lock().expect("refused records").is_empty());
    });
}
