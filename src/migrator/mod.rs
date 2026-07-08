use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource};
use crate::conf::Config;
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{Executor, ExecutorError};
use crate::inspection::{self, InspectionError};
use crate::tracking::{DatabaseTrackingStore, TrackingError, TrackingStore};
use crate::verification;
use gaman_core::clarifier::{Clarification, Clarifier, ClarifyError, ClarifyResult, Decision};
use gaman_core::clarifier::{TypeResolution, non_type_decisions, resolve_unknown_types};
use gaman_core::dialects::{Dialect, DialectError};
use gaman_core::diff::{DiffEngine, DiffError};
use gaman_core::graphs::{GraphError, MigrationGraph};
use gaman_core::migrations::Migration;
use gaman_core::operations::Operation;
use gaman_core::sql_plan::{SqlPlanError, SqlPlanRenderer, render_migration_sql};
use gaman_core::states::types::EntityKind;
use gaman_core::states::{ReplayError, Schema};

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
    #[error("{0}")]
    Inspection(#[from] InspectionError),
    #[error("migration replay failed")]
    Replay(#[from] ReplayError),
    #[error("sql plan failed: {0}")]
    SqlPlan(#[from] SqlPlanError),
    #[error("{0}")]
    Environment(#[from] EnvironmentError),
    #[error("migration tracking failed: {0}")]
    Tracking(#[from] TrackingError),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("clarification error: {0}")]
    Clarifier(#[from] ClarifyError),
    #[error("clarification needed")]
    NeedsInput(Vec<Clarification>),
}

/// Central orchestrator for all migration actions.
/// Holds the shared runtime environment, the migration graph, and the diff engine.
/// The CLI constructs one instance and calls its methods directly.
pub struct Migrator {
    environment: Box<dyn Environment + Send + Sync>,
    pub source: Box<dyn MigrationSource + Send + Sync>,
    pub graph: MigrationGraph,
    ordered_ids: Vec<String>,
    sql_renderer: SqlPlanRenderer,
    tracking: Box<dyn TrackingStore + Send + Sync>,
    pub diff: DiffEngine,
}

impl Migrator {
    pub fn new(
        source: Box<dyn MigrationSource + Send + Sync>,
        environment: Box<dyn Environment + Send + Sync>,
    ) -> Result<Self, MigratorError> {
        Self::new_with_tracking(source, environment, Box::new(DatabaseTrackingStore))
    }

    /// Create a migrator with a caller-provided migration tracking store.
    ///
    /// Native database migrations normally use `DatabaseTrackingStore`. Custom
    /// stores are intended for hosts that track applied migration ids outside
    /// the target database, such as future browser or embedded runtimes.
    pub fn new_with_tracking(
        source: Box<dyn MigrationSource + Send + Sync>,
        environment: Box<dyn Environment + Send + Sync>,
        tracking: Box<dyn TrackingStore + Send + Sync>,
    ) -> Result<Self, MigratorError> {
        let mut graph = MigrationGraph::new();
        let migrations = source.load_all()?;
        for migration in migrations.iter().cloned() {
            graph.add(migration)?;
        }
        // Validate dependency integrity eagerly so broken repos fail at construction, not at migrate-time.
        let ordered_ids = graph
            .topological_order()?
            .into_iter()
            .map(str::to_string)
            .collect();
        let sql_renderer = SqlPlanRenderer::new(environment.dialect(), migrations)?;
        Ok(Self {
            environment,
            source,
            graph,
            ordered_ids,
            sql_renderer,
            tracking,
            diff: DiffEngine::new(),
        })
    }

    pub fn config(&self) -> &Config {
        self.environment.config().as_ref()
    }

    pub fn dialect(&self) -> Dialect {
        self.environment.dialect()
    }

    async fn executor(&self) -> Result<Box<dyn EnvironmentExecutor + Send>, MigratorError> {
        self.environment
            .executor()
            .await
            .map_err(MigratorError::from)
    }

    /// Generate a new migration by diffing `current` against the replayed previous state.
    /// Refuses if there are multiple heads — resolve with `make_merge_migration` first.
    /// Returns `None` when there are no changes.
    /// Pass previously collected `decisions` to resolve any clarification clarifications.
    /// Returns `Err(MigratorError::NeedsInput(clars))` when clarifications are still outstanding.
    pub fn make_migrations(
        &self,
        name: Option<String>,
        current: Schema,
        dry_run: bool,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, MigratorError> {
        self.graph.detect_conflict()?;
        let dialect = self.dialect();
        let (previous, last_per_ns, entity_ns) = self.replay_with_sources()?;
        let previous = previous
            .prepare(dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        let current = current
            .prepare(dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        let current = match resolve_unknown_types(dialect, current, &previous, decisions)? {
            TypeResolution::Resolved(schema) => schema,
            TypeResolution::NeedsInput(clars) => return Err(MigratorError::NeedsInput(clars)),
        }
        .prepare(dialect)
        .map_err(|err| MigratorError::Config(err.to_string()))?;
        let raw_ops = self.diff.diff(&current, &previous, &dialect)?;
        if raw_ops.is_empty() {
            return Ok(None);
        }
        let op_decisions = non_type_decisions(decisions);
        let ops = match Clarifier.process(&raw_ops, &op_decisions)? {
            ClarifyResult::NeedsInput(clars) => {
                return Err(MigratorError::NeedsInput(clars));
            }
            ClarifyResult::Resolved(ops) => ops,
        };
        let ops = dialect.reorder(ops, &previous, &current);
        let name = name.unwrap_or_else(|| name_from_ops(&ops));
        let id = format!("{:04}_{}", self.graph.next_number(), name);
        MigrationGraph::validate_id(&id).map_err(MigratorError::Graph)?;
        let dependencies = compute_deps(&ops, &last_per_ns, &entity_ns);
        let migration = Migration {
            id,
            dependencies,
            operations: ops,
            atomic: true,
        };
        if !dry_run {
            self.source.save(&migration)?;
        }
        Ok(Some(migration))
    }

    fn replay(&self) -> Result<Schema, MigratorError> {
        let (state, _, _) = self.replay_with_sources()?;
        Ok(state)
    }

    fn replay_with_sources(
        &self,
    ) -> Result<
        (
            Schema,
            HashMap<String, String>,
            HashMap<(EntityKind, String), String>,
        ),
        MigratorError,
    > {
        let order = self.ordered_ids.iter().map(String::as_str);
        let mut state = Schema::default();
        let mut last_per_ns: HashMap<String, String> = HashMap::new();
        let mut entity_ns: HashMap<(EntityKind, String), String> = HashMap::new();
        for id in order {
            if let Some(migration) = self.graph.get(id) {
                for (i, op) in migration.operations.iter().enumerate() {
                    state.apply(op).map_err(|e| ReplayError::WithContext {
                        migration: id.to_string(),
                        op_num: i + 1,
                        inner: Box::new(e),
                    })?;
                }
                let ns = namespace_of(id).to_string();
                for entity in migration.get_entities() {
                    entity_ns.insert(entity, ns.clone());
                }
                last_per_ns.insert(ns, id.to_string());
            }
        }
        Ok((state, last_per_ns, entity_ns))
    }

    /// Generate an empty migration with no operations.
    /// Dependencies are set to the current graph heads so it slots in at the tip.
    /// The id is auto-prefixed with the next sequential number: `{n:04}_{name}`.
    pub fn make_empty_migration(&self, name: String) -> Result<Migration, MigratorError> {
        let (_, last_per_ns, entity_ns) = self.replay_with_sources()?;
        let id = format!("{:04}_{}", self.graph.next_number(), name);
        MigrationGraph::validate_id(&id).map_err(MigratorError::Graph)?;
        let dependencies = compute_deps(&[], &last_per_ns, &entity_ns);
        let migration = Migration {
            id,
            dependencies,
            operations: vec![],
            atomic: true,
        };
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
        Ok(self.sql_renderer.render_migrations(migrations)?)
    }

    pub fn sql_rollback(&self, migrations: &[Migration]) -> Result<Vec<String>, MigratorError> {
        Ok(self.sql_renderer.render_rollback_migrations(migrations)?)
    }

    fn replay_prefix(&self, end_exclusive: usize) -> Result<Schema, MigratorError> {
        let mut state = Schema::default();
        for id in self.ordered_ids.iter().take(end_exclusive) {
            let migration = self.graph.get(id).expect("ordered id must exist in graph");
            apply_migration_to_state(&mut state, migration)?;
        }
        Ok(state)
    }

    fn replay_before_migration(&self, id: &str) -> Result<Schema, MigratorError> {
        let position = self
            .ordered_ids
            .iter()
            .position(|known| known == id)
            .ok_or_else(|| MigratorError::Config(format!("unknown migration '{id}'")))?;
        self.replay_prefix(position)
    }

    fn replay_through_migration(&self, id: &str) -> Result<Schema, MigratorError> {
        let position = self
            .ordered_ids
            .iter()
            .position(|known| known == id)
            .ok_or_else(|| MigratorError::Config(format!("unknown migration '{id}'")))?;
        self.replay_prefix(position + 1)
    }

    fn render_migration_sql(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<Vec<String>, MigratorError> {
        Ok(render_migration_sql(self.dialect(), migration, start)?)
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
        let dialect = self.dialect();
        if !direction_forward {
            gaman_core::sql_plan::rollback_migrations(migrations)?;
            return Ok(());
        }

        let mut state = Schema::default();
        let mut index_names: std::collections::HashMap<String, std::collections::HashSet<String>> =
            std::collections::HashMap::new();
        let mut constraint_names: std::collections::HashMap<
            String,
            std::collections::HashSet<String>,
        > = std::collections::HashMap::new();

        for m in migrations {
            dialect.validate_migration_with_state(m, &state)?;
            for (i, op) in m.operations.iter().enumerate() {
                match op {
                    gaman_core::operations::Operation::CreateTable { table } => {
                        for fk in &table.foreign_keys {
                            let is_self_ref = fk.to_table == table.qualified_name();
                            if !is_self_ref && !state.tables.contains_key(&fk.to_table) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): foreign key '{}' references unknown table '{}'",
                                    m.id,
                                    i + 1,
                                    fk.name,
                                    fk.to_table
                                )));
                            }
                        }
                        for idx in &table.indexes {
                            let entry = index_names.entry(table.name.clone()).or_default();
                            if !entry.insert(idx.name.clone()) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): duplicate index name '{}' on table '{}'",
                                    m.id,
                                    i + 1,
                                    idx.name,
                                    table.name
                                )));
                            }
                        }
                        for c in &table.constraints {
                            let entry = constraint_names.entry(table.name.clone()).or_default();
                            if !entry.insert(c.name().to_string()) {
                                return Err(MigratorError::Config(format!(
                                    "migration '{}' (operation {}): duplicate constraint name '{}' on table '{}'",
                                    m.id,
                                    i + 1,
                                    c.name(),
                                    table.name
                                )));
                            }
                        }
                    }
                    gaman_core::operations::Operation::AddForeignKey {
                        table_name: _,
                        foreign_key,
                    } => {
                        if !state.tables.contains_key(&foreign_key.to_table) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): foreign key '{}' references unknown table '{}'",
                                m.id,
                                i + 1,
                                foreign_key.name,
                                foreign_key.to_table
                            )));
                        }
                    }
                    gaman_core::operations::Operation::AddIndex {
                        table_name, index, ..
                    } => {
                        let entry = index_names.entry(table_name.clone()).or_default();
                        if !entry.insert(index.name.clone()) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): duplicate index name '{}' on table '{}'",
                                m.id,
                                i + 1,
                                index.name,
                                table_name
                            )));
                        }
                    }
                    gaman_core::operations::Operation::AddConstraint {
                        table_name,
                        constraint,
                    } => {
                        let entry = constraint_names.entry(table_name.clone()).or_default();
                        if !entry.insert(constraint.name().to_string()) {
                            return Err(MigratorError::Config(format!(
                                "migration '{}' (operation {}): duplicate constraint name '{}' on table '{}'",
                                m.id,
                                i + 1,
                                constraint.name(),
                                table_name
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
                state.validate_checked().map_err(|e| {
                    MigratorError::Config(format!(
                        "migration '{}' (operation {}): {e}",
                        m.id,
                        i + 1
                    ))
                })?;
            }
        }
        Ok(())
    }

    /// Create the migration tracking table if it does not already exist.
    /// Safe to call repeatedly — uses CREATE TABLE IF NOT EXISTS internally.
    pub async fn install(&self, executor: &mut dyn Executor) -> Result<(), MigratorError> {
        self.tracking.install(self.dialect(), executor).await?;
        Ok(())
    }

    async fn run_sql_statements(
        &self,
        sqls: &[String],
        executor: &mut dyn Executor,
    ) -> Result<(), MigratorError> {
        for sql in sqls {
            executor.execute(sql).await?;
        }
        Ok(())
    }

    async fn applied_set(
        &self,
        executor: &mut dyn Executor,
    ) -> Result<HashSet<String>, MigratorError> {
        Ok(self.tracking.applied_ids(executor).await?)
    }

    async fn apply_one(
        &self,
        migration: &Migration,
        executor: &mut dyn Executor,
        fake: bool,
    ) -> Result<(), MigratorError> {
        let rendered = if fake {
            None
        } else {
            let start = self.replay_before_migration(&migration.id)?;
            Some(self.render_migration_sql(migration, &start)?)
        };
        if migration.atomic {
            executor.begin().await?;
        }
        if let Some(sqls) = rendered.as_ref()
            && let Err(e) = self.run_sql_statements(sqls, executor).await
        {
            if migration.atomic {
                let _ = executor.rollback().await;
            }
            return Err(e);
        }
        if let Err(e) = self.tracking.record(&migration.id, executor).await {
            if migration.atomic {
                let _ = executor.rollback().await;
            }
            return Err(e.into());
        }
        if migration.atomic
            && let Err(e) = executor.commit().await
        {
            let _ = executor.rollback().await;
            return Err(e.into());
        }
        Ok(())
    }

    /// Apply all unapplied migrations in topological order.
    /// If `target` is given, apply or roll back to that migration id.
    /// If `fake` is true, record migrations as applied without executing them.
    /// Refuses if there are multiple heads — resolve with `make_merge_migration` first.
    /// Calls `install` internally so the tracking table is always present.
    /// Each migration runs in its own transaction; a failure rolls back only that migration.
    /// Returns the number of migrations applied (forward direction only).
    pub async fn migrate(&self, target: Option<&str>, fake: bool) -> Result<usize, MigratorError> {
        let mut executor = self.executor().await?;
        self.migrate_with(executor.as_mut(), target, fake).await
    }

    pub async fn migrate_with(
        &self,
        executor: &mut dyn Executor,
        target: Option<&str>,
        fake: bool,
    ) -> Result<usize, MigratorError> {
        self.graph.detect_conflict()?;
        let all_ordered: Vec<&str> = self.ordered_ids.iter().map(String::as_str).collect();

        let validation_ordered = if let Some(target_id) = target {
            if self.graph.get(target_id).is_none() {
                return Err(MigratorError::Config(format!(
                    "unknown target migration '{target_id}'"
                )));
            }
            let target_pos = all_ordered
                .iter()
                .position(|id| *id == target_id)
                .expect("target exists in graph so must be in topo order");
            &all_ordered[..=target_pos]
        } else {
            &all_ordered[..]
        };
        let all_migrations: Vec<_> = validation_ordered
            .iter()
            .filter_map(|id| self.graph.get(id))
            .cloned()
            .collect();
        self.validate_plan(&all_migrations, true)?;

        self.install(executor).await?;
        executor.acquire_lock().await?;
        let result = self
            .migrate_locked(executor, target, fake, all_ordered)
            .await;
        let release_result = executor.release_lock().await;

        match (result, release_result) {
            (Ok(count), Ok(())) => Ok(count),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error.into()),
        }
    }

    async fn migrate_locked<'a>(
        &'a self,
        executor: &mut dyn Executor,
        target: Option<&str>,
        fake: bool,
        all_ordered: Vec<&'a str>,
    ) -> Result<usize, MigratorError> {
        if let Some(target_id) = target {
            if self.graph.get(target_id).is_none() {
                return Err(MigratorError::Config(format!(
                    "unknown target migration '{target_id}'"
                )));
            }

            let order = all_ordered;
            let applied: HashSet<String> = self.applied_set(executor).await?;
            self.validate_applied_ids(&applied)?;

            let target_pos = order
                .iter()
                .position(|id| *id == target_id)
                .expect("target exists in graph so must be in topo order");

            let mut to_revert: Vec<&str> = order[target_pos + 1..]
                .iter()
                .filter(|id| applied.contains(*id as &str))
                .copied()
                .collect();
            to_revert.reverse();

            for id in to_revert {
                let migration = self.graph.get(id).expect("applied id must exist in graph");
                let rendered = if fake {
                    None
                } else {
                    let start = self.replay_through_migration(id)?;
                    let mut rollback_migrations =
                        gaman_core::sql_plan::rollback_migrations(std::slice::from_ref(migration))?;
                    let rollback_migration = rollback_migrations
                        .pop()
                        .expect("one input migration produces one rollback migration");
                    Some(self.render_migration_sql(&rollback_migration, &start)?)
                };
                if migration.atomic {
                    executor.begin().await?;
                }
                if let Some(sqls) = rendered.as_ref()
                    && let Err(e) = self.run_sql_statements(sqls, executor).await
                {
                    if migration.atomic {
                        let _ = executor.rollback().await;
                    }
                    return Err(e);
                }
                if let Err(e) = self.tracking.unrecord(id, executor).await {
                    if migration.atomic {
                        let _ = executor.rollback().await;
                    }
                    return Err(e.into());
                }
                if migration.atomic
                    && let Err(e) = executor.commit().await
                {
                    let _ = executor.rollback().await;
                    return Err(e.into());
                }
            }

            let pending: Vec<&str> = order[..=target_pos]
                .iter()
                .filter(|id| !applied.contains(*id as &str))
                .copied()
                .collect();
            let applied_count = pending.len();
            for id in pending {
                let migration = self.graph.get(id).expect("pending id must exist in graph");
                self.apply_one(migration, executor, fake).await?;
            }

            return Ok(applied_count);
        }

        let applied: HashSet<String> = self.applied_set(executor).await?;
        self.validate_applied_ids(&applied)?;
        let pending: Vec<String> = all_ordered
            .iter()
            .filter(|id| !applied.contains(**id))
            .map(|id| id.to_string())
            .collect();
        for id in &pending {
            let migration = self.graph.get(id).expect("pending id must exist in graph");
            self.apply_one(migration, executor, fake).await?;
        }
        Ok(pending.len())
    }

    fn validate_applied_ids(&self, applied: &HashSet<String>) -> Result<(), MigratorError> {
        let unknown: Vec<&str> = applied
            .iter()
            .map(String::as_str)
            .filter(|id| self.graph.get(id).is_none())
            .collect();
        if unknown.is_empty() {
            Ok(())
        } else {
            Err(MigratorError::Config(format!(
                "database has applied migration ids that are not present locally: {}",
                unknown.join(", ")
            )))
        }
    }

    /// Return the ordered list of migration ids that would be applied.
    /// Refuses on conflict — the graph must have a single head to produce a linear plan.
    /// Calls `install` internally so the tracking table is always present.
    pub async fn plan(&self) -> Result<Vec<String>, MigratorError> {
        let mut executor = self.executor().await?;
        self.plan_with(executor.as_mut()).await
    }

    pub async fn plan_with(
        &self,
        executor: &mut dyn Executor,
    ) -> Result<Vec<String>, MigratorError> {
        self.graph.detect_conflict()?;
        self.install(executor).await?;
        let order: Vec<&str> = self.ordered_ids.iter().map(String::as_str).collect();
        let applied: HashSet<String> = self.applied_set(executor).await?;
        let pending = order
            .iter()
            .filter(|id| !applied.contains(**id))
            .map(|id| id.to_string())
            .collect();
        Ok(pending)
    }

    /// Return true if there are unapplied migrations, false otherwise.
    pub async fn check(&self) -> Result<bool, MigratorError> {
        let mut executor = self.executor().await?;
        self.check_with(executor.as_mut()).await
    }

    pub async fn check_with(&self, executor: &mut dyn Executor) -> Result<bool, MigratorError> {
        self.plan_with(executor)
            .await
            .map(|pending| !pending.is_empty())
    }

    /// Return all migration ids in topological order paired with whether each has been applied.
    pub async fn show_migrations(&self) -> Result<Vec<(String, bool)>, MigratorError> {
        let mut executor = self.executor().await?;
        self.show_migrations_with(executor.as_mut()).await
    }

    pub async fn show_migrations_with(
        &self,
        executor: &mut dyn Executor,
    ) -> Result<Vec<(String, bool)>, MigratorError> {
        self.graph.detect_conflict()?;
        self.install(executor).await?;
        let order: Vec<&str> = self.ordered_ids.iter().map(String::as_str).collect();
        let applied: HashSet<String> = self.applied_set(executor).await?;
        Ok(order
            .iter()
            .map(|id| (id.to_string(), applied.contains(*id)))
            .collect())
    }

    pub async fn inspect_db(&self, schemas: &[&str]) -> Result<Schema, MigratorError> {
        let mut executor = self.executor().await?;
        Ok(inspection::inspect_database(executor.as_mut(), schemas, self.dialect()).await?)
    }

    /// Compare migration-owned schema objects against the live database.
    ///
    /// `verify` replays the migration graph, inspects the live database, projects
    /// the live schema down to objects present in the replayed state, and returns
    /// the operations needed to repair drift for those owned objects. Live-only
    /// objects are ignored. Opaque object bodies are intentionally stripped before
    /// comparison; verify checks stable metadata such as signatures and trigger
    /// wiring, not catalog-rendered source text.
    pub async fn verify(&self, schema: &str) -> Result<Vec<Operation>, MigratorError> {
        Ok(self.verify_report(schema).await?.operations)
    }

    pub async fn verify_report(
        &self,
        schema: &str,
    ) -> Result<verification::VerificationReport, MigratorError> {
        let mut executor = self.executor().await?;
        self.verify_report_with(executor.as_mut(), schema).await
    }

    pub async fn verify_with(
        &self,
        executor: &mut (dyn EnvironmentExecutor + Send),
        schema: &str,
    ) -> Result<Vec<Operation>, MigratorError> {
        Ok(self.verify_report_with(executor, schema).await?.operations)
    }

    pub(crate) async fn verify_report_with(
        &self,
        executor: &mut (dyn EnvironmentExecutor + Send),
        schema: &str,
    ) -> Result<verification::VerificationReport, MigratorError> {
        let dialect = self.dialect();
        let mut replay = self.replay()?;
        replay
            .prepare_mut(&dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;

        let live = inspection::inspect_database(executor, &[schema], dialect).await?;
        let mut report = verification::verify(replay, live, schema, dialect);
        self.install(executor).await?;
        let applied = self.applied_set(executor).await?;
        self.validate_applied_ids(&applied)?;
        report.pending_migrations = self
            .ordered_ids
            .iter()
            .filter(|id| !applied.contains(id.as_str()))
            .cloned()
            .collect();
        Ok(report)
    }
}

