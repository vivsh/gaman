use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::dialects::{Dialect, DialectError};
use crate::graphs::{GraphError, MigrationGraph};
use crate::migrations::Migration;
use crate::states::{ReplayError, Schema, SchemaValidationError};

#[derive(Debug, Error)]
pub enum SqlPlanError {
    #[error(transparent)]
    Graph(#[from] GraphError),
    #[error(transparent)]
    Replay(#[from] ReplayError),
    #[error("dialect error in migration '{migration}': {source}")]
    Dialect {
        migration: String,
        #[source]
        source: DialectError,
    },
    #[error("schema validation failed in migration '{migration}': {source}")]
    Schema {
        migration: String,
        #[source]
        source: SchemaValidationError,
    },
    #[error(
        "migration '{migration}' has dependencies that are not present in the graph: {missing}"
    )]
    UnknownDependencies { migration: String, missing: String },
    #[error("duplicate requested migration id: {0}")]
    DuplicateRequestedId(String),
    #[error("migration '{0}' is already present in the replay baseline")]
    AlreadyInBaseline(String),
    #[error(
        "migration '{migration}' depends on '{dependency}', which is not in the replay baseline or earlier requested migrations"
    )]
    UnsatisfiedDependency {
        migration: String,
        dependency: String,
    },
    #[error("rollback SQL can only be rendered for graph migrations; '{0}' is not in the graph")]
    UnknownRollbackMigration(String),
    #[error("requested migrations are not in graph order: '{previous}' appears after '{current}'")]
    NonTopologicalSelection { previous: String, current: String },
    #[error(
        "migration '{migration}' is not reversible: operation {op_num} ('{op_type}') has no inverse"
    )]
    NonReversible {
        migration: String,
        op_num: usize,
        op_type: &'static str,
    },
}

#[derive(Debug)]
pub struct SqlPlanRenderer {
    dialect: Dialect,
    graph: MigrationGraph,
    ordered_ids: Vec<String>,
}

impl SqlPlanRenderer {
    pub fn new(dialect: Dialect, migrations: Vec<Migration>) -> Result<Self, SqlPlanError> {
        let mut graph = MigrationGraph::new();
        for migration in migrations {
            graph.add(migration)?;
        }
        let ordered_ids = graph
            .topological_order()?
            .into_iter()
            .map(str::to_string)
            .collect();
        Ok(Self {
            dialect,
            graph,
            ordered_ids,
        })
    }

    pub fn render_migrations(&self, migrations: &[Migration]) -> Result<Vec<String>, SqlPlanError> {
        let request = self.plan_forward_request(migrations)?;
        let mut statements = Vec::new();
        let mut state = self.replay_ids(&request.baseline_ids)?;
        for migration in migrations {
            statements.extend(render_migration_sql(self.dialect, migration, &state)?);
            apply_migration_to_state(&mut state, migration)?;
            state
                .prepare_mut(&self.dialect)
                .map_err(|source| SqlPlanError::Schema {
                    migration: migration.id.clone(),
                    source,
                })?;
        }
        Ok(statements)
    }

    pub fn render_rollback_migrations(
        &self,
        migrations: &[Migration],
    ) -> Result<Vec<String>, SqlPlanError> {
        let request = self.plan_rollback_request(migrations)?;
        let rollback_migrations = rollback_migrations(migrations)?;
        let mut statements = Vec::new();
        let mut state = self.replay_ids(&request.baseline_ids)?;
        for migration in &rollback_migrations {
            statements.extend(render_migration_sql(self.dialect, migration, &state)?);
            apply_migration_to_state(&mut state, migration)?;
            state
                .prepare_mut(&self.dialect)
                .map_err(|source| SqlPlanError::Schema {
                    migration: migration.id.clone(),
                    source,
                })?;
        }
        Ok(statements)
    }

    pub fn initial_state_for(&self, migrations: &[Migration]) -> Result<Schema, SqlPlanError> {
        let request = self.plan_forward_request(migrations)?;
        self.replay_ids(&request.baseline_ids)
    }

