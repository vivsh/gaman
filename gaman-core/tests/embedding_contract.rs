//! External-consumer contract for direct schema building and typed runner commands.

use std::collections::HashSet;
use std::future::Future;
use std::sync::Mutex;
use std::task::{Context, Poll, Wake, Waker};

use gaman_core::schema::{Schema, SchemaBuilder, TableBuilder};
use gaman_core::{
    BoxFuture, Command, CommandResult, Dialect, Executor, ExecutorError, InspectionError,
    MakeCommand, MakeResult, Migration, MigrationRunner, MigrationStore, SchemaInspector,
    StoreError, TrackingError, TrackingStore,
};

#[derive(Default)]
struct MemoryStore(Mutex<Vec<Migration>>);

impl MigrationStore for MemoryStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
        Box::pin(async { Ok(self.0.lock().expect("migration lock").clone()) })
    }

    fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            self.0
                .lock()
                .expect("migration lock")
                .push(migration.clone());
            Ok(())
        })
    }
}

struct MemoryTracking;

impl TrackingStore for MemoryTracking {
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

impl SchemaInspector for NoopExecutor {
    fn inspect<'a>(
        &'a mut self,
        _: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        Box::pin(async { Ok(Schema::default()) })
    }
}

struct NoopWake;

impl Wake for NoopWake {
    fn wake(self: std::sync::Arc<Self>) {}
}

/// Runs an immediately-ready host-neutral future without selecting an async runtime.
fn block_on<T>(future: impl Future<Output = T>) -> T {
    let waker = Waker::from(std::sync::Arc::new(NoopWake));
    let mut context = Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    match future.as_mut().poll(&mut context) {
        Poll::Ready(value) => value,
        Poll::Pending => panic!("embedding contract future unexpectedly pending"),
    }
}

/// Verifies an external host can build a schema, retry a typed command, and consume its result.
#[test]
fn public_builder_and_runner_contract_is_directly_embeddable() {
    let schema = SchemaBuilder::new(Dialect::Postgres)
        .table_def(
            TableBuilder::new("users")
                .column("id", "bigserial", |column| column.primary_key())
                .column("email", "text", |column| column.not_null())
                .build(),
        )
        .build()
        .expect("prepared builder schema");
    let command = Command::Make(MakeCommand::Generate {
        schema,
        name: Some("users".to_string()),
        dry_run: true,
        decisions: Vec::new(),
    })
    .with_decisions(Vec::new())
    .expect("make command accepts clarification decisions");
    let mut runner = MigrationRunner::new(
        Dialect::Postgres,
        MemoryStore::default(),
        MemoryTracking,
        NoopExecutor,
    );

    let result = block_on(runner.run_command(&command)).expect("preview migration");
    assert!(matches!(
        result,
        CommandResult::Make(MakeResult::Preview(_))
    ));
}