fn namespace_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(pos) => &id[..pos],
        None => "",
    }
}

fn apply_migration_to_state(
    state: &mut Schema,
    migration: &Migration,
) -> Result<(), MigratorError> {
    for (i, op) in migration.operations.iter().enumerate() {
        state.apply(op).map_err(|e| ReplayError::WithContext {
            migration: migration.id.clone(),
            op_num: i + 1,
            inner: Box::new(e),
        })?;
    }
    Ok(())
}

fn op_entity_label(op: &Operation) -> Option<&str> {
    match op {
        Operation::CreateTable { table } | Operation::DropTable { table } => Some(&table.name),
        Operation::RenameTable { new_name, .. } => Some(new_name),
        Operation::AddColumn { table_name, .. }
        | Operation::DropColumn { table_name, .. }
        | Operation::RenameColumn { table_name, .. }
        | Operation::AlterColumn { table_name, .. }
        | Operation::AddForeignKey { table_name, .. }
        | Operation::DropForeignKey { table_name, .. }
        | Operation::AddIndex { table_name, .. }
        | Operation::DropIndex { table_name, .. }
        | Operation::AddConstraint { table_name, .. }
        | Operation::DropConstraint { table_name, .. }
        | Operation::CreateTrigger { table_name, .. }
        | Operation::AlterTrigger { table_name, .. }
        | Operation::DropTrigger { table_name, .. } => Some(table_name),
        Operation::CreateFunction { function } | Operation::DropFunction { function } => {
            Some(&function.name)
        }
        Operation::AlterFunction { new, .. } => Some(&new.name),
        Operation::CreateView { view } | Operation::DropView { view } => Some(&view.name),
        Operation::ReplaceView { new, .. } => Some(&new.name),
        Operation::CreateExtension { extension } | Operation::DropExtension { extension } => {
            Some(&extension.name)
        }
        Operation::CreateEnum { enum_def } | Operation::DropEnum { enum_def } => {
            Some(&enum_def.name)
        }
        Operation::RenameEnumValue { enum_name, .. } => Some(enum_name),
        Operation::AlterEnum { new, .. } => Some(&new.name),
        Operation::Statement { .. } => None,
    }
}