    fn plan_forward_request(
        &self,
        migrations: &[Migration],
    ) -> Result<PlannedRequest, SqlPlanError> {
        validate_unique_requested_ids(migrations)?;
        if migrations.is_empty() {
            return Ok(PlannedRequest {
                baseline_ids: Vec::new(),
            });
        }

        let positions = self.position_map();
        let requested_ids: HashSet<&str> = migrations.iter().map(|m| m.id.as_str()).collect();
        let first = &migrations[0];
        let baseline_ids = match positions.get(first.id.as_str()).copied() {
            Some(position) => self.ordered_ids[..position].to_vec(),
            None => {
                let known_deps: Vec<&str> = first
                    .dependencies
                    .iter()
                    .map(String::as_str)
                    .filter(|dep| self.graph.get(dep).is_some())
                    .collect();
                self.dependency_closure_ids(&known_deps)?
            }
        };

        let mut satisfied: HashSet<String> = baseline_ids.iter().cloned().collect();
        let mut previous_known: Option<(&str, usize)> = None;
        for migration in migrations {
            if satisfied.contains(&migration.id) {
                return Err(SqlPlanError::AlreadyInBaseline(migration.id.clone()));
            }

            if let Some(position) = positions.get(migration.id.as_str()).copied() {
                if let Some((previous, previous_position)) = previous_known
                    && position < previous_position
                {
                    return Err(SqlPlanError::NonTopologicalSelection {
                        previous: previous.to_string(),
                        current: migration.id.clone(),
                    });
                }
                previous_known = Some((migration.id.as_str(), position));
            }

            for dependency in &migration.dependencies {
                if !satisfied.contains(dependency) {
                    if self.graph.get(dependency).is_none()
                        && !requested_ids.contains(dependency.as_str())
                    {
                        return Err(SqlPlanError::UnknownDependencies {
                            migration: migration.id.clone(),
                            missing: dependency.clone(),
                        });
                    }
                    return Err(SqlPlanError::UnsatisfiedDependency {
                        migration: migration.id.clone(),
                        dependency: dependency.clone(),
                    });
                }
            }
            satisfied.insert(migration.id.clone());
        }

        Ok(PlannedRequest { baseline_ids })
    }

    fn plan_rollback_request(
        &self,
        migrations: &[Migration],
    ) -> Result<PlannedRequest, SqlPlanError> {
        validate_unique_requested_ids(migrations)?;
        if migrations.is_empty() {
            return Ok(PlannedRequest {
                baseline_ids: Vec::new(),
            });
        }

        let positions = self.position_map();
        let mut latest_position = 0;
        let mut previous_known: Option<(&str, usize)> = None;
        for migration in migrations {
            let position = positions
                .get(migration.id.as_str())
                .copied()
                .ok_or_else(|| SqlPlanError::UnknownRollbackMigration(migration.id.clone()))?;
            if let Some((previous, previous_position)) = previous_known
                && position < previous_position
            {
                return Err(SqlPlanError::NonTopologicalSelection {
                    previous: previous.to_string(),
                    current: migration.id.clone(),
                });
            }
            latest_position = latest_position.max(position);
            previous_known = Some((migration.id.as_str(), position));
        }

        Ok(PlannedRequest {
            baseline_ids: self.ordered_ids[..=latest_position].to_vec(),
        })
    }

    fn replay_ids(&self, ids: &[String]) -> Result<Schema, SqlPlanError> {
        let mut state = Schema::default();
        for id in ids {
            let migration = self.graph.get(id).expect("ordered id must exist in graph");
            apply_migration_to_state(&mut state, migration)?;
        }
        Ok(state)
    }

    fn position_map(&self) -> HashMap<&str, usize> {
        self.ordered_ids
            .iter()
            .enumerate()
            .map(|(position, id)| (id.as_str(), position))
            .collect()
    }

    fn dependency_closure_ids(&self, roots: &[&str]) -> Result<Vec<String>, SqlPlanError> {
        let mut seen = HashSet::new();
        for root in roots {
            self.collect_dependency_closure(root, &mut seen)?;
        }
        Ok(self
            .ordered_ids
            .iter()
            .filter(|id| seen.contains(id.as_str()))
            .cloned()
            .collect())
    }

