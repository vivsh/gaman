//! Shared migration replay helpers for offline planning and SQL plan baselines.

use std::collections::HashMap;

use crate::graphs::MigrationGraph;
use crate::migrations::Migration;
use crate::states::types::EntityKind;
use crate::states::{ReplayError, Schema};

#[derive(Debug)]
pub(crate) struct ReplaySources {
    pub(crate) schema: Schema,
    pub(crate) last_per_ns: HashMap<String, String>,
    pub(crate) entity_ns: HashMap<(EntityKind, String), String>,
}

#[derive(Debug)]
pub(crate) struct ReplayEngine<'a> {
    graph: &'a MigrationGraph,
}

impl<'a> ReplayEngine<'a> {
    pub(crate) fn new(graph: &'a MigrationGraph) -> Self {
        Self { graph }
    }

    pub(crate) fn replay_ids(&self, ids: &[String]) -> Result<Schema, ReplayError> {
        let mut schema = Schema::default();
        for id in ids {
            let migration = self.graph.get(id).expect("ordered id must exist in graph");
            Self::apply_migration(&mut schema, migration)?;
        }
        Ok(schema)
    }

    pub(crate) fn replay_with_sources(
        &self,
        ordered_ids: &[String],
    ) -> Result<ReplaySources, ReplayError> {
        let mut schema = Schema::default();
        let mut last_per_ns = HashMap::new();
        let mut entity_ns = HashMap::new();

        for id in ordered_ids {
            if let Some(migration) = self.graph.get(id) {
                Self::apply_migration(&mut schema, migration)?;
                let ns = namespace_of(id).to_string();
                for entity in migration.get_entities() {
                    entity_ns.insert(entity, ns.clone());
                }
                last_per_ns.insert(ns, id.clone());
            }
        }

        Ok(ReplaySources {
            schema,
            last_per_ns,
            entity_ns,
        })
    }

    pub(crate) fn apply_migration(
        schema: &mut Schema,
        migration: &Migration,
    ) -> Result<(), ReplayError> {
        for (index, operation) in migration.operations.iter().enumerate() {
            schema
                .apply(operation)
                .map_err(|inner| ReplayError::WithContext {
                    migration: migration.id.clone(),
                    op_num: index + 1,
                    inner: Box::new(inner),
                })?;
        }
        Ok(())
    }
}

fn namespace_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(pos) => &id[..pos],
        None => "",
    }
}