fn name_from_ops(ops: &[Operation]) -> String {
    let mut unique: Vec<&str> = Vec::new();
    for op in ops {
        if let Some(label) = op_entity_label(op)
            && !unique.contains(&label)
        {
            unique.push(label);
        }
    }
    match unique.as_slice() {
        [] | [_, _, _, ..] => auto_timestamp(),
        [a] => a.to_string(),
        [a, b] => format!("{a}_{b}"),
    }
}

fn auto_timestamp() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let mins = secs / 60;
    let hhmm = (mins % (24 * 60)) as u32;
    let days = secs / 86400;
    // days since epoch → approximate YYYYMMDD (good enough for a unique suffix)
    let (y, m, d) = days_to_ymd(days);
    format!("auto_{y:04}{m:02}{d:02}_{hhmm:04}")
}

fn days_to_ymd(days: u64) -> (u32, u32, u32) {
    // Rata Die algorithm (days since 1970-01-01)
    let z = days + 719468;
    let era = z / 146097;
    let doe = z % 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u32, m as u32, d as u32)
}

fn compute_deps(
    ops: &[Operation],
    last_per_ns: &HashMap<String, String>,
    entity_ns: &HashMap<(EntityKind, String), String>,
) -> Vec<String> {
    let mut ns_set: HashSet<String> = HashSet::new();
    ns_set.insert(String::new());

    for op in ops {
        let entities: Vec<(EntityKind, String)> = match op {
            Operation::CreateTable { table } | Operation::DropTable { table } => {
                let mut v = vec![(EntityKind::Table, table.qualified_name())];
                for fk in &table.foreign_keys {
                    v.push((EntityKind::Table, fk.to_table.clone()));
                }
                v
            }
            Operation::AddForeignKey {
                table_name,
                foreign_key,
            }
            | Operation::DropForeignKey {
                table_name,
                foreign_key,
                ..
            } => {
                vec![
                    (EntityKind::Table, table_name.clone()),
                    (EntityKind::Table, foreign_key.to_table.clone()),
                ]
            }
            Operation::CreateEnum { enum_def }
            | Operation::DropEnum { enum_def }
            | Operation::AlterEnum { new: enum_def, .. } => {
                vec![(EntityKind::Enum, enum_def.qualified_name())]
            }
            Operation::RenameEnumValue {
                enum_name, schema, ..
            } => {
                vec![(
                    EntityKind::Enum,
                    gaman_core::schema::schema_qualified_key(enum_name, schema.as_deref()),
                )]
            }
            Operation::CreateFunction { function }
            | Operation::DropFunction { function }
            | Operation::AlterFunction { new: function, .. } => {
                vec![(EntityKind::Function, function.qualified_name())]
            }
            Operation::CreateView { view }
            | Operation::DropView { view }
            | Operation::ReplaceView { new: view, .. } => {
                vec![(EntityKind::View, view.qualified_name())]
            }
            Operation::CreateExtension { extension } | Operation::DropExtension { extension } => {
                vec![(EntityKind::Extension, extension.qualified_name())]
            }
            Operation::AddColumn { table_name, .. }
            | Operation::DropColumn { table_name, .. }
            | Operation::AlterColumn { table_name, .. }
            | Operation::RenameColumn { table_name, .. }
            | Operation::AddIndex { table_name, .. }
            | Operation::DropIndex { table_name, .. }
            | Operation::AddConstraint { table_name, .. }
            | Operation::DropConstraint { table_name, .. }
            | Operation::CreateTrigger { table_name, .. }
            | Operation::AlterTrigger { table_name, .. }
            | Operation::DropTrigger { table_name, .. } => {
                vec![(EntityKind::Table, table_name.clone())]
            }
            Operation::RenameTable { old_name, .. } => {
                vec![(EntityKind::Table, old_name.clone())]
            }
            Operation::Statement { .. } => vec![],
        };
        for entity in entities {
            if let Some(ns) = entity_ns.get(&entity) {
                ns_set.insert(ns.clone());
            }
        }
    }

    let mut deps: Vec<String> = ns_set
        .iter()
        .filter_map(|ns| last_per_ns.get(ns).cloned())
        .collect();
    deps.sort();
    deps.dedup();
    deps
}

#[cfg(test)]
mod tests;
