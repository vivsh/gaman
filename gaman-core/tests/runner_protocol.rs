//! Contract tests for the portable runner protocol and migration snapshots.

use std::cell::Cell;
use std::collections::HashSet;
use std::future::Future;
use std::marker::PhantomData;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::task::{Context, Poll, Wake, Waker};

use gaman_core::runner::EntityFilter;
use gaman_core::{
    BoxFuture, COMMAND_PROTOCOL_VERSION, Command, CommandEnvelope, CommandError, CommandResponse,
    CommandResult, Dialect, Executor, ExecutorError, MakeCommand, MakeResult, Migration,
    MigrationCatalog, MigrationRunner, MigrationStore, StoreError, TrackingError, TrackingStore,
};

struct CountingStore {
    loads: Arc<AtomicUsize>,
}

impl MigrationStore for CountingStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
        Box::pin(async move {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(Vec::new())
        })
    }

    fn save<'a>(&'a self, _: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async { Ok(()) })
    }
}

#[derive(Default)]
struct EmptyTracking;

impl TrackingStore for EmptyTracking {
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
        Box::pin(async { Ok(HashSet::new()) })
    }

    fn record<'a>(
        &'a self,
        _: Dialect,
        _: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async { Ok(()) })
    }

    fn unrecord<'a>(
        &'a self,
        _: Dialect,
        _: &'a str,
        _: &'a mut dyn Executor,
    ) -> BoxFuture<'a, Result<(), TrackingError>> {
        Box::pin(async { Ok(()) })
    }
}

struct NoopExecutor;

impl Executor for NoopExecutor {
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

impl gaman_core::SchemaInspector for NoopExecutor {
    fn inspect<'a>(
        &'a mut self,
        _: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::schema::Schema, gaman_core::InspectionError>> {
        Box::pin(async { Ok(gaman_core::schema::Schema::default()) })
    }
}

/// Executes no SQL while deliberately remaining sendable but not shareable.
struct SendOnlyExecutor(PhantomData<Cell<()>>);

impl Executor for SendOnlyExecutor {
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

impl gaman_core::SchemaInspector for SendOnlyExecutor {
    fn inspect<'a>(
        &'a mut self,
        _: &'a [&'a str],
    ) -> BoxFuture<'a, Result<gaman_core::schema::Schema, gaman_core::InspectionError>> {
        Box::pin(async { Ok(gaman_core::schema::Schema::default()) })
    }
}

/// Requires an expression's concrete type to be transferable between threads.
fn assert_send<T: Send>(_: T) {}

struct ThreadWake(std::thread::Thread);

impl Wake for ThreadWake {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs one immediately-progressing test future without adding a runtime dependency.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
    let mut context = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::thread::park(),
        }
    }
}

/// Verifies command envelopes round-trip without host-specific state.
#[test]
fn command_envelope_round_trips_through_json() {
    let envelope = CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command: Command::Status {
            reverse: true,
            search: Some("users".to_string()),
        },
    };

    let encoded = serde_json::to_string(&envelope).expect("serialize command envelope");
    let decoded: CommandEnvelope =
        serde_json::from_str(&encoded).expect("deserialize command envelope");

    assert_eq!(decoded.protocol_version, COMMAND_PROTOCOL_VERSION);
    assert!(matches!(
        decoded.command,
        Command::Status {
            reverse: true,
            search: Some(search)
        } if search == "users"
    ));
}

/// Verifies protocol v6 preserves repeatable migration-generation filters.
#[test]
fn filtered_make_round_trips_through_protocol_v6() {
    let filters = vec![
        EntityFilter::parse("enum:user_*").expect("enum filter"),
        EntityFilter::parse("table:users").expect("table filter"),
    ];
    let envelope = CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command: Command::Make(MakeCommand::Generate {
            schema: gaman_core::schema::Schema::default(),
            name: Some("roles".to_string()),
            dry_run: true,
            decisions: Vec::new(),
            filters: filters.clone(),
        }),
    };

    let encoded = serde_json::to_string(&envelope).expect("serialize filtered command");
    let decoded: CommandEnvelope =
        serde_json::from_str(&encoded).expect("deserialize filtered command");

    assert_eq!(decoded.protocol_version, 6);
    assert!(matches!(
        decoded.command,
        Command::Make(MakeCommand::Generate { filters: decoded, .. }) if decoded == filters
    ));
}

/// Verifies protocol version six is explicit so older hosts cannot ignore sequence semantics.
#[test]
fn sequence_protocol_version_is_six() {
    assert_eq!(COMMAND_PROTOCOL_VERSION, 6);
}

