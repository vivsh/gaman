use super::adapters::SchemaInspector;
use super::diagnostics::CommandError;
use super::protocol::{
    ApplyCommand, Command, CommandResult, MakeCommand, MakeResult, MigrationStatus, RepairOptions,
    RepairReport, SchemaCheckFailure, SchemaCheckInput, SchemaCheckResult, SchemaCheckStatus,
    SqlInput,
};
use crate::drift::{self, VerificationReport};
use crate::migration_engine::{Executor, MigrationEngine, MigrationStore, TrackingStore};
use crate::parsers::segment_sql;
use crate::repair::plan_repair;
use crate::states::Schema;
/// Unified lifecycle façade shared by native, WASM, and other host adapters.
pub struct MigrationRunner<M, T, E> {
    engine: MigrationEngine<M, T, E>,
}

impl<M, T, E> MigrationRunner<M, T, E>
where
    M: MigrationStore,
    T: TrackingStore,
    E: Executor + SchemaInspector,
{
    /// Creates a runner from the same migration dependencies as [`MigrationEngine`].
    pub fn new(dialect: crate::dialects::Dialect, migrations: M, tracking: T, executor: E) -> Self {
        Self {
            engine: MigrationEngine::new(dialect, migrations, tracking, executor),
        }
    }

    /// Executes one resolved lifecycle command without host I/O or presentation.
    pub async fn run_command(&mut self, command: &Command) -> Result<CommandResult, CommandError> {
        if !command.uses_migration_catalog() {
            return self.dispatch_command(command).await;
        }
        let catalog = self.engine.load_catalog().await?;
        let engine = self.engine.for_catalog(&catalog);
        MigrationRunner { engine }.dispatch_command(command).await
    }

    /// Dispatches one borrowed command against the active migration snapshot.
    async fn dispatch_command(&mut self, command: &Command) -> Result<CommandResult, CommandError> {
        match command {
            Command::Make(command) => self.run_make(command).await,
            Command::Apply(command) => self.run_apply(command).await,
            Command::Status { reverse, search } => {
                self.run_status(*reverse, search.as_deref()).await
            }
            Command::Show {
                id,
                reverse,
                search,
            } => {
                self.run_show(id.as_deref(), *reverse, search.as_deref())
                    .await
            }
            Command::Sql { id, backwards } => self.run_sql(id.as_deref(), *backwards).await,
            Command::CheckSchema { inputs } => self.run_check_schema(inputs).await,
            Command::Inspect { schemas, table } => {
                self.run_inspect(schemas, table.as_deref()).await
            }
            Command::Verify { schemas } => Ok(CommandResult::Verify(self.verify(schemas).await?)),
            Command::Repair { schemas, options } => {
                Ok(CommandResult::Repair(self.repair(schemas, options).await?))
            }
        }
    }

    async fn run_make(&mut self, command: &MakeCommand) -> Result<CommandResult, CommandError> {
        let result = match command {
            MakeCommand::Generate {
                schema,
                name,
                decisions,
                dry_run,
            } if *dry_run => match self
                .engine
                .make_dry_run_named(schema.clone(), name.as_deref(), decisions)
                .await?
            {
                Some(migration) => MakeResult::Preview(migration),
                None => MakeResult::NoChanges,
            },
            MakeCommand::Generate {
                schema,
                name,
                decisions,
                ..
            } => match self
                .engine
                .make_named(schema.clone(), name.as_deref(), decisions)
                .await?
            {
                Some(migration) => MakeResult::Created(migration),
                None => MakeResult::NoChanges,
            },
            MakeCommand::Empty { name } => MakeResult::Created(self.engine.make_empty(name).await?),
            MakeCommand::Merge { name } => MakeResult::Created(self.engine.make_merge(name).await?),
            MakeCommand::Check { schema, decisions } => {
                self.engine.make_check(schema.clone(), decisions).await?;
                MakeResult::CheckPassed
            }
        };
        Ok(CommandResult::Make(result))
    }

    async fn run_apply(&mut self, command: &ApplyCommand) -> Result<CommandResult, CommandError> {
        match command {
            ApplyCommand::Execute { target, fake } => Ok(CommandResult::Movement(
                self.engine.apply(target.as_deref(), *fake).await?,
            )),
            ApplyCommand::Plan => Ok(CommandResult::Pending(self.engine.plan().await?)),
            ApplyCommand::Check => {
                let pending = self.engine.plan().await?;
                if pending.is_empty() {
                    Ok(CommandResult::Pending(pending))
                } else {
                    Err(CommandError::Invalid(format!(
                        "{} pending migration(s) exist",
                        pending.len()
                    )))
                }
            }
        }
    }

    async fn run_status(
        &mut self,
        reverse: bool,
        search: Option<&str>,
    ) -> Result<CommandResult, CommandError> {
        let mut rows = self
            .engine
            .status()
            .await?
            .into_iter()
            .map(|(id, applied)| MigrationStatus { id, applied })
            .collect::<Vec<_>>();
        if let Some(search) = search {
            rows.retain(|row| row.id.contains(search));
        }
        if reverse {
            rows.reverse();
        }
        Ok(CommandResult::Status(rows))
    }

    async fn run_show(
        &mut self,
        id: Option<&str>,
        reverse: bool,
        search: Option<&str>,
    ) -> Result<CommandResult, CommandError> {
        let mut rows = self.engine.show().await?;
        if let Some(id) = id {
            let id = self.engine.resolve_id(id).await?;
            rows.retain(|row| row.id == id);
        }
        if let Some(search) = search {
            rows.retain(|row| row.id.contains(search) || row.content.contains(search));
        }
        if reverse {
            rows.reverse();
        }
        Ok(CommandResult::Show(rows))
    }

    async fn run_sql(
        &mut self,
        id: Option<&str>,
        backwards: bool,
    ) -> Result<CommandResult, CommandError> {
        let sql = if backwards {
            self.engine.sql_rollback(id).await?
        } else {
            self.engine.sql(id).await?
        };
        Ok(CommandResult::Sql(sql))
    }

    async fn run_check_schema(
        &mut self,
        inputs: &[SchemaCheckInput],
    ) -> Result<CommandResult, CommandError> {
        let mut results = Vec::with_capacity(inputs.len());
        for input in inputs {
            results.push(match input {
                SchemaCheckInput::Sql(input) => self.check_input(input).await,
                SchemaCheckInput::Ignored { name, reason } => SchemaCheckResult {
                    name: name.clone(),
                    status: SchemaCheckStatus::Ignored {
                        reason: reason.clone(),
                    },
                },
            });
        }
        Ok(CommandResult::SchemaCheck(results))
    }

    /// Validates every independently segmented statement while retaining all failures.
    async fn check_input(&mut self, input: &SqlInput) -> SchemaCheckResult {
        let segments = match segment_sql(&input.sql, self.engine.dialect()) {
            Ok(segments) => segments,
            Err(error) => {
                let (line, column) = match &error {
                    crate::parsers::ParseError::Segment { line, column, .. } => {
                        (Some(*line), Some(*column))
                    }
                    _ => (None, None),
                };
                return SchemaCheckResult {
                    name: input.name.clone(),
                    status: SchemaCheckStatus::Checked {
                        passed: 0,
                        failures: vec![SchemaCheckFailure::Segmentation {
                            line,
                            column,
                            message: error.to_string(),
                        }],
                    },
                };
            }
        };
        let mut passed = 0;
        let mut failures = Vec::new();
        for segment in segments {
            match self.engine.prepare_sql(&segment.sql).await {
                Ok(()) => passed += 1,
                Err(error) => failures.push(SchemaCheckFailure::Statement {
                    ordinal: segment.ordinal,
                    line: segment.start_line,
                    column: segment.start_column,
                    message: error.to_string(),
                }),
            }
        }
        SchemaCheckResult {
            name: input.name.clone(),
            status: SchemaCheckStatus::Checked { passed, failures },
        }
    }

    async fn run_inspect(
        &mut self,
        schemas: &[String],
        table: Option<&str>,
    ) -> Result<CommandResult, CommandError> {
        let schema = self.inspect(schemas).await?;
        Ok(CommandResult::Inspect(match table {
            Some(table) => select_table(schema, table)?,
            None => schema,
        }))
    }

    async fn inspect(&mut self, schemas: &[String]) -> Result<Schema, CommandError> {
        let names = schemas.iter().map(String::as_str).collect::<Vec<_>>();
        Ok(self.engine.executor_mut().inspect(&names).await?)
    }

    async fn verify(&mut self, schemas: &[String]) -> Result<VerificationReport, CommandError> {
        let replay = self.engine.replay_schema().await?;
        let live = self
            .engine
            .dialect()
            .normalize_inspected_schema(self.inspect(schemas).await?)
            .map_err(|error| CommandError::Invalid(error.to_string()))?;
        let names = schemas.iter().map(String::as_str).collect::<Vec<_>>();
        let mut report = drift::diff_schemas(replay, live, &names, self.engine.dialect());
        report.pending_migrations = self.engine.plan().await?;
        Ok(report)
    }

    async fn repair(
        &mut self,
        schemas: &[String],
        options: &RepairOptions,
    ) -> Result<RepairReport, CommandError> {
        let initial = self.verify(schemas).await?;
        if !options.allow_pending && !initial.pending_migrations.is_empty() {
            return Err(CommandError::Invalid(
                "pending migrations block repair".to_string(),
            ));
        }
        let plan = plan_repair(&initial);
        if !options.allow_partial && !plan.skipped_findings.is_empty() {
            return Err(CommandError::Invalid(format!(
                "{} drift finding(s) cannot be repaired automatically",
                plan.skipped_findings.len()
            )));
        }
        let sql = self.engine.render_operations(&plan.operations).await?;
        if options.apply && !sql.is_empty() {
            self.engine.execute_untracked(&sql).await?;
            return Ok(RepairReport {
                verification: self.verify(schemas).await?,
                operations: plan.operations,
                sql,
                applied: true,
                skipped_findings: plan.skipped_findings,
            });
        }
        Ok(RepairReport {
            verification: initial,
            operations: plan.operations,
            sql,
            applied: false,
            skipped_findings: plan.skipped_findings,
        })
    }
}

