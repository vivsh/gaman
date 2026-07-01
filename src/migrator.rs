use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::adapters::{AdapterError, MigrationSource};
use crate::conf::Config;
use crate::dialects::{Dialect, DialectError};
use crate::diff::{DiffEngine, DiffError};
use crate::disambiguator::{
    Clarification, Decision, DisambiguationResult, Disambiguator, DisambiguatorError,
};
use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
use crate::executor::{Executor, ExecutorError};
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{ReplayError, Schema};
use gaman_core::sql_plan::{SqlPlanError, SqlPlanRenderer, render_migration_sql};

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
    #[error("sql plan failed: {0}")]
    SqlPlan(#[from] SqlPlanError),
    #[error("{0}")]
    Environment(#[from] EnvironmentError),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("disambiguation error: {0}")]
    Disambiguator(#[from] DisambiguatorError),
    #[error("clarification needed")]
    NeedsInput(Vec<Clarification>),
}

/// Central orchestrator for all migration actions.
/// Holds the shared runtime environment, the migration graph, and the diff engine.
/// The CLI constructs one instance and calls its methods directly.
pub struct Migrator {
    environment: Box<dyn Environment>,
    pub source: Box<dyn MigrationSource>,
    pub graph: MigrationGraph,
    ordered_ids: Vec<String>,
    sql_renderer: SqlPlanRenderer,
    pub diff: DiffEngine,
}

impl Migrator {
    pub fn new(
        source: Box<dyn MigrationSource>,
        environment: Box<dyn Environment>,
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
            diff: DiffEngine::new(),
        })
    }

    pub fn config(&self) -> &Config {
        self.environment.config().as_ref()
    }

    pub fn dialect(&self) -> Dialect {
        self.environment.dialect()
    }

    async fn executor(&self) -> Result<Box<dyn EnvironmentExecutor>, MigratorError> {
        self.environment
            .executor()
            .await
            .map_err(MigratorError::from)
    }

    /// Generate a new migration by diffing `current` against the replayed previous state.
    /// Refuses if there are multiple heads — resolve with `make_merge_migration` first.
    /// Returns `None` when there are no changes.
    /// Pass previously collected `decisions` to resolve any disambiguation clarifications.
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
        let current = current
            .prepare(dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        let (previous, last_per_ns, entity_ns) = self.replay_with_sources()?;
        let previous = previous
            .prepare(dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        let raw_ops = self.diff.diff(&current, &previous, &dialect)?;
        if raw_ops.is_empty() {
            return Ok(None);
        }
        let ops = match Disambiguator.process(&raw_ops, decisions)? {
            DisambiguationResult::NeedsInput(clars) => {
                return Err(MigratorError::NeedsInput(clars));
            }
            DisambiguationResult::Resolved(ops) => ops,
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
                    crate::operations::Operation::CreateTable { table } => {
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
                    crate::operations::Operation::AddForeignKey {
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
                    crate::operations::Operation::AddIndex {
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
                    crate::operations::Operation::AddConstraint {
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
            }
        }
        Ok(())
    }

    /// Create the migration tracking table if it does not already exist.
    /// Safe to call repeatedly — uses CREATE TABLE IF NOT EXISTS internally.
    pub async fn install(&self, executor: &mut dyn Executor) -> Result<(), MigratorError> {
        for sql in self.dialect().create_tracking_table_sql() {
            executor.execute(&sql).await?;
        }
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
        Ok(executor
            .fetch_strings(self.dialect().applied_migrations_sql())
            .await?
            .into_iter()
            .collect())
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
        if let Err(e) = executor
            .execute(&self.dialect().record_sql(&migration.id))
            .await
        {
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
                if let Err(e) = executor.execute(&self.dialect().unrecord_sql(id)).await {
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
        let schema = executor
            .inspect_db(schemas)
            .await
            .map_err(MigratorError::Executor)?;
        schema
            .prepare(self.dialect())
            .map_err(|err| MigratorError::Config(err.to_string()))
    }

    /// Compare the replayed schema state against the live database and return any differences.
    /// An empty vec means the database matches migrations exactly — no drift.
    /// Scoped to tables/columns/indexes/FKs/constraints only; views, functions,
    /// extensions, and enums are excluded because their canonical representation
    /// differs too much between replayed YAML and pg_catalog.
    pub async fn verify(&self, schema: &str) -> Result<Vec<Operation>, MigratorError> {
        let mut executor = self.executor().await?;
        self.verify_with(executor.as_mut(), schema).await
    }

    pub async fn verify_with(
        &self,
        executor: &mut dyn EnvironmentExecutor,
        schema: &str,
    ) -> Result<Vec<Operation>, MigratorError> {
        let dialect = self.dialect();
        let mut replay = self.replay()?;
        replay
            .prepare_mut(&dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        scope_tables_for_verify(&mut replay, schema);
        scope_opaque_objects_for_verify(&mut replay, schema);
        normalize_state_types(&mut replay, &dialect);

        let mut live = executor
            .inspect_db(&[schema])
            .await
            .map_err(MigratorError::Executor)?;
        live.prepare_mut(&dialect)
            .map_err(|err| MigratorError::Config(err.to_string()))?;
        scope_tables_for_verify(&mut live, schema);
        scope_opaque_objects_for_verify(&mut live, schema);
        normalize_state_types(&mut live, &dialect);

        strip_opaque_source_for_verify(&mut replay);
        strip_opaque_source_for_verify(&mut live);

        Ok(self.diff.diff(&replay, &live, &dialect)?)
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
        if let Some(label) = op_entity_label(op) {
            if !unique.contains(&label) {
                unique.push(label);
            }
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

fn scope_tables_for_verify(state: &mut Schema, schema: &str) {
    let tables = std::mem::take(&mut state.tables);
    state.tables = tables
        .into_values()
        .filter_map(|mut table| match table.schema.as_deref() {
            None => {
                scope_table_references(&mut table, schema);
                Some(table)
            }
            Some(current) if current == schema || (schema == "public" && current == "public") => {
                table.schema = None;
                scope_table_references(&mut table, schema);
                Some(table)
            }
            _ => None,
        })
        .map(|table| (table.qualified_name(), table))
        .collect();
}

fn scope_opaque_objects_for_verify(state: &mut Schema, schema: &str) {
    let views = std::mem::take(&mut state.views);
    state.views = views
        .into_values()
        .filter_map(|mut view| match view.schema.as_deref() {
            None => Some(view),
            Some(current) if current == schema || (schema == "public" && current == "public") => {
                view.schema = None;
                Some(view)
            }
            _ => None,
        })
        .map(|view| (view.qualified_name(), view))
        .collect();

    let functions = std::mem::take(&mut state.functions);
    state.functions = functions
        .into_values()
        .filter_map(|mut function| match function.schema.as_deref() {
            None => Some(function),
            Some(current) if current == schema || (schema == "public" && current == "public") => {
                function.schema = None;
                Some(function)
            }
            _ => None,
        })
        .map(|function| {
            let key = function_verify_key(&function);
            (key, function)
        })
        .collect();

    let extensions = std::mem::take(&mut state.extensions);
    state.extensions = extensions
        .into_values()
        .filter_map(|mut extension| match extension.schema.as_deref() {
            None => Some(extension),
            Some(current) if current == schema || (schema == "public" && current == "public") => {
                extension.schema = None;
                Some(extension)
            }
            _ => None,
        })
        .map(|extension| (extension.qualified_name(), extension))
        .collect();

    let enums = std::mem::take(&mut state.enums);
    state.enums = enums
        .into_values()
        .filter_map(|mut enum_def| match enum_def.schema.as_deref() {
            None => Some(enum_def),
            Some(current) if current == schema || (schema == "public" && current == "public") => {
                enum_def.schema = None;
                Some(enum_def)
            }
            _ => None,
        })
        .map(|enum_def| (enum_def.qualified_name(), enum_def))
        .collect();
}

fn function_verify_key(function: &crate::states::FunctionDef) -> String {
    if function.arguments.is_empty() {
        function.qualified_name()
    } else {
        format!("{}({})", function.qualified_name(), function.arguments)
    }
}

fn scope_table_references(table: &mut crate::states::Table, schema: &str) {
    let prefix = format!("{schema}.");
    for fk in &mut table.foreign_keys {
        if let Some(local) = fk.to_table.strip_prefix(&prefix) {
            fk.to_table = local.to_string();
        }
    }
    for trigger in &mut table.triggers {
        if let Some(function_name) = &mut trigger.function_name
            && let Some(local) = function_name.strip_prefix(&prefix)
        {
            *function_name = local.to_string();
        }
    }
}

fn normalize_state_types(state: &mut Schema, dialect: &crate::dialects::Dialect) {
    for table in state.tables.values_mut() {
        for col in table.columns.iter_mut() {
            let normalized = dialect.normalize_type(&col.col_type).to_string();
            col.col_type = normalized;
        }
    }
}

fn strip_opaque_source_for_verify(state: &mut Schema) {
    for function in state.functions.values_mut() {
        function.body.clear();
    }
    for view in state.views.values_mut() {
        view.definition.clear();
    }
    for table in state.tables.values_mut() {
        for trigger in &mut table.triggers {
            trigger.body = None;
            trigger.when = None;
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
    use crate::environment::{Environment, EnvironmentError, EnvironmentExecutor};
    use crate::executor::{BoxFuture, Introspectable};
    use crate::operations::Operation;
    use crate::states::{
        Column, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Schema, Table, TriggerDef,
        TriggerEvent, TriggerScope, TriggerTiming, Volatility,
    };

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
        ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, EnvironmentError>> {
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
            body: None,
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
        let offline =
            gaman_core::OfflinePlanner::new(Dialect::Postgres).from_migrations(migrations);

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
        assert!(
            matches!(&mig.operations[0], Operation::DropTable { table } if table.name == "users")
        );
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
            mig.operations.iter().any(
                |op| matches!(op, Operation::AddColumn { column, .. } if column.name == "email")
            )
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
        assert!(mig.operations.iter().any(
            |op| matches!(op, Operation::DropColumn { column, .. } if column.name == "email")
        ));
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
        assert!(shared.saved.borrow().is_empty());
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
        posts.foreign_keys.push(ForeignKey {
            name: "posts_user_id_fkey".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        let migrator = migrator_from(vec![migration_with_ops(
            "0001_create_tables",
            &[],
            vec![
                Operation::CreateTable {
                    table: simple_table("users", &["id"]),
                },
                Operation::CreateTable { table: posts },
            ],
        )]);

        let mut live_users = simple_table("users", &["id"]);
        live_users.schema = Some("isolated".to_string());
        let mut live_posts = simple_table("posts", &["id", "user_id"]);
        live_posts.schema = Some("isolated".to_string());
        live_posts.foreign_keys.push(ForeignKey {
            name: "posts_user_id_fkey".to_string(),
            from_column: "user_id".to_string(),
            to_table: "isolated.users".to_string(),
            to_column: "id".to_string(),
        });
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
    async fn verify_ignores_trigger_body_only_drift() {
        let mut table = simple_table("events", &["id"]);
        let mut trigger = verify_trigger("events_audit_trg", "audit_fn");
        trigger.body = Some("SELECT 1".to_string());
        table.triggers.push(trigger);
        let migrator = migrator_from(vec![migration_with_ops(
            "0001_create_table",
            &[],
            vec![Operation::CreateTable {
                table: table.clone(),
            }],
        )]);

        let mut live_table = table;
        live_table.triggers[0].body = Some("SELECT 2".to_string());
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

    /// Validates that save() is called exactly once when dry_run is false.
    #[test]
    fn save_called_when_not_dry_run() {
        let (m, shared) = migrator_with_source(vec![]);
        let current = state_with_tables(&[simple_table("users", &["id"])]);
        m.make_migrations(Some("initial".into()), current, false, &[])
            .unwrap()
            .unwrap();
        assert_eq!(shared.saved.borrow().len(), 1);
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
        use crate::states::ForeignKey;
        let m = migrator_from(vec![]);
        let table = Table {
            name: "posts".into(),
            schema: None,
            primary_key: None,
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
            atomic: true,
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
            primary_key: None,
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
    fn validate_plan_rejects_duplicate_index_name() {
        use crate::states::Index;
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
        use crate::graphs::MigrationGraph;
        let err = MigrationGraph::validate_id("0001 bad id!").unwrap_err();
        assert!(matches!(err, crate::graphs::GraphError::InvalidId(_)));
        assert!(MigrationGraph::validate_id("0001_good_id").is_ok());
    }

    #[test]
    fn validate_duplicate_column_in_schema_state() {
        use crate::states::Column;
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
        use crate::states::ForeignKey;
        Table {
            name: name.to_string(),
            schema: None,
            primary_key: None,
            columns: vec![simple_column("id"), simple_column("ref_id")],
            foreign_keys: vec![ForeignKey {
                name: format!("{name}_{fk_to}_fkey"),
                from_column: "ref_id".to_string(),
                to_table: fk_to.to_string(),
                to_column: "id".to_string(),
            }],
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
        let users = simple_table("users", &["id"]);
        let m = migrator_from(vec![migration_with_ops(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable { table: users }],
        )]);
        let posts = table_with_fk("posts", "users");
        let current = state_with_tables(&[simple_table("users", &["id"]), posts]);
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
        let auth_users = simple_table("auth_users", &["id"]);
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
            simple_table("auth_users", &["id"]),
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
        let auth_users = simple_table("auth_users", &["id"]);
        let billing_plans = simple_table("billing_plans", &["id"]);
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
            use crate::states::ForeignKey;
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
                    ForeignKey {
                        name: "sub_user_fkey".into(),
                        from_column: "user_id".into(),
                        to_table: "auth_users".into(),
                        to_column: "id".into(),
                    },
                    ForeignKey {
                        name: "sub_plan_fkey".into(),
                        from_column: "plan_id".into(),
                        to_table: "billing_plans".into(),
                        to_column: "id".into(),
                    },
                ],
                indexes: vec![],
                constraints: vec![],
                triggers: vec![],
            }
        };
        let current = state_with_tables(&[
            simple_table("root_t", &["id"]),
            simple_table("auth_users", &["id"]),
            simple_table("billing_plans", &["id"]),
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
                    table: simple_table("auth_users", &["id"]),
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
            simple_table("auth_users", &["id"]),
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
}
