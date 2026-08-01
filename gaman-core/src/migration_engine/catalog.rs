use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::adapters::{BoxFuture, ExecutorError, MigrationStore, StoreError, TrackingError};
use super::engine::graph_from;
use super::execution_diagnostic::StatementDiagnostic;
use crate::clarifier::Clarification;
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::offline_planner::OfflineError;
use crate::sql_plan::SqlPlanError;
/// Canonical migration content for offline presentation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationArtifact {
    /// Migration identifier in graph order.
    pub id: String,
    /// Canonical YAML representation.
    pub content: String,
}

/// Counts forward and backward migration movement.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationMovement {
    /// Applied migrations.
    pub applied: usize,
    /// Reverted migrations.
    pub reverted: usize,
}

/// Errors returned by [`MigrationEngine`].
#[derive(Debug, Error)]
pub enum EngineError {
    /// Migration storage failed.
    #[error(transparent)]
    Store(#[from] StoreError),
    /// Tracking storage failed.
    #[error(transparent)]
    Tracking(#[from] TrackingError),
    /// SQL execution failed.
    #[error(transparent)]
    Executor(#[from] ExecutorError),
    /// One rendered migration statement was rejected by the target database.
    #[error("migration '{migration}' {direction} statement {statement_ordinal} failed: {source}")]
    MigrationExecution {
        /// Migration whose rendered SQL failed.
        migration: String,
        /// Whether Gaman was applying or rolling back that migration.
        direction: &'static str,
        /// One-based statement ordinal within the migration direction.
        statement_ordinal: usize,
        /// Bounded statement identity and optional database-provided location.
        statement: Box<StatementDiagnostic>,
        /// Database failure returned by the active executor.
        #[source]
        source: Box<ExecutorError>,
    },
    /// Migration graph construction failed.
    #[error(transparent)]
    Graph(#[from] GraphError),
    /// Offline planning failed.
    #[error(transparent)]
    Offline(#[from] OfflineError),
    /// SQL rendering failed.
    #[error(transparent)]
    SqlPlan(#[from] SqlPlanError),
    /// User decisions are required before migration generation can continue.
    #[error("migration generation needs clarification input")]
    NeedsInput(Vec<Clarification>),
    /// The requested operation cannot be completed from current migration state.
    #[error("migration engine configuration error: {0}")]
    Config(String),
}

/// Immutable, validated migration history observed by one lifecycle command.
pub struct MigrationCatalog {
    migrations: Vec<Migration>,
    graph: MigrationGraph,
    ordered: Vec<String>,
}

/// Command-scoped migration storage backed by an immutable catalog snapshot.
pub(crate) struct CatalogMigrationStore<'a, M> {
    pub(super) catalog: &'a MigrationCatalog,
    pub(super) writer: &'a M,
}

impl<M> MigrationStore for CatalogMigrationStore<'_, M>
where
    M: MigrationStore,
{
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<Migration>, StoreError>> {
        Box::pin(async move { Ok(self.catalog.migrations.clone()) })
    }

    fn save<'a>(&'a self, migration: &'a Migration) -> BoxFuture<'a, Result<(), StoreError>> {
        self.writer.save(migration)
    }
}

impl MigrationCatalog {
    /// Builds and validates a deterministic snapshot from caller-provided migrations.
    pub fn new(migrations: Vec<Migration>) -> Result<Self, EngineError> {
        let (graph, ordered) = graph_from(&migrations)?;
        Ok(Self {
            migrations,
            graph,
            ordered,
        })
    }

    /// Returns migrations in their loaded storage representation.
    pub fn migrations(&self) -> &[Migration] {
        &self.migrations
    }

    /// Returns deterministic dependency order for the snapshot.
    pub fn ordered_ids(&self) -> &[String] {
        &self.ordered
    }

    /// Resolves an exact migration identifier or unique prefix within this snapshot.
    pub fn resolve_id(&self, input: &str) -> Result<String, EngineError> {
        Ok(self.graph.resolve_id(input)?)
    }
}