fn select_table(mut schema: Schema, input: &str) -> Result<Schema, CommandError> {
    let selected = if schema.tables.contains_key(input) {
        input.to_string()
    } else {
        let matches = schema
            .tables
            .iter()
            .filter(|(_, table)| table.name == input)
            .map(|(name, _)| name.clone())
            .collect::<Vec<_>>();
        match matches.as_slice() {
            [] => {
                return Err(CommandError::Invalid(format!(
                    "unknown inspected table '{input}'"
                )));
            }
            [name] => name.clone(),
            _ => {
                return Err(CommandError::Invalid(format!(
                    "inspected table name '{input}' is ambiguous: {}",
                    matches.join(", ")
                )));
            }
        }
    };
    schema.tables.retain(|name, _| name == &selected);
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::future::Future;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    use super::*;
    use crate::dialects::Dialect;
    use crate::migration_engine::{BoxFuture, ExecutorError, StoreError, TrackingError};
    use crate::runner::InspectionError;

    #[derive(Default)]
    struct MemoryMigrations(Mutex<Vec<crate::migrations::Migration>>);

    impl MigrationStore for MemoryMigrations {
        fn load_all<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<Vec<crate::migrations::Migration>, StoreError>> {
            Box::pin(async { Ok(self.0.lock().expect("migration store lock").clone()) })
        }

        fn save<'a>(
            &'a self,
            migration: &'a crate::migrations::Migration,
        ) -> BoxFuture<'a, Result<(), StoreError>> {
            Box::pin(async move {
                self.0
                    .lock()
                    .expect("migration store lock")
                    .push(migration.clone());
                Ok(())
            })
        }
    }

    struct CountingMigrations {
        loads: Arc<AtomicUsize>,
    }

    impl MigrationStore for CountingMigrations {
        fn load_all<'a>(
            &'a self,
        ) -> BoxFuture<'a, Result<Vec<crate::migrations::Migration>, StoreError>> {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Vec::new()) })
        }

        fn save<'a>(
            &'a self,
            _: &'a crate::migrations::Migration,
        ) -> BoxFuture<'a, Result<(), StoreError>> {
            Box::pin(async { Ok(()) })
        }
    }

    #[derive(Default)]
    struct MemoryTracking(Mutex<HashSet<String>>);

    impl TrackingStore for MemoryTracking {
        fn install<'a>(
            &'a self,
            _: crate::dialects::Dialect,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<(), TrackingError>> {
            Box::pin(async { Ok(()) })
        }
        fn applied_ids<'a>(
            &'a self,
            _: crate::dialects::Dialect,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<HashSet<String>, TrackingError>> {
            Box::pin(async { Ok(self.0.lock().expect("tracking lock").clone()) })
        }
        fn record<'a>(
            &'a self,
            _: crate::dialects::Dialect,
            id: &'a str,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<(), TrackingError>> {
            Box::pin(async move {
                self.0.lock().expect("tracking lock").insert(id.to_string());
                Ok(())
            })
        }
        fn unrecord<'a>(
            &'a self,
            _: crate::dialects::Dialect,
            id: &'a str,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<(), TrackingError>> {
            Box::pin(async move {
                self.0.lock().expect("tracking lock").remove(id);
                Ok(())
            })
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

    struct PrepareExecutor {
        prepared: Arc<Mutex<Vec<String>>>,
    }

    impl Executor for PrepareExecutor {
        fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.prepared
                .lock()
                .expect("prepare log lock")
                .push(sql.to_string());
            Box::pin(async move {
                if sql.contains("bad") {
                    Err(ExecutorError::Prepare("rejected statement".to_string()))
                } else {
                    Ok(())
                }
            })
        }

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

    impl SchemaInspector for PrepareExecutor {
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

    /// Runs one immediately-ready async future without choosing a host runtime.
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::from(std::sync::Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("test future unexpectedly pending"),
        }
    }

    /// Verifies runner status and verification use shared engine and inspection adapters.
    #[test]
    fn runner_executes_status_and_verify_without_host_presentation() {
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            MemoryMigrations::default(),
            MemoryTracking::default(),
            NoopExecutor,
        );
        let status = block_on(runner.run_command(&Command::Status {
            reverse: false,
            search: None,
        }))
        .unwrap();
        assert!(matches!(status, CommandResult::Status(rows) if rows.is_empty()));
        let report = block_on(runner.run_command(&Command::Verify {
            schemas: vec!["public".to_string()],
        }))
        .unwrap();
        assert!(matches!(report, CommandResult::Verify(report) if report.findings.is_empty()));
    }

    /// Verifies each catalog-backed command loads storage once and the next command reloads it.
    #[test]
    fn runner_uses_one_fresh_catalog_per_command() {
        let loads = Arc::new(AtomicUsize::new(0));
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            CountingMigrations {
                loads: Arc::clone(&loads),
            },
            MemoryTracking::default(),
            NoopExecutor,
        );
        let command = Command::Status {
            reverse: false,
            search: None,
        };

        block_on(runner.run_command(&command)).expect("first status command");
        assert_eq!(loads.load(Ordering::SeqCst), 1);
        block_on(runner.run_command(&command)).expect("second status command");
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }

    /// Verifies schema checking bypasses migration storage entirely.
    #[test]
    fn schema_check_does_not_load_migration_catalog() {
        let loads = Arc::new(AtomicUsize::new(0));
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            CountingMigrations {
                loads: Arc::clone(&loads),
            },
            MemoryTracking::default(),
            PrepareExecutor {
                prepared: Arc::new(Mutex::new(Vec::new())),
            },
        );
        let command = Command::CheckSchema {
            inputs: vec![SchemaCheckInput::Ignored {
                name: "schema.yaml".to_string(),
                reason: "not SQL".to_string(),
            }],
        };

        block_on(runner.run_command(&command)).expect("schema check command");
        assert_eq!(loads.load(Ordering::SeqCst), 0);
    }

    /// Verifies schema checking retains ignored inputs and continues after statement failures.
    #[test]
    fn schema_check_returns_complete_structured_results() {
        let prepared = Arc::new(Mutex::new(Vec::new()));
        let mut runner = MigrationRunner::new(
            Dialect::Postgres,
            MemoryMigrations::default(),
            MemoryTracking::default(),
            PrepareExecutor {
                prepared: Arc::clone(&prepared),
            },
        );
        let result = block_on(runner.run_command(&Command::CheckSchema {
            inputs: vec![
                SchemaCheckInput::Ignored {
                    name: "schema.yaml".to_string(),
                    reason: "YAML schema input".to_string(),
                },
                SchemaCheckInput::Sql(SqlInput {
                    name: "schema.sql".to_string(),
                    sql: "SELECT 1;\nSELECT bad;\nSELECT 2;".to_string(),
                }),
                SchemaCheckInput::Sql(SqlInput {
                    name: "broken.sql".to_string(),
                    sql: "SELECT 'unterminated".to_string(),
                }),
            ],
        }))
        .expect("schema check result");

        let CommandResult::SchemaCheck(results) = result else {
            panic!("unexpected runner result");
        };
        assert!(matches!(
            results[0].status,
            SchemaCheckStatus::Ignored { .. }
        ));
        assert!(matches!(
            results[1].status,
            SchemaCheckStatus::Checked { passed: 2, ref failures }
                if matches!(failures.as_slice(), [SchemaCheckFailure::Statement { ordinal: 2, line, column, .. }] if *line > 0 && *column > 0)
        ));
        assert!(matches!(
            results[2].status,
            SchemaCheckStatus::Checked { passed: 0, ref failures }
                if matches!(failures.as_slice(), [SchemaCheckFailure::Segmentation { .. }])
        ));
        assert_eq!(prepared.lock().expect("prepare log lock").len(), 3);
    }
}