    fn collect_dependency_closure(
        &self,
        id: &str,
        seen: &mut HashSet<String>,
    ) -> Result<(), SqlPlanError> {
        if !seen.insert(id.to_string()) {
            return Ok(());
        }
        let migration = self
            .graph
            .get(id)
            .ok_or_else(|| SqlPlanError::UnknownDependencies {
                migration: id.to_string(),
                missing: id.to_string(),
            })?;
        for dependency in &migration.dependencies {
            self.collect_dependency_closure(dependency, seen)?;
        }
        Ok(())
    }
}

#[derive(Debug)]
struct PlannedRequest {
    baseline_ids: Vec<String>,
}

pub fn render_migration_sql(
    dialect: Dialect,
    migration: &Migration,
    start: &Schema,
) -> Result<Vec<String>, SqlPlanError> {
    let mut start = start.clone();
    start
        .prepare_mut(&dialect)
        .map_err(|source| SqlPlanError::Schema {
            migration: migration.id.clone(),
            source,
        })?;

    let mut target = start.clone();
    apply_migration_to_state(&mut target, migration)?;
    target
        .prepare_mut(&dialect)
        .map_err(|source| SqlPlanError::Schema {
            migration: migration.id.clone(),
            source,
        })?;

    dialect
        .plan_migration_sql(migration, &start)
        .map_err(|source| SqlPlanError::Dialect {
            migration: migration.id.clone(),
            source,
        })
}

pub fn rollback_migrations(migrations: &[Migration]) -> Result<Vec<Migration>, SqlPlanError> {
    let mut rollbacks = Vec::with_capacity(migrations.len());
    for migration in migrations.iter().rev() {
        let mut operations = Vec::with_capacity(migration.operations.len());
        for (idx, op) in migration.operations.iter().enumerate() {
            let inverse = op.inverse().ok_or_else(|| SqlPlanError::NonReversible {
                migration: migration.id.clone(),
                op_num: idx + 1,
                op_type: op.type_name(),
            })?;
            operations.push(inverse);
        }
        operations.reverse();
        rollbacks.push(Migration {
            id: format!("{}__rollback", migration.id),
            dependencies: vec![],
            operations,
            atomic: migration.atomic,
        });
    }
    Ok(rollbacks)
}

pub fn apply_migration_to_state(
    state: &mut Schema,
    migration: &Migration,
) -> Result<(), SqlPlanError> {
    for (i, op) in migration.operations.iter().enumerate() {
        state.apply(op).map_err(|e| ReplayError::WithContext {
            migration: migration.id.clone(),
            op_num: i + 1,
            inner: Box::new(e),
        })?;
    }
    Ok(())
}

