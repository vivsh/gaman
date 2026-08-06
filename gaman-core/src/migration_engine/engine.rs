use std::collections::HashSet;

use super::adapters::{Executor, ExecutorError, MigrationStore, TrackingStore};
use super::catalog::{
    CatalogMigrationStore, EngineError, MigrationArtifact, MigrationCatalog, MigrationMovement,
};
use super::execution_diagnostic::statement_diagnostic;
use crate::clarifier::Decision;
use crate::dialects::Dialect;
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::offline_planner::{OfflineError, OfflinePlanner};
use crate::operations::Operation;
use crate::sql_plan::SqlPlanRenderer;
use crate::states::Schema;
/// I/O-free migration lifecycle engine over caller-provided storage and execution adapters.
pub struct MigrationEngine<M, T, E> {
    dialect: Dialect,
    migrations: M,
    tracking: T,
    executor: E,
}

impl<M, T, E> MigrationEngine<M, T, E>
where
    M: MigrationStore,
    T: TrackingStore,
    E: Executor,
{
    /// Creates an engine with a fixed dialect and caller-owned integration adapters.
    pub fn new(dialect: Dialect, migrations: M, tracking: T, executor: E) -> Self {
        Self {
            dialect,
            migrations,
            tracking,
            executor,
        }
    }

    /// Returns migration storage for command-scoped catalog loading.
    ///
    /// The returned projection deliberately excludes tracking and execution
    /// adapters so callers can await storage reads without requiring them to be
    /// thread-safe for shared access.
    pub(crate) fn migration_store(&self) -> &M {
        &self.migrations
    }

    /// Returns a fresh migration history snapshot from caller-owned storage.
    async fn migration_snapshot(&mut self) -> Result<Vec<Migration>, EngineError> {
        Ok(self.migrations.load_all().await?)
    }

    /// Creates an engine view whose migration reads remain fixed for one runner command.
    pub(crate) fn for_catalog<'a>(
        &'a mut self,
        catalog: &'a MigrationCatalog,
    ) -> MigrationEngine<CatalogMigrationStore<'a, M>, &'a T, &'a mut E> {
        MigrationEngine::new(
            self.dialect,
            CatalogMigrationStore {
                catalog,
                writer: &self.migrations,
            },
            &self.tracking,
            &mut self.executor,
        )
    }

    /// Returns the SQL dialect used for parsing and rendering.
    pub fn dialect(&self) -> Dialect {
        self.dialect
    }

    /// Exposes the caller-owned executor to the runner's live-inspection boundary.
    pub(crate) fn executor_mut(&mut self) -> &mut E {
        &mut self.executor
    }

    /// Generates and saves a migration for an authored schema.
    pub async fn make(
        &mut self,
        schema: Schema,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_named(schema, None, decisions).await
    }

    /// Generates and saves a migration with an optional caller-selected descriptive suffix.
    pub async fn make_named(
        &mut self,
        schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_named_filtered(schema, name, decisions, &[]).await
    }

    /// Generates and saves a migration limited to selected root entities and dependencies.
    pub async fn make_named_filtered(
        &mut self,
        schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
        filters: &[crate::EntityFilter],
    ) -> Result<Option<Migration>, EngineError> {
        match self.plan_make(schema, name, decisions, filters).await? {
            Some(migration) => {
                self.migrations.save(&migration).await?;
                Ok(Some(migration))
            }
            None => Ok(None),
        }
    }

    /// Generates a migration without persisting it to caller-owned storage.
    pub async fn make_dry_run(
        &mut self,
        schema: Schema,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_dry_run_named(schema, None, decisions).await
    }

    /// Generates a named migration without persisting it to caller-owned storage.
    pub async fn make_dry_run_named(
        &mut self,
        schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, EngineError> {
        self.make_dry_run_named_filtered(schema, name, decisions, &[])
            .await
    }

    /// Previews a migration limited to selected root entities and dependencies.
    pub async fn make_dry_run_named_filtered(
        &mut self,
        schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
        filters: &[crate::EntityFilter],
    ) -> Result<Option<Migration>, EngineError> {
        self.plan_make(schema, name, decisions, filters).await
    }

    /// Fails when prepared schema state differs from committed migration history.
    pub async fn make_check(
        &mut self,
        schema: Schema,
        decisions: &[Decision],
    ) -> Result<(), EngineError> {
        match self.plan_make(schema, None, decisions, &[]).await? {
            Some(_) => Err(EngineError::Config(
                "schema has changes not yet in a migration".to_string(),
            )),
            None => Ok(()),
        }
    }

    async fn plan_make(
        &mut self,
        schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
        filters: &[crate::EntityFilter],
    ) -> Result<Option<Migration>, EngineError> {
        let planner =
            OfflinePlanner::new(self.dialect).from_migrations(self.migration_snapshot().await?);
        match planner.make_named_migration_filtered(schema, name, decisions, filters) {
            Ok(Some(migration)) => Ok(Some(migration)),
            Ok(None) => Ok(None),
            Err(OfflineError::NeedsInput(clarifications)) => {
                Err(EngineError::NeedsInput(clarifications))
            }
            Err(error) => Err(error.into()),
        }
    }

    /// Replays committed migrations into the dialect-prepared expected schema state.
    pub async fn replay_schema(&mut self) -> Result<Schema, EngineError> {
        Ok(OfflinePlanner::new(self.dialect)
            .from_migrations(self.migration_snapshot().await?)
            .replay()?)
    }

    /// Prepares one supplied SQL statement without executing or recording migration state.
    pub async fn prepare_sql(&mut self, sql: &str) -> Result<(), EngineError> {
        Ok(self.executor.prepare(sql).await?)
    }

    /// Renders untracked repair operations against the current migration replay baseline.
    pub async fn render_operations(
        &mut self,
        operations: &[Operation],
    ) -> Result<Vec<String>, EngineError> {
        let renderer = SqlPlanRenderer::new(self.dialect, self.migration_snapshot().await?)?;
        Ok(renderer.render_operations(operations)?)
    }

    /// Applies untracked repair SQL while retaining normal lock and transaction guarantees.
    pub async fn execute_untracked(&mut self, sql: &[String]) -> Result<(), EngineError> {
        self.executor.acquire_lock().await?;
        let result = self
            .execute_untracked_locked(sql, &mut std::collections::HashMap::new())
            .await;
        let release = self.executor.release_lock().await;
        match (result, release) {
            (Ok(()), Ok(())) => Ok(()),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
        }
    }

    /// Renders and applies untracked operations with managed-row precondition enforcement.
    pub async fn execute_operations_untracked(
        &mut self,
        operations: &[Operation],
    ) -> Result<Vec<String>, EngineError> {
        let sql = self.render_operations(operations).await?;
        let mut checked = checked_row_statements(self.dialect, operations)?;
        self.executor.acquire_lock().await?;
        let result = self.execute_untracked_locked(&sql, &mut checked).await;
        let release = self.executor.release_lock().await;
        match (result, release) {
            (Ok(()), Ok(())) => Ok(sql),
            (Err(error), _) => Err(error),
            (Ok(()), Err(error)) => Err(error.into()),
        }
    }

    async fn execute_untracked_locked(
        &mut self,
        sql: &[String],
        checked: &mut std::collections::HashMap<String, usize>,
    ) -> Result<(), EngineError> {
        let atomic = self.dialect.supports_transactional_ddl();
        if atomic {
            self.executor.begin().await?;
        }
        for statement in sql {
            if let Err(error) = execute_statement(&mut self.executor, statement, checked).await {
                if atomic {
                    let _ = self.executor.rollback().await;
                }
                return Err(error.into());
            }
        }
        if atomic {
            self.executor.commit().await?;
        }
        Ok(())
    }

    /// Creates and saves an empty migration at the current graph head.
    pub async fn make_empty(&mut self, name: &str) -> Result<Migration, EngineError> {
        let (graph, _) = self.graph().await?;
        let id = format!("{:04}_{}", graph.next_number(), name);
        MigrationGraph::validate_id(&id)?;
        let migration = Migration {
            id,
            dependencies: graph.heads().into_iter().map(str::to_string).collect(),
            operations: Vec::new(),
            atomic: self.dialect.supports_transactional_ddl(),
        };
        self.migrations.save(&migration).await?;
        Ok(migration)
    }

    /// Creates and saves a merge migration for a graph with multiple heads.
    pub async fn make_merge(&mut self, name: &str) -> Result<Migration, EngineError> {
        let (graph, _) = self.graph().await?;
        let id = format!("{:04}_{}", graph.next_number(), name);
        MigrationGraph::validate_id(&id)?;
        let migration = graph.create_merge_migration(id)?;
        self.migrations.save(&migration).await?;
        Ok(migration)
    }

    /// Returns canonical migration YAML in graph order.
    pub async fn show(&mut self) -> Result<Vec<MigrationArtifact>, EngineError> {
        let (graph, ordered) = self.graph().await?;
        ordered
            .iter()
            .map(|id| {
                let migration = graph
                    .get(id)
                    .ok_or_else(|| EngineError::Config(format!("unknown migration '{id}'")))?;
                let content = migration
                    .to_yaml_string()
                    .map_err(|error| EngineError::Config(error.to_string()))?;
                Ok(MigrationArtifact {
                    id: id.clone(),
                    content,
                })
            })
            .collect()
    }

    /// Renders forward SQL for all migrations or one resolved ID.
    pub async fn sql(&mut self, id: Option<&str>) -> Result<Vec<String>, EngineError> {
        let migrations = self.migration_snapshot().await?;
        let (graph, ordered) = graph_from(&migrations)?;
        let selected = select_migrations(&graph, &ordered, id)?;
        Ok(SqlPlanRenderer::new(self.dialect, migrations)?.render_migrations(&selected)?)
    }

    /// Renders rollback SQL for all migrations or one resolved ID.
    pub async fn sql_rollback(&mut self, id: Option<&str>) -> Result<Vec<String>, EngineError> {
        let migrations = self.migration_snapshot().await?;
        let (graph, ordered) = graph_from(&migrations)?;
        let selected = select_migrations(&graph, &ordered, id)?;
        Ok(
            SqlPlanRenderer::new(self.dialect, migrations)?
                .render_rollback_migrations(&selected)?,
        )
    }

    /// Lists pending migration IDs.
    pub async fn plan(&mut self) -> Result<Vec<String>, EngineError> {
        let (graph, ordered) = self.graph().await?;
        graph.detect_conflict()?;
        self.tracking
            .install(self.dialect, &mut self.executor)
            .await?;
        let applied = self
            .tracking
            .applied_ids(self.dialect, &mut self.executor)
            .await?;
        validate_applied(&graph, &applied)?;
        Ok(ordered
            .into_iter()
            .filter(|id| !applied.contains(id))
            .collect())
    }

    /// Lists migration IDs together with their applied state.
    pub async fn status(&mut self) -> Result<Vec<(String, bool)>, EngineError> {
        let (graph, ordered) = self.graph().await?;
        graph.detect_conflict()?;
        self.tracking
            .install(self.dialect, &mut self.executor)
            .await?;
        let applied = self
            .tracking
            .applied_ids(self.dialect, &mut self.executor)
            .await?;
        validate_applied(&graph, &applied)?;
        Ok(ordered
            .into_iter()
            .map(|id| {
                let is_applied = applied.contains(&id);
                (id, is_applied)
            })
            .collect())
    }

    /// Applies pending migrations, optionally converging on one target.
    pub async fn apply(
        &mut self,
        target: Option<&str>,
        fake: bool,
    ) -> Result<MigrationMovement, EngineError> {
        let migrations = self.migration_snapshot().await?;
        let (graph, ordered) = graph_from(&migrations)?;
        graph.detect_conflict()?;
        if !fake {
            preflight_movement(self.dialect, &graph, &ordered, &migrations, target)?;
        }
        self.tracking
            .install(self.dialect, &mut self.executor)
            .await?;
        self.executor.acquire_lock().await?;
        let result = self
            .apply_locked(&graph, &ordered, &migrations, target, fake)
            .await;
        let release = self.executor.release_lock().await;
        match (result, release) {
            (Ok(movement), Ok(())) => Ok(movement),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Rolls back applied migrations until `target` is the latest applied migration.
    pub async fn rollback_to(
        &mut self,
        target: &str,
        fake: bool,
    ) -> Result<MigrationMovement, EngineError> {
        let migrations = self.migration_snapshot().await?;
        let (graph, ordered) = graph_from(&migrations)?;
        graph.detect_conflict()?;
        let target = graph.resolve_id(target)?;
        if !fake {
            preflight_movement(self.dialect, &graph, &ordered, &migrations, Some(&target))?;
        }
        self.tracking
            .install(self.dialect, &mut self.executor)
            .await?;
        self.executor.acquire_lock().await?;
        let result = self
            .rollback_locked(&graph, &ordered, &migrations, &target, fake)
            .await;
        let release = self.executor.release_lock().await;
        match (result, release) {
            (Ok(movement), Ok(())) => Ok(movement),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    /// Resolves a full migration ID or unique prefix.
    pub async fn resolve_id(&mut self, input: &str) -> Result<String, EngineError> {
        let (graph, _) = self.graph().await?;
        Ok(graph.resolve_id(input)?)
    }

    async fn apply_locked(
        &mut self,
        graph: &MigrationGraph,
        ordered: &[String],
        migrations: &[Migration],
        target: Option<&str>,
        fake: bool,
    ) -> Result<MigrationMovement, EngineError> {
        let applied = self
            .tracking
            .applied_ids(self.dialect, &mut self.executor)
            .await?;
        validate_applied(graph, &applied)?;
        let renderer = SqlPlanRenderer::new(self.dialect, migrations.to_vec())?;
        let target = target.map(|value| graph.resolve_id(value)).transpose()?;
        let mut applied = applied;
        let (end, reverted) = if let Some(target) = target.as_ref() {
            let end = ordered
                .iter()
                .position(|known| known == target)
                .ok_or_else(|| EngineError::Config(format!("unknown migration '{target}'")))?
                + 1;
            let reverts: Vec<&Migration> = ordered[end..]
                .iter()
                .rev()
                .filter(|id| applied.contains(*id))
                .filter_map(|id| graph.get(id))
                .collect();
            for migration in &reverts {
                self.rollback_one(&renderer, migration, fake).await?;
                applied.remove(&migration.id);
            }
            (end, reverts.len())
        } else {
            (ordered.len(), 0)
        };
        let pending: Vec<&Migration> = ordered[..end]
            .iter()
            .filter(|id| !applied.contains(*id))
            .filter_map(|id| graph.get(id))
            .collect();
        for migration in &pending {
            self.apply_one(&renderer, migration, fake).await?;
        }
        Ok(MigrationMovement {
            applied: pending.len(),
            reverted,
        })
    }

    async fn apply_one(
        &mut self,
        renderer: &SqlPlanRenderer,
        migration: &Migration,
        fake: bool,
    ) -> Result<(), EngineError> {
        let sql = (!fake)
            .then(|| renderer.render_migrations(std::slice::from_ref(migration)))
            .transpose()?;
        let mut checked = checked_row_statements(self.dialect, &migration.operations)?;
        if migration.atomic {
            self.executor.begin().await?;
        }
        if let Some(statements) = sql {
            for (statement_ordinal, statement) in statements.iter().enumerate() {
                if let Err(error) =
                    execute_statement(&mut self.executor, statement, &mut checked).await
                {
                    if migration.atomic {
                        let _ = self.executor.rollback().await;
                    }
                    return Err(migration_execution_error(
                        migration,
                        "apply",
                        statement_ordinal + 1,
                        statement,
                        error,
                    ));
                }
            }
        }
        if let Err(error) = self
            .tracking
            .record(self.dialect, &migration.id, &mut self.executor)
            .await
        {
            if migration.atomic {
                let _ = self.executor.rollback().await;
            }
            return Err(error.into());
        }
        if migration.atomic {
            self.executor.commit().await?;
        }
        Ok(())
    }

    async fn rollback_locked(
        &mut self,
        graph: &MigrationGraph,
        ordered: &[String],
        migrations: &[Migration],
        target: &str,
        fake: bool,
    ) -> Result<MigrationMovement, EngineError> {
        let applied = self
            .tracking
            .applied_ids(self.dialect, &mut self.executor)
            .await?;
        validate_applied(graph, &applied)?;
        let end = ordered
            .iter()
            .position(|known| known == target)
            .ok_or_else(|| EngineError::Config(format!("unknown migration '{target}'")))?
            + 1;
        if !applied.contains(target) {
            return Err(EngineError::Config(format!(
                "target migration '{target}' is not applied"
            )));
        }
        let renderer = SqlPlanRenderer::new(self.dialect, migrations.to_vec())?;
        let reverts: Vec<&Migration> = ordered[end..]
            .iter()
            .rev()
            .filter(|id| applied.contains(*id))
            .filter_map(|id| graph.get(id))
            .collect();
        for migration in &reverts {
            self.rollback_one(&renderer, migration, fake).await?;
        }
        Ok(MigrationMovement {
            applied: 0,
            reverted: reverts.len(),
        })
    }

    async fn rollback_one(
        &mut self,
        renderer: &SqlPlanRenderer,
        migration: &Migration,
        fake: bool,
    ) -> Result<(), EngineError> {
        let sql = (!fake)
            .then(|| renderer.render_rollback_migrations(std::slice::from_ref(migration)))
            .transpose()?;
        let inverse = migration
            .operations
            .iter()
            .rev()
            .filter_map(Operation::inverse)
            .collect::<Vec<_>>();
        let mut checked = checked_row_statements(self.dialect, &inverse)?;
        if migration.atomic {
            self.executor.begin().await?;
        }
        if let Some(statements) = sql {
            for (statement_ordinal, statement) in statements.iter().enumerate() {
                if let Err(error) =
                    execute_statement(&mut self.executor, statement, &mut checked).await
                {
                    if migration.atomic {
                        let _ = self.executor.rollback().await;
                    }
                    return Err(migration_execution_error(
                        migration,
                        "rollback",
                        statement_ordinal + 1,
                        statement,
                        error,
                    ));
                }
            }
        }
        if let Err(error) = self
            .tracking
            .unrecord(self.dialect, &migration.id, &mut self.executor)
            .await
        {
            if migration.atomic {
                let _ = self.executor.rollback().await;
            }
            return Err(error.into());
        }
        if migration.atomic {
            self.executor.commit().await?;
        }
        Ok(())
    }

    async fn graph(&mut self) -> Result<(MigrationGraph, Vec<String>), EngineError> {
        Ok(graph_from(&self.migration_snapshot().await?)?)
    }
}

fn checked_row_statements(
    dialect: Dialect,
    operations: &[Operation],
) -> Result<std::collections::HashMap<String, usize>, EngineError> {
    let mut checked = std::collections::HashMap::new();
    for operation in operations {
        if !matches!(
            operation,
            Operation::InsertRow { .. } | Operation::UpdateRow { .. } | Operation::DeleteRow { .. }
        ) {
            continue;
        }
        let statements = crate::managed_rows::sql::render(dialect, operation)
            .map_err(|error| EngineError::Config(error.to_string()))?;
        for statement in statements {
            *checked.entry(statement).or_insert(0) += 1;
        }
    }
    Ok(checked)
}

async fn execute_statement<E: Executor>(
    executor: &mut E,
    statement: &str,
    checked: &mut std::collections::HashMap<String, usize>,
) -> Result<(), ExecutorError> {
    let managed = checked.get_mut(statement).is_some_and(|remaining| {
        if *remaining == 0 {
            false
        } else {
            *remaining -= 1;
            true
        }
    });
    if !managed {
        return executor.execute(statement).await;
    }
    crate::managed_rows::ensure_one_affected(executor.execute_affected(statement).await?)
}

/// Preserves migration and bounded statement context when a live executor rejects rendered SQL.
fn migration_execution_error(
    migration: &Migration,
    direction: &'static str,
    statement_ordinal: usize,
    statement: &str,
    source: super::adapters::ExecutorError,
) -> EngineError {
    let statement = statement_diagnostic(
        statement,
        source
            .database_failure()
            .and_then(|failure| failure.position.as_ref()),
    );
    EngineError::MigrationExecution {
        migration: migration.id.clone(),
        direction,
        statement_ordinal,
        statement: Box::new(statement),
        source: Box::new(source),
    }
}

/// Validates every SQL plan that target convergence may execute before live side effects begin.
fn preflight_movement(
    dialect: Dialect,
    graph: &MigrationGraph,
    ordered: &[String],
    migrations: &[Migration],
    target: Option<&str>,
) -> Result<(), EngineError> {
    let renderer = SqlPlanRenderer::new(dialect, migrations.to_vec())?;
    let end = target
        .map(|value| graph.resolve_id(value))
        .transpose()?
        .map(|target| {
            ordered
                .iter()
                .position(|known| known == &target)
                .map(|position| position + 1)
                .ok_or_else(|| EngineError::Config(format!("unknown migration '{target}'")))
        })
        .transpose()?
        .unwrap_or(ordered.len());
    let forward = ordered[..end]
        .iter()
        .filter_map(|id| graph.get(id))
        .cloned()
        .collect::<Vec<_>>();
    renderer.render_migrations(&forward)?;
    if end < ordered.len() {
        let rollback = ordered[end..]
            .iter()
            .filter_map(|id| graph.get(id))
            .cloned()
            .collect::<Vec<_>>();
        renderer.render_rollback_migrations(&rollback)?;
    }
    Ok(())
}

pub(super) fn graph_from(
    migrations: &[Migration],
) -> Result<(MigrationGraph, Vec<String>), GraphError> {
    let mut graph = MigrationGraph::new();
    for migration in migrations.iter().cloned() {
        graph.add(migration)?;
    }
    let ordered = graph
        .topological_order()?
        .into_iter()
        .map(str::to_string)
        .collect();
    Ok((graph, ordered))
}

fn select_migrations(
    graph: &MigrationGraph,
    ordered: &[String],
    id: Option<&str>,
) -> Result<Vec<Migration>, EngineError> {
    match id {
        Some(id) => Ok(vec![
            graph
                .get(&graph.resolve_id(id)?)
                .ok_or_else(|| EngineError::Config(format!("unknown migration '{id}'")))?
                .clone(),
        ]),
        None => Ok(ordered
            .iter()
            .filter_map(|id| graph.get(id).cloned())
            .collect()),
    }
}

fn validate_applied(graph: &MigrationGraph, applied: &HashSet<String>) -> Result<(), EngineError> {
    let unknown: Vec<&str> = applied
        .iter()
        .map(String::as_str)
        .filter(|id| graph.get(id).is_none())
        .collect();
    if unknown.is_empty() {
        Ok(())
    } else {
        Err(EngineError::Config(format!(
            "applied migration ids are not present locally: {}",
            unknown.join(", ")
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::task::{Context, Poll, Wake, Waker};

    use super::*;
    use crate::migration_engine::{BoxFuture, ExecutorError, StoreError, TrackingError};

    #[derive(Default)]
    struct MemoryMigrations(Mutex<Vec<Migration>>);
    impl MigrationStore for MemoryMigrations {
        fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
            Box::pin(async { Ok(self.0.lock().expect("lock").clone()) })
        }
        fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
            Box::pin(async move {
                self.0.lock().expect("lock").push(migration.clone());
                Ok(())
            })
        }
    }

    #[derive(Default)]
    struct MemoryTracking(Mutex<HashSet<String>>);
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
            Box::pin(async { Ok(self.0.lock().expect("lock").clone()) })
        }
        fn record<'a>(
            &'a self,
            _: Dialect,
            id: &'a str,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<(), TrackingError>> {
            Box::pin(async move {
                self.0.lock().expect("lock").insert(id.into());
                Ok(())
            })
        }
        fn unrecord<'a>(
            &'a self,
            _: Dialect,
            id: &'a str,
            _: &'a mut dyn Executor,
        ) -> BoxFuture<'a, Result<(), TrackingError>> {
            Box::pin(async move {
                self.0.lock().expect("lock").remove(id);
                Ok(())
            })
        }
    }

    #[derive(Default)]
    struct RecordingExecutor(Vec<String>);
    impl Executor for RecordingExecutor {
        fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.0.push(sql.into());
            Box::pin(async { Ok(()) })
        }
        fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.0.push("BEGIN".into());
            Box::pin(async { Ok(()) })
        }
        fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.0.push("COMMIT".into());
            Box::pin(async { Ok(()) })
        }
        fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
            self.0.push("ROLLBACK".into());
            Box::pin(async { Ok(()) })
        }
    }

    struct NoopWake;
    impl Wake for NoopWake {
        fn wake(self: Arc<Self>) {}
    }
    fn block_on<T>(future: impl Future<Output = T>) -> T {
        let waker = Waker::from(Arc::new(NoopWake));
        let mut context = Context::from_waker(&waker);
        let mut future = Box::pin(future);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
                return value;
            }
        }
    }

    /// Verifies generated migrations persist, apply through the executor, and roll back from tracking.
    #[test]
    fn lifecycle_persists_applies_and_rolls_back() {
        let migrations = MemoryMigrations::default();
        let tracking = MemoryTracking::default();
        let mut engine = MigrationEngine::new(
            Dialect::Postgres,
            migrations,
            tracking,
            RecordingExecutor::default(),
        );
        let schema = Schema::from_sql_str(
            "CREATE TABLE users (id integer PRIMARY KEY);",
            Dialect::Postgres,
        )
        .expect("schema");
        let first = block_on(engine.make(schema, &[]))
            .expect("make")
            .expect("migration");
        let changed = Schema::from_sql_str(
            "CREATE TABLE users (id integer PRIMARY KEY, email text);",
            Dialect::Postgres,
        )
        .expect("changed schema");
        block_on(engine.make(changed, &[]))
            .expect("second make")
            .expect("second migration");
        assert_eq!(
            block_on(engine.apply(None, false)).expect("apply").applied,
            2
        );
        assert_eq!(
            block_on(engine.rollback_to(&first.id, false))
                .expect("rollback")
                .reverted,
            1
        );
    }
}
