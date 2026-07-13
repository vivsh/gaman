//! Offline migration planner for replaying committed migrations and generating the next migration.
//!
//! The planner owns the offline lifecycle: replay baseline, normalize schemas, clarify risky
//! operations, canonicalize through dialect preparation, diff, and render SQL on request.

use thiserror::Error;

use crate::clarifier::{
    Clarifier, ClarifyError, ClarifyResult, Decision, TypeResolution, non_type_decisions,
    resolve_unknown_types,
};
use crate::dialects::{Dialect, DialectError};
use crate::diff::{DiffEngine, DiffError};
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::replay::{ReplayEngine, ReplaySources, compute_deps, deterministic_name_from_ops};
use crate::sql_plan::{SqlPlanError, SqlPlanRenderer};
use crate::states::{ReplayError, Schema};

#[derive(Debug, Error)]
pub enum OfflineError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Diff(#[from] DiffError),
    #[error(transparent)]
    Clarifier(#[from] ClarifyError),
    #[error("clarification needed for migration generation")]
    NeedsInput(Vec<crate::clarifier::Clarification>),
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
        self.replay_with_sources()?
            .schema
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))
    }

    pub fn make_migration(
        &self,
        desired_schema: Schema,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, OfflineError> {
        self.make_named_migration(desired_schema, None, decisions)
    }

    /// Generates one migration using an optional caller-selected descriptive suffix.
    pub fn make_named_migration(
        &self,
        desired_schema: Schema,
        name: Option<&str>,
        decisions: &[Decision],
    ) -> Result<Option<Migration>, OfflineError> {
        let replay = self.replay_with_sources()?;
        let previous = replay
            .schema
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))?;
        let desired_schema = desired_schema
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))?;
        let desired_schema =
            match resolve_unknown_types(self.dialect, desired_schema, &previous, decisions)? {
                TypeResolution::Resolved(schema) => schema,
                TypeResolution::NeedsInput(clarifications) => {
                    return Err(OfflineError::NeedsInput(clarifications));
                }
            }
            .prepare(self.dialect)
            .map_err(|err| OfflineError::Schema(err.to_string()))?;
        let raw_ops = DiffEngine::new().diff(&desired_schema, &previous, &self.dialect)?;
        if raw_ops.is_empty() {
            return Ok(None);
        }
        let op_decisions = non_type_decisions(decisions);
        let ops = match Clarifier.process(&raw_ops, &op_decisions)? {
            ClarifyResult::NeedsInput(clarifications) => {
                return Err(OfflineError::NeedsInput(clarifications));
            }
            ClarifyResult::Resolved(ops) => ops,
        };
        let ops = self.dialect.reorder(ops, &previous, &desired_schema);
        let suffix = name
            .filter(|name| !name.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| deterministic_name_from_ops(&ops));
        let id = format!("{:04}_{suffix}", self.next_number());
        MigrationGraph::validate_id(&id)?;
        Ok(Some(Migration {
            id,
            dependencies: compute_deps(&ops, &replay.last_per_ns, &replay.entity_ns),
            operations: ops,
            atomic: self.dialect.supports_transactional_ddl(),
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

    fn replay_with_sources(&self) -> Result<ReplaySources, OfflineError> {
        let (graph, ordered_ids) = self.graph()?;
        Ok(ReplayEngine::new(&graph).replay_with_sources(&ordered_ids)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clarifier::{Answer, ClarificationKind, Decision};
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
            options: Default::default(),
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
            generated_storage: None,
            dialect_options: Default::default(),
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
            generated_storage: None,
            dialect_options: Default::default(),
        }
    }

    fn col(name: &str, col_type: &str) -> Column {
        Column {
            name: name.to_string(),
            col_type: col_type.to_string(),
            nullable: true,
            default: None,
            primary_key: false,
            references: None,
            check: None,
            generated: None,
            generated_storage: None,
            dialect_options: Default::default(),
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

    #[test]
    fn make_migration_asks_for_new_unknown_type_before_diff() {
        let mut desired = Schema::default();
        desired.tables.insert(
            "users".to_string(),
            users_table(vec![id_col(), col("age", "intger")]),
        );

        let err = OfflinePlanner::new(Dialect::Postgres)
            .make_migration(desired, &[])
            .unwrap_err();
        let OfflineError::NeedsInput(clarifications) = err else {
            panic!("expected needs input");
        };

        assert_eq!(clarifications.len(), 1);
        assert!(matches!(
            &clarifications[0].kind,
            ClarificationKind::UnknownType { type_name, suggested, .. }
                if type_name == "intger" && suggested.contains(&"integer".to_string())
        ));
    }

    #[test]
    fn make_migration_type_decision_rewrites_desired_schema() {
        let mut desired = Schema::default();
        desired.tables.insert(
            "users".to_string(),
            users_table(vec![id_col(), col("age", "intger")]),
        );
        let decisions = vec![Decision {
            clarification_id: "unknown_type:users:age".to_string(),
            answer: Answer::UseType("integer".to_string()),
        }];

        let migration = OfflinePlanner::new(Dialect::Postgres)
            .make_migration(desired, &decisions)
            .expect("planning should succeed")
            .expect("migration should be generated");
        let Operation::CreateTable { table } = &migration.operations[0] else {
            panic!("expected create table");
        };

        assert_eq!(table.columns[1].col_type, "integer");
    }

    #[test]
    fn make_migration_trusts_unknown_type_from_replay() {
        let previous = Migration {
            id: "0001_users".to_string(),
            dependencies: vec![],
            operations: vec![Operation::CreateTable {
                table: users_table(vec![id_col(), col("code", "project_code")]),
            }],
            atomic: true,
        };
        let mut desired = Schema::default();
        desired.tables.insert(
            "users".to_string(),
            users_table(vec![
                id_col(),
                col("code", "project_code"),
                col("other_code", "project_code"),
            ]),
        );

        let migration = OfflinePlanner::new(Dialect::Postgres)
            .from_migrations(vec![previous])
            .make_migration(desired, &[])
            .expect("trusted replay type should not ask")
            .expect("migration should be generated");

        assert!(matches!(
            migration.operations.as_slice(),
            [Operation::AddColumn { column, .. }] if column.col_type == "project_code"
        ));
    }

    #[test]
    fn make_migration_accepts_known_extension_type_without_prompt() {
        let mut desired = Schema::default();
        desired.tables.insert(
            "users".to_string(),
            users_table(vec![id_col(), col("email", "citext")]),
        );

        let migration = OfflinePlanner::new(Dialect::Postgres)
            .make_migration(desired, &[])
            .expect("known extension type should not ask")
            .expect("migration should be generated");

        assert!(matches!(
            migration.operations.as_slice(),
            [Operation::CreateTable { table }] if table.columns[1].col_type == "citext"
        ));
    }
}