/// Verifies legacy unfiltered generation payloads default to an empty filter
/// list while the envelope version remains host-enforced.
#[test]
fn generate_filters_are_serde_defaulted() {
    let envelope = CommandEnvelope {
        protocol_version: COMMAND_PROTOCOL_VERSION,
        command: Command::Make(MakeCommand::Generate {
            schema: gaman_core::schema::Schema::default(),
            name: None,
            dry_run: false,
            decisions: Vec::new(),
            filters: Vec::new(),
        }),
    };
    let mut encoded = serde_json::to_value(envelope).expect("serialize command");
    encoded["command"]["arguments"]
        .as_object_mut()
        .expect("generate arguments")
        .remove("filters");
    let decoded: CommandEnvelope =
        serde_json::from_value(encoded).expect("deserialize command without filters");

    assert!(matches!(
        decoded.command,
        Command::Make(MakeCommand::Generate { filters, .. }) if filters.is_empty()
    ));
}

/// Verifies portable make responses retain filename-derived migration identifiers.
#[test]
fn command_response_includes_migration_id() {
    let migration = Migration {
        id: "0001_users".to_string(),
        dependencies: Vec::new(),
        operations: Vec::new(),
        atomic: true,
    };
    let response = CommandResponse::new(CommandResult::Make(MakeResult::Created(migration)));

    let encoded = serde_json::to_value(response).expect("serialize command response");

    assert_eq!(encoded["result"]["value"]["migration"]["id"], "0001_users");
}

/// Verifies invalid commands expose stable machine-readable diagnostic metadata.
#[test]
fn command_failure_is_structured_and_versioned() {
    let failure = CommandError::Invalid("unsupported retry input".to_string()).failure();
    let encoded = serde_json::to_value(failure).expect("serialize command failure");

    assert_eq!(encoded["protocol_version"], COMMAND_PROTOCOL_VERSION);
    assert_eq!(encoded["diagnostic"]["code"], "invalid_command");
    assert_eq!(encoded["diagnostic"]["retryable"], false);
}

/// Verifies one migration catalog owns deterministic order and prefix resolution.
#[test]
fn migration_catalog_is_a_validated_snapshot() {
    let migrations = vec![
        Migration {
            id: "0001_users".to_string(),
            dependencies: Vec::new(),
            operations: Vec::new(),
            atomic: true,
        },
        Migration {
            id: "0002_posts".to_string(),
            dependencies: vec!["0001_users".to_string()],
            operations: Vec::new(),
            atomic: true,
        },
    ];

    let catalog = MigrationCatalog::new(migrations).expect("build migration catalog");

    assert_eq!(catalog.ordered_ids(), ["0001_users", "0002_posts"]);
    assert_eq!(
        catalog.resolve_id("0002").expect("resolve unique prefix"),
        "0002_posts"
    );
}

/// Verifies one runner command reads its migration store exactly once.
#[test]
fn runner_uses_one_catalog_snapshot_per_command() {
    let loads = Arc::new(AtomicUsize::new(0));
    let store = CountingStore {
        loads: Arc::clone(&loads),
    };
    let mut runner = MigrationRunner::new(Dialect::Postgres, store, EmptyTracking, NoopExecutor);
    let command = Command::Status {
        reverse: false,
        search: None,
    };

    let result = block_on(runner.run_command(&command)).expect("run status command");

    assert!(matches!(result, CommandResult::Status(rows) if rows.is_empty()));
    assert_eq!(loads.load(Ordering::SeqCst), 1);
}

/// Verifies catalog loading does not require a send-only executor to be shareable.
#[test]
fn catalog_backed_runner_future_is_send_with_send_only_executor() {
    let mut runner = MigrationRunner::new(
        Dialect::Postgres,
        CountingStore {
            loads: Arc::new(AtomicUsize::new(0)),
        },
        EmptyTracking,
        SendOnlyExecutor(PhantomData),
    );
    let command = Command::Status {
        reverse: false,
        search: None,
    };

    assert_send(runner.run_command(&command));
}

/// Verifies a send-only executor works behind the Tokio mutex used by host request handlers.
#[test]
fn catalog_backed_runner_executes_in_a_send_tokio_task() {
    let runner = Arc::new(tokio::sync::Mutex::new(MigrationRunner::new(
        Dialect::Postgres,
        CountingStore {
            loads: Arc::new(AtomicUsize::new(0)),
        },
        EmptyTracking,
        SendOnlyExecutor(PhantomData),
    )));
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("build Tokio runtime");

    let result = runtime.block_on(async move {
        tokio::spawn(async move {
            let mut runner = runner.lock().await;
            runner
                .run_command(&Command::Status {
                    reverse: false,
                    search: None,
                })
                .await
        })
        .await
        .expect("join Tokio task")
    });

    assert!(matches!(result, Ok(CommandResult::Status(rows)) if rows.is_empty()));
}
