use std::collections::HashMap;

use thiserror::Error;

use crate::dialects::{Dialect, DialectError};
use crate::diff::{DiffEngine, DiffError};
use crate::disambiguator::{Decision, DisambiguationResult, Disambiguator, DisambiguatorError};
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::sql_plan::{SqlPlanError, SqlPlanRenderer};
use crate::states::types::EntityKind;
use crate::states::{ReplayError, Schema};

#[derive(Debug, Error)]
pub enum OfflineError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    Disambiguator(#[from] DisambiguatorError),
    #[error("migration generation needs disambiguation input")]
    NeedsInput(Vec<crate::disambiguator::Clarification>),
    #[error(transparent)]
    Dialect(#[from] DialectError),
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error(transparent)]
    SqlPlan(#[from] SqlPlanError),
    #[error("schema validation failed: {0}")]
    Schema(String),
}

#[derive(Debug, Clone)]
pub struct OfflinePlanner {
    dialect: Dialect,
    migrations: Vec<Migration>,
}

#[derive(Copy, Clone)]
pub struct EmbeddedMigrations {
    pub files: &'static [(&'static str, &'static str)],
    pub dir: &'static str,
    pub children: &'static [(&'static str, &'static EmbeddedMigrations)],
}

impl OfflinePlanner {
    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            migrations: Vec::new(),
        }
    }

    pub fn from_migrations(mut self, migrations: Vec<Migration>) -> Self {
        self.migrations = migrations;
        self
    }

    pub fn replay(&self) -> Result<Schema, OfflineError> {
        let (schema, _, _) = self.replay_with_sources()?;
        schema
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))
    }

    pub fn make_migration(
        &self,
        desired_schema: Schema,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, OfflineError> {
        let desired_schema = desired_schema
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))?;
        let (previous, last_per_ns, entity_ns) = self.replay_with_sources()?;
        let previous = previous
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))?;
        let raw_ops = DiffEngine::new().diff(&desired_schema, &previous, &self.dialect)?;
        if raw_ops.is_empty() {
            return Ok(None);
        }
        let ops = match Disambiguator.process(&raw_ops, decisions)? {
            DisambiguationResult::NeedsInput(clarifications) => {
                return Err(OfflineError::NeedsInput(clarifications));
            }
            DisambiguationResult::Resolved(ops) => ops,
        };
        let ops = self.dialect.reorder(ops, &previous, &desired_schema);
        let id = format!(
            "{:04}_{}",
            self.next_number(),
            deterministic_name_from_ops(&ops)
        );
        MigrationGraph::validate_id(&id)?;
        Ok(Some(Migration {
            id,
            dependencies: compute_deps(&ops, &last_per_ns, &entity_ns),
            operations: ops,
            atomic: true,
        }))
    }

    pub fn sql_migrate(&self, migrations: &[Migration]) -> Result<Vec<String>, OfflineError> {
        Ok(SqlPlanRenderer::new(self.dialect, self.migrations.clone())?
            .render_migrations(migrations)?)
    }

    fn graph(&self) -> Result<(MigrationGraph, Vec<String>), OfflineError> {
        let mut graph = MigrationGraph::new();
        for migration in self.migrations.clone() {
            graph.add(migration)?;
        }
        let ordered_ids = graph
            .topological_order()?
            .into_iter()
            .map(str::to_string)
            .collect();
        Ok((graph, ordered_ids))
    }

    fn next_number(&self) -> u32 {
        self.migrations
            .iter()
            .filter(|migration| !migration.id.contains('/'))
            .filter_map(|migration| migration.id.split('_').next()?.parse::<u32>().ok())
            .max()
            .map(|number| number + 1)
            .unwrap_or(1)
    }

    fn replay_with_sources(
        &self,
    ) -> Result<
        (
            Schema,
            HashMap<String, String>,
            HashMap<(EntityKind, String), String>,
        ),
        OfflineError,
    > {
        let (graph, ordered_ids) = self.graph()?;
        let mut state = Schema::default();
        let mut last_per_ns: HashMap<String, String> = HashMap::new();
        let mut entity_ns: HashMap<(EntityKind, String), String> = HashMap::new();
        for id in ordered_ids {
            if let Some(migration) = graph.get(&id) {
                apply_migration_to_state(&mut state, migration)?;
                let ns = namespace_of(&id).to_string();
                for entity in migration.get_entities() {
                    entity_ns.insert(entity, ns.clone());
                }
                last_per_ns.insert(ns, id);
            }
        }
        Ok((state, last_per_ns, entity_ns))
    }
}