fn validate_unique_requested_ids(migrations: &[Migration]) -> Result<(), SqlPlanError> {
    let mut seen = HashSet::new();
    for migration in migrations {
        if !seen.insert(migration.id.as_str()) {
            return Err(SqlPlanError::DuplicateRequestedId(migration.id.clone()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::Operation;
    use crate::states::{Column, Table};

    fn column(name: &str, col_type: &str) -> Column {
        Column {
            name: name.to_string(),
            col_type: col_type.to_string(),
            nullable: true,
            ..Default::default()
        }
    }

    fn table(name: &str, columns: &[&str]) -> Table {
        Table {
            name: name.to_string(),
            schema: None,
            primary_key: None,
            columns: columns
                .iter()
                .map(|name| column(name, "text"))
                .collect::<Vec<_>>(),
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    }

    fn migration(id: &str, dependencies: &[&str], operations: Vec<Operation>) -> Migration {
        Migration {
            id: id.to_string(),
            dependencies: dependencies
                .iter()
                .map(|dependency| dependency.to_string())
                .collect(),
            operations,
            atomic: true,
        }
    }

    #[test]
    fn statement_is_emitted_exactly() {
        let migration = migration(
            "0001_raw",
            &[],
            vec![Operation::Statement {
                up: "UPDATE users SET active = true".to_string(),
                down: Some("UPDATE users SET active = false".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![migration.clone()]).unwrap();

        assert_eq!(
            renderer.render_migrations(&[migration]).unwrap(),
            vec!["UPDATE users SET active = true"]
        );
    }

    #[test]
    fn rollback_fails_before_rendering_non_reversible_operations() {
        let migration = migration(
            "0001_raw",
            &[],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: None,
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![migration.clone()]).unwrap();
        let err = renderer
            .render_rollback_migrations(&[migration])
            .unwrap_err();

        assert!(err.to_string().contains("has no inverse"));
    }

    #[test]
    fn unknown_dependencies_fail_before_rendering() {
        let migration = migration(
            "0002_later",
            &["0001_missing"],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: Some("SELECT 1".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![]).unwrap();
        let err = renderer.render_migrations(&[migration]).unwrap_err();

        assert!(
            err.to_string()
                .contains("dependencies that are not present")
        );
    }

    #[test]
    fn generated_migration_replays_declared_dependency_closure_not_full_graph() {
        let create_users = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id"]),
            }],
        );
        let create_posts = migration(
            "0002_create_posts",
            &["0001_create_users"],
            vec![Operation::CreateTable {
                table: table("posts", &["id"]),
            }],
        );
        let generated = migration(
            "0003_generated_posts",
            &["0001_create_users"],
            vec![Operation::CreateTable {
                table: table("posts", &["id"]),
            }],
        );
        let renderer =
            SqlPlanRenderer::new(Dialect::Postgres, vec![create_users, create_posts]).unwrap();

        assert!(renderer.render_migrations(&[generated]).is_ok());
    }

    #[test]
    fn generated_migration_without_dependencies_starts_from_empty_schema() {
        let create_users = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id"]),
            }],
        );
        let generated = migration(
            "0002_generated_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id"]),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![create_users]).unwrap();

        assert!(renderer.render_migrations(&[generated]).is_ok());
    }

    #[test]
    fn generated_migrations_must_be_dependency_ordered() {
        let generated_first = migration(
            "0002_second",
            &["0001_first"],
            vec![Operation::Statement {
                up: "SELECT 2".to_string(),
                down: Some("SELECT 2".to_string()),
            }],
        );
        let generated_second = migration(
            "0001_first",
            &[],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: Some("SELECT 1".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![]).unwrap();
        let err = renderer
            .render_migrations(&[generated_first, generated_second])
            .unwrap_err();

        assert!(err.to_string().contains("not in the replay baseline"));
    }

    #[test]
    fn known_migration_slice_must_be_in_graph_order() {
        let first = migration(
            "0001_first",
            &[],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: Some("SELECT 1".to_string()),
            }],
        );
        let second = migration(
            "0002_second",
            &["0001_first"],
            vec![Operation::Statement {
                up: "SELECT 2".to_string(),
                down: Some("SELECT 2".to_string()),
            }],
        );
        let renderer =
            SqlPlanRenderer::new(Dialect::Postgres, vec![first.clone(), second.clone()]).unwrap();
        let err = renderer.render_migrations(&[second, first]).unwrap_err();

        assert!(
            err.to_string()
                .contains("already present in the replay baseline")
        );
    }

    #[test]
    fn known_migration_slice_must_be_dependency_closed() {
        let first = migration("0001_first", &[], vec![]);
        let second = migration("0002_second", &["0001_first"], vec![]);
        let third = migration("0003_third", &["0002_second"], vec![]);
        let fourth = migration(
            "0004_fourth",
            &["0003_third"],
            vec![Operation::Statement {
                up: "SELECT 4".to_string(),
                down: Some("SELECT 4".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(
            Dialect::Postgres,
            vec![first, second.clone(), third, fourth.clone()],
        )
        .unwrap();
        let err = renderer.render_migrations(&[second, fourth]).unwrap_err();

        assert!(err.to_string().contains("not in the replay baseline"));
    }

    #[test]
    fn duplicate_requested_migrations_fail() {
        let migration = migration(
            "0001_raw",
            &[],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: Some("SELECT 1".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![migration.clone()]).unwrap();
        let err = renderer
            .render_migrations(&[migration.clone(), migration])
            .unwrap_err();

        assert!(err.to_string().contains("duplicate requested migration id"));
    }

    #[test]
    fn rollback_of_generated_migration_fails() {
        let generated = migration(
            "0001_generated",
            &[],
            vec![Operation::Statement {
                up: "SELECT 1".to_string(),
                down: Some("SELECT 1".to_string()),
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Postgres, vec![]).unwrap();
        let err = renderer
            .render_rollback_migrations(&[generated])
            .unwrap_err();

        assert!(
            err.to_string()
                .contains("can only be rendered for graph migrations")
        );
    }

    #[test]
    fn single_known_migration_renders_from_graph_prefix() {
        let create_users = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id"]),
            }],
        );
        let add_email = migration(
            "0002_add_email",
            &["0001_create_users"],
            vec![Operation::AddColumn {
                table_name: "users".to_string(),
                column: column("email", "text"),
            }],
        );
        let renderer =
            SqlPlanRenderer::new(Dialect::Postgres, vec![create_users, add_email.clone()]).unwrap();
        let sql = renderer.render_migrations(&[add_email]).unwrap().join("\n");

        assert!(sql.contains("ALTER TABLE \"users\" ADD COLUMN \"email\" text"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_rebuild_forward_starts_from_dependency_replay_state() {
        let create = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id", "name"]),
            }],
        );
        let drop_name = migration(
            "0002_drop_name",
            &["0001_create_users"],
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: column("name", "text"),
                cascade: false,
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Sqlite, vec![create]).unwrap();
        let sql = renderer.render_migrations(&[drop_name]).unwrap().join("\n");

        assert!(sql.contains("CREATE TABLE \"__gaman_rebuild_users\""));
        assert!(sql.contains("INSERT INTO \"__gaman_rebuild_users\" (\"id\")"));
        assert!(!sql.contains("\"name\") SELECT"));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_generated_rebuild_uses_dependency_closure_baseline() {
        let create_users = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id", "name"]),
            }],
        );
        let add_age = migration(
            "0002_add_age",
            &["0001_create_users"],
            vec![Operation::AddColumn {
                table_name: "users".to_string(),
                column: column("age", "text"),
            }],
        );
        let generated = migration(
            "0003_generated_drop_name",
            &["0001_create_users"],
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: column("name", "text"),
                cascade: false,
            }],
        );
        let renderer = SqlPlanRenderer::new(Dialect::Sqlite, vec![create_users, add_age]).unwrap();
        let sql = renderer.render_migrations(&[generated]).unwrap().join("\n");

        assert!(sql.contains("CREATE TABLE \"__gaman_rebuild_users\""));
        assert!(sql.contains("INSERT INTO \"__gaman_rebuild_users\" (\"id\")"));
        assert!(!sql.contains("\"age\""));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn sqlite_rollback_starts_after_last_selected_migration_not_graph_tip() {
        let create = migration(
            "0001_create_users",
            &[],
            vec![Operation::CreateTable {
                table: table("users", &["id"]),
            }],
        );
        let add_email = migration(
            "0002_add_email",
            &["0001_create_users"],
            vec![Operation::AddColumn {
                table_name: "users".to_string(),
                column: column("email", "text"),
            }],
        );
        let add_age = migration(
            "0003_add_age",
            &["0002_add_email"],
            vec![Operation::AddColumn {
                table_name: "users".to_string(),
                column: column("age", "text"),
            }],
        );
        let renderer =
            SqlPlanRenderer::new(Dialect::Sqlite, vec![create, add_email.clone(), add_age])
                .unwrap();
        let sql = renderer
            .render_rollback_migrations(&[add_email])
            .unwrap()
            .join("\n");

        assert!(sql.contains("CREATE TABLE \"__gaman_rebuild_users\""));
        assert!(sql.contains("INSERT INTO \"__gaman_rebuild_users\" (\"id\")"));
        assert!(!sql.contains("\"age\""));
    }
}
