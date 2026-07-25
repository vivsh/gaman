//! Shared migration replay, source tracking, dependency, and naming helpers.

use std::collections::{BTreeSet, HashMap};

use crate::graphs::MigrationGraph;
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{ReplayError, Schema};

/// Schema replay output plus source metadata used to calculate dependencies.
#[derive(Debug)]
pub struct ReplaySources {
    /// Schema after replaying the selected migration order.
    pub schema: Schema,
    /// Last migration id seen for each migration namespace.
    pub last_per_ns: HashMap<String, String>,
    /// Namespace that most recently produced each entity.
    pub entity_ns: HashMap<(EntityKind, String), String>,
}

/// Replays migration graphs into schema state with contextual replay errors.
#[derive(Debug)]
pub struct ReplayEngine<'a> {
    graph: &'a MigrationGraph,
}

impl<'a> ReplayEngine<'a> {
    /// Create a replay helper over a migration graph.
    pub fn new(graph: &'a MigrationGraph) -> Self {
        Self { graph }
    }

    /// Replay the provided migration ids into schema state.
    pub fn replay_ids(&self, ids: &[String]) -> Result<Schema, ReplayError> {
        let mut schema = Schema::default();
        for id in ids {
            let migration = self
                .graph
                .get(id)
                .ok_or_else(|| ReplayError::MigrationNotFound(id.clone()))?;
            Self::apply_migration(&mut schema, migration)?;
        }
        Ok(schema)
    }

    /// Replay migration ids and return source metadata for dependency planning.
    pub fn replay_with_sources(
        &self,
        ordered_ids: &[String],
    ) -> Result<ReplaySources, ReplayError> {
        let mut schema = Schema::default();
        let mut last_per_ns = HashMap::new();
        let mut entity_ns = HashMap::new();

        for id in ordered_ids {
            let migration = self
                .graph
                .get(id)
                .ok_or_else(|| ReplayError::MigrationNotFound(id.clone()))?;
            Self::apply_migration(&mut schema, migration)?;
            let ns = namespace_of(id).to_string();
            for entity in migration.get_entities() {
                entity_ns.insert(entity, ns.clone());
            }
            last_per_ns.insert(ns, id.clone());
        }

        Ok(ReplaySources {
            schema,
            last_per_ns,
            entity_ns,
        })
    }

    /// Apply one migration to a schema with migration/operation error context.
    pub fn apply_migration(schema: &mut Schema, migration: &Migration) -> Result<(), ReplayError> {
        for (index, operation) in migration.operations.iter().enumerate() {
            schema
                .apply(operation)
                .map_err(|inner| ReplayError::WithContext {
                    migration: migration.id.clone(),
                    op_num: index + 1,
                    operation: operation_summary(operation),
                    inner: Box::new(inner),
                })?;
        }
        Ok(())
    }
}

/// Produces stable operation context for replay diagnostics without exposing migration internals.
fn operation_summary(operation: &Operation) -> String {
    format!(
        "{} {}",
        operation.type_name().replace('_', " "),
        operation.entity_name()
    )
}

/// Return the namespace prefix of a migration id, or an empty string for root migrations.
pub fn namespace_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(pos) => &id[..pos],
        None => "",
    }
}

/// Compute migration dependencies from operation entity references and replay source metadata.
pub fn compute_deps(
    ops: &[Operation],
    last_per_ns: &HashMap<String, String>,
    entity_ns: &HashMap<(EntityKind, String), String>,
) -> Vec<String> {
    let mut namespaces = BTreeSet::new();
    namespaces.insert(String::new());

    for op in ops {
        for entity in op_entities(op) {
            if let Some(ns) = entity_ns.get(&entity) {
                namespaces.insert(ns.clone());
            }
        }
    }

    namespaces
        .iter()
        .filter_map(|ns| last_per_ns.get(ns).cloned())
        .collect()
}

/// Derive a deterministic migration name from the primary operation targets.
pub fn deterministic_name_from_ops(ops: &[Operation]) -> String {
    let mut labels: Vec<&str> = Vec::new();
    for op in ops {
        if let Some(label) = op.entity_label()
            && !labels.contains(&label)
        {
            labels.push(label);
        }
    }
    match labels.as_slice() {
        [] => "changes".to_string(),
        [a] => sanitize_id_part(a),
        [a, b] => format!("{}_{}", sanitize_id_part(a), sanitize_id_part(b)),
        [a, _, ..] => format!("{}_changes", sanitize_id_part(a)),
    }
}

fn sanitize_id_part(value: &str) -> String {
    let sanitized: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() {
                c
            } else if c.is_ascii_alphabetic() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let trimmed = sanitized.trim_matches('_');
    if trimmed.is_empty() {
        "changes".to_string()
    } else {
        trimmed.to_string()
    }
}

fn op_entities(op: &Operation) -> Vec<(EntityKind, String)> {
    op.touched_entities()
}