fn namespace_of(id: &str) -> &str {
    match id.rfind('/') {
        Some(pos) => &id[..pos],
        None => "",
    }
}

fn apply_migration_to_state(state: &mut Schema, migration: &Migration) -> Result<(), OfflineError> {
    for (i, op) in migration.operations.iter().enumerate() {
        state.apply(op).map_err(|e| ReplayError::WithContext {
            migration: migration.id.clone(),
            op_num: i + 1,
            inner: Box::new(e),
        })?;
    }
    Ok(())
}

fn deterministic_name_from_ops(ops: &[Operation]) -> String {
    let mut labels: Vec<&str> = Vec::new();
    for op in ops {
        if let Some(label) = op_entity_label(op)
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

fn compute_deps(
    ops: &[Operation],
    last_per_ns: &HashMap<String, String>,
    entity_ns: &HashMap<(EntityKind, String), String>,
) -> Vec<String> {
    let mut namespaces = std::collections::BTreeSet::new();
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

fn op_entities(op: &Operation) -> Vec<(EntityKind, String)> {
    match op {
        Operation::CreateTable { table } | Operation::DropTable { table } => {
            let mut entities = vec![(EntityKind::Table, table.qualified_name())];
            for fk in &table.foreign_keys {
                entities.push((EntityKind::Table, fk.to_table.clone()));
            }
            entities
        }
        Operation::AddForeignKey {
            table_name,
            foreign_key,
        }
        | Operation::DropForeignKey {
            table_name,
            foreign_key,
            ..
        } => vec![
            (EntityKind::Table, table_name.clone()),
            (EntityKind::Table, foreign_key.to_table.clone()),
        ],
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
                crate::states::schema_qualified_key(enum_name, schema.as_deref()),
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::Operation;
    use crate::states::{Column, Schema, Table};

    fn users_table(columns: Vec<Column>) -> Table {
        Table {
            name: "users".to_string(),
            schema: None,
            primary_key: None,
            columns,
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    }

    fn id_col() -> Column {
        Column {
            name: "id".to_string(),
            col_type: "integer".to_string(),
            nullable: false,
            default: None,
            primary_key: true,
            references: None,
            check: None,
            generated: None,
        }
    }

    fn email_col() -> Column {
        Column {
            name: "email".to_string(),
            col_type: "text".to_string(),
            nullable: true,
            default: None,
            primary_key: false,
            references: None,
            check: None,
            generated: None,
        }
    }

    #[test]
    fn offline_planner_generates_migration_without_database_driver() {
        let mut desired = Schema::default();
        desired.tables.insert(
            "users".to_string(),
            users_table(vec![id_col(), email_col()]),
        );

        let migration = OfflinePlanner::new(Dialect::Postgres)
            .make_migration(desired, &[])
            .expect("offline planning should succeed")
            .expect("migration should be generated");

        assert_eq!(migration.id, "0001_users");
        assert!(migration.dependencies.is_empty());
        assert!(matches!(
            migration.operations.as_slice(),
            [Operation::CreateTable { table }] if table.name == "users"
        ));
    }

    #[test]
    fn offline_replay_is_deterministic() {
        let migration = Migration {
            id: "0001_users".to_string(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable {
                table: users_table(vec![id_col()]),
            }],
            atomic: true,
        };
        let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![migration]);

        let first = planner.replay().expect("first replay should succeed");
        let second = planner.replay().expect("second replay should succeed");

        assert_eq!(first, second);
        assert!(first.tables.contains_key("users"));
    }

    #[test]
    fn migration_yaml_round_trips_from_strings() {
        let migration = Migration {
            id: "0001_users".to_string(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable {
                table: users_table(vec![id_col()]),
            }],
            atomic: true,
        };

        let yaml = migration.to_yaml_string().expect("serialize migration");
        let mut parsed = Migration::from_yaml_str(&yaml).expect("parse migration");
        parsed.id = migration.id.clone();

        assert_eq!(parsed.id, migration.id);
        assert_eq!(parsed.operations, migration.operations);
    }
}
