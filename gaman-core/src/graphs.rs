use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::migrations::Migration;

#[derive(Debug, Error)]
pub enum GraphError {
    #[error("duplicate migration id: {0}")]
    DuplicateId(String),
    #[error("cycle detected in migration graph")]
    CycleDetected,
    #[error("multiple heads detected — run make --merge to resolve")]
    Conflict,
    #[error("unknown dependency '{dep}' referenced by '{migration}'")]
    UnknownDependency { migration: String, dep: String },
    #[error("no migrations in graph")]
    Empty,
    #[error(
        "invalid migration id '{0}': only lowercase letters, digits, and underscores are allowed (namespaced ids like 'auth/0001_init' are set automatically by embedded children)"
    )]
    InvalidId(String),
    #[error("unknown migration id or prefix '{0}'")]
    UnknownId(String),
    #[error("migration id prefix '{prefix}' is ambiguous: {matches}")]
    AmbiguousId { prefix: String, matches: String },
}

/// A single node in the migration DAG.
#[derive(Debug, Clone)]
pub struct MigrationNode {
    pub migration: Migration,
}

/// Directed acyclic graph of migrations.
/// Edges: dependency → dependent (A → B means B depends on A).
#[derive(Debug, Default)]
pub struct MigrationGraph {
    nodes: HashMap<String, MigrationNode>,
}

impl MigrationGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a migration to the graph.
    pub fn add(&mut self, migration: Migration) -> Result<(), GraphError> {
        let id = migration.id.clone();
        if self.nodes.contains_key(&id) {
            return Err(GraphError::DuplicateId(id));
        }
        self.nodes.insert(id, MigrationNode { migration });
        Ok(())
    }

    /// Validate that an id is safe as a filename and unambiguous.
    /// Only lowercase letters, digits, and underscores are allowed.
    /// Called by Migrator before writing any new migration file.
    pub fn validate_id(id: &str) -> Result<(), GraphError> {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
        {
            return Err(GraphError::InvalidId(id.to_string()));
        }
        Ok(())
    }

    /// Return IDs of all head migrations (those with no dependents).
    pub fn heads(&self) -> Vec<&str> {
        let mut has_dependents: HashSet<&str> = HashSet::new();
        for node in self.nodes.values() {
            for dep in &node.migration.dependencies {
                has_dependents.insert(dep.as_str());
            }
        }
        self.nodes
            .keys()
            .filter(|id| !has_dependents.contains(id.as_str()))
            .map(String::as_str)
            .collect()
    }

    /// Return the next sequential number (1-based) for a new migration.
    /// Parses the leading digits of each migration id (e.g. "0003_add_users" → 3).
    /// Returns 1 if the graph is empty.
    pub fn next_number(&self) -> u32 {
        self.nodes
            .keys()
            .filter(|id| !id.contains('/'))
            .filter_map(|id| id.split('_').next()?.parse::<u32>().ok())
            .max()
            .map(|n| n + 1)
            .unwrap_or(1)
    }

    /// Returns `Err` if there are multiple heads (conflict).
    pub fn detect_conflict(&self) -> Result<(), GraphError> {
        let heads = self.heads();
        if heads.len() > 1 {
            Err(GraphError::Conflict)
        } else {
            Ok(())
        }
    }

    /// Validate that the graph contains no cycles.
    pub fn validate_acyclic(&self) -> Result<(), GraphError> {
        // DFS-based cycle detection
        let mut visited: HashSet<&str> = HashSet::new();
        let mut stack: HashSet<&str> = HashSet::new();

        for id in self.nodes.keys() {
            self.dfs(id, &mut visited, &mut stack)?;
        }
        Ok(())
    }

    fn dfs<'a>(
        &'a self,
        id: &'a str,
        visited: &mut HashSet<&'a str>,
        stack: &mut HashSet<&'a str>,
    ) -> Result<(), GraphError> {
        if stack.contains(id) {
            return Err(GraphError::CycleDetected);
        }
        if visited.contains(id) {
            return Ok(());
        }
        stack.insert(id);
        if let Some(node) = self.nodes.get(id) {
            for dep in &node.migration.dependencies {
                self.dfs(dep.as_str(), visited, stack)?;
            }
        }
        stack.remove(id);
        visited.insert(id);
        Ok(())
    }

    /// Produce a deterministic topological order of all migration IDs.
    pub fn topological_order(&self) -> Result<Vec<&str>, GraphError> {
        self.validate_acyclic()?;
        self.validate_dependencies()?;

        let mut visited: HashSet<&str> = HashSet::new();
        let mut order: Vec<&str> = Vec::new();

        let mut ids: Vec<&str> = self.nodes.keys().map(String::as_str).collect();
        ids.sort();

        for id in ids {
            self.topo_visit(id, &mut visited, &mut order);
        }
        Ok(order)
    }

    fn validate_dependencies(&self) -> Result<(), GraphError> {
        for node in self.nodes.values() {
            for dep in &node.migration.dependencies {
                if !self.nodes.contains_key(dep.as_str()) {
                    return Err(GraphError::UnknownDependency {
                        migration: node.migration.id.clone(),
                        dep: dep.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    fn topo_visit<'a>(
        &'a self,
        id: &'a str,
        visited: &mut HashSet<&'a str>,
        order: &mut Vec<&'a str>,
    ) {
        if visited.contains(id) {
            return;
        }
        visited.insert(id);
        if let Some(node) = self.nodes.get(id) {
            let mut deps: Vec<&str> = node
                .migration
                .dependencies
                .iter()
                .map(String::as_str)
                .collect();
            deps.sort();
            for dep in deps {
                self.topo_visit(dep, visited, order);
            }
        }
        order.push(id);
    }

    /// Create an empty merge migration that depends on all current heads.
    pub fn create_merge_migration(&self, id: String) -> Result<Migration, GraphError> {
        let heads = self.heads();
        if heads.len() <= 1 {
            return Err(GraphError::Empty);
        }
        let mut dependencies: Vec<String> = heads.iter().map(|s| s.to_string()).collect();
        dependencies.sort();
        Ok(Migration {
            id,
            dependencies,
            operations: vec![],
            atomic: true,
        })
    }

    pub fn get(&self, id: &str) -> Option<&Migration> {
        self.nodes.get(id).map(|n| &n.migration)
    }

    /// Resolves an exact migration id or an unambiguous prefix in graph order.
    ///
    /// Exact ids take precedence over prefixes. Ambiguous prefixes return all
    /// matching ids in deterministic topological order.
    pub fn resolve_id(&self, input: &str) -> Result<String, GraphError> {
        if self.nodes.contains_key(input) {
            return Ok(input.to_string());
        }

        let matches: Vec<&str> = self
            .topological_order()?
            .into_iter()
            .filter(|id| id.starts_with(input))
            .collect();
        match matches.as_slice() {
            [] => Err(GraphError::UnknownId(input.to_string())),
            [id] => Ok((*id).to_string()),
            _ => Err(GraphError::AmbiguousId {
                prefix: input.to_string(),
                matches: matches.join(", "),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn migration(id: &str, deps: &[&str]) -> Migration {
        Migration {
            id: id.to_string(),
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
            operations: vec![],
            atomic: true,
        }
    }

    fn linear_graph() -> MigrationGraph {
        // 0001 → 0002 → 0003
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0002_add_users", &["0001_initial"]))
            .unwrap();
        g.add(migration("0003_add_posts", &["0002_add_users"]))
            .unwrap();
        g
    }

    /// Adding the same migration id twice must return DuplicateId.
    #[test]
    fn add_duplicate_id_is_error() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        let err = g.add(migration("0001_initial", &[])).unwrap_err();
        assert!(matches!(err, GraphError::DuplicateId(id) if id == "0001_initial"));
    }

    /// An empty graph has no heads.
    #[test]
    fn empty_graph_has_no_heads() {
        let g = MigrationGraph::new();
        assert!(g.heads().is_empty());
    }

    /// A single migration with no dependencies is the sole head.
    #[test]
    fn single_migration_is_head() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        assert_eq!(g.heads(), vec!["0001_initial"]);
    }

    /// In a linear chain, only the last migration is the head.
    #[test]
    fn linear_chain_has_one_head() {
        let g = linear_graph();
        assert_eq!(g.heads(), vec!["0003_add_posts"]);
    }

    /// Branching graph — two migrations from the same root — produces two heads.
    #[test]
    fn branching_graph_has_two_heads() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0002_branch_a", &["0001_initial"]))
            .unwrap();
        g.add(migration("0003_branch_b", &["0001_initial"]))
            .unwrap();
        let mut heads = g.heads();
        heads.sort();
        assert_eq!(heads, vec!["0002_branch_a", "0003_branch_b"]);
    }

    /// detect_conflict returns Ok for a linear (single-head) graph.
    #[test]
    fn detect_conflict_ok_for_linear_graph() {
        assert!(linear_graph().detect_conflict().is_ok());
    }

    /// detect_conflict returns Conflict when multiple heads exist.
    #[test]
    fn detect_conflict_err_for_branched_graph() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0002_branch_a", &["0001_initial"]))
            .unwrap();
        g.add(migration("0003_branch_b", &["0001_initial"]))
            .unwrap();
        assert!(matches!(g.detect_conflict(), Err(GraphError::Conflict)));
    }

    /// validate_acyclic passes for a valid DAG.
    #[test]
    fn validate_acyclic_passes_for_dag() {
        assert!(linear_graph().validate_acyclic().is_ok());
    }

    /// validate_acyclic returns CycleDetected when a cycle exists.
    #[test]
    fn validate_acyclic_detects_cycle() {
        let mut g = MigrationGraph::new();
        // A depends on B, B depends on A
        g.add(migration("A", &["B"])).unwrap();
        g.add(migration("B", &["A"])).unwrap();
        assert!(matches!(
            g.validate_acyclic(),
            Err(GraphError::CycleDetected)
        ));
    }

    /// validate_acyclic detects a longer cycle (A → B → C → A).
    #[test]
    fn validate_acyclic_detects_longer_cycle() {
        let mut g = MigrationGraph::new();
        g.add(migration("A", &["C"])).unwrap();
        g.add(migration("B", &["A"])).unwrap();
        g.add(migration("C", &["B"])).unwrap();
        assert!(matches!(
            g.validate_acyclic(),
            Err(GraphError::CycleDetected)
        ));
    }

    /// topological_order on a linear chain returns migrations in dependency order.
    #[test]
    fn topological_order_linear_chain() {
        let g = linear_graph();
        let order = g.topological_order().unwrap();
        assert_eq!(
            order,
            vec!["0001_initial", "0002_add_users", "0003_add_posts"]
        );
    }

    /// topological_order is deterministic: dependencies always precede dependents.
    #[test]
    fn topological_order_respects_dependencies() {
        let mut g = MigrationGraph::new();
        g.add(migration("0003_c", &["0002_b"])).unwrap();
        g.add(migration("0001_a", &[])).unwrap();
        g.add(migration("0002_b", &["0001_a"])).unwrap();
        let order = g.topological_order().unwrap();
        let pos = |id: &str| order.iter().position(|&s| s == id).unwrap();
        assert!(pos("0001_a") < pos("0002_b"));
        assert!(pos("0002_b") < pos("0003_c"));
    }

    /// topological_order fails on a cyclic graph.
    #[test]
    fn topological_order_fails_on_cycle() {
        let mut g = MigrationGraph::new();
        g.add(migration("A", &["B"])).unwrap();
        g.add(migration("B", &["A"])).unwrap();
        assert!(matches!(
            g.topological_order(),
            Err(GraphError::CycleDetected)
        ));
    }

    /// next_number returns 1 for an empty graph.
    #[test]
    fn next_number_empty_graph() {
        assert_eq!(MigrationGraph::new().next_number(), 1);
    }

    /// next_number returns max + 1 for an existing graph.
    #[test]
    fn next_number_increments_from_max() {
        assert_eq!(linear_graph().next_number(), 4);
    }

    /// next_number handles gaps in numbering correctly.
    #[test]
    fn next_number_handles_gaps() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_a", &[])).unwrap();
        g.add(migration("0005_b", &["0001_a"])).unwrap();
        assert_eq!(g.next_number(), 6);
    }

    /// create_merge_migration fails when there is only one head.
    #[test]
    fn create_merge_migration_requires_multiple_heads() {
        let err = linear_graph()
            .create_merge_migration("merge".to_string())
            .unwrap_err();
        assert!(matches!(err, GraphError::Empty));
    }

    /// create_merge_migration produces a migration with all heads as sorted dependencies.
    #[test]
    fn create_merge_migration_depends_on_all_heads() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0002_branch_a", &["0001_initial"]))
            .unwrap();
        g.add(migration("0003_branch_b", &["0001_initial"]))
            .unwrap();
        let merge = g.create_merge_migration("0004_merge".to_string()).unwrap();
        assert_eq!(merge.id, "0004_merge");
        assert_eq!(merge.dependencies, vec!["0002_branch_a", "0003_branch_b"]);
        assert!(merge.operations.is_empty());
    }

    /// topological_order returns UnknownDependency when a dependency does not exist in the graph.
    #[test]
    fn topological_order_fails_on_unknown_dependency() {
        let mut g = MigrationGraph::new();
        g.add(migration("0002_b", &["0001_ghost"])).unwrap();
        let err = g.topological_order().unwrap_err();
        assert!(matches!(err, GraphError::UnknownDependency { dep, .. } if dep == "0001_ghost"));
    }

    /// topological_order is stable regardless of the order nodes were added.
    #[test]
    fn topological_order_stable_regardless_of_insertion_order() {
        let mut g1 = MigrationGraph::new();
        g1.add(migration("0003_add_posts", &["0002_add_users"]))
            .unwrap();
        g1.add(migration("0001_initial", &[])).unwrap();
        g1.add(migration("0002_add_users", &["0001_initial"]))
            .unwrap();

        let mut g2 = MigrationGraph::new();
        g2.add(migration("0001_initial", &[])).unwrap();
        g2.add(migration("0002_add_users", &["0001_initial"]))
            .unwrap();
        g2.add(migration("0003_add_posts", &["0002_add_users"]))
            .unwrap();

        assert_eq!(
            g1.topological_order().unwrap(),
            g2.topological_order().unwrap()
        );
        assert_eq!(
            g1.topological_order().unwrap(),
            vec!["0001_initial", "0002_add_users", "0003_add_posts"]
        );
    }

    /// Parallel branches are always ordered alphabetically (not by HashMap iteration).
    #[test]
    fn topological_order_parallel_branches_alphabetical() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0003_feature_b", &["0001_initial"]))
            .unwrap();
        g.add(migration("0002_feature_a", &["0001_initial"]))
            .unwrap();
        g.add(migration(
            "0004_merge",
            &["0002_feature_a", "0003_feature_b"],
        ))
        .unwrap();
        let order = g.topological_order().unwrap();
        assert_eq!(order[0], "0001_initial");
        assert_eq!(order[3], "0004_merge");
        let pos = |id: &str| order.iter().position(|&s| s == id).unwrap();
        assert!(
            pos("0002_feature_a") < pos("0003_feature_b"),
            "0002 should come before 0003 alphabetically"
        );
        assert!(pos("0002_feature_a") < pos("0004_merge"));
        assert!(pos("0003_feature_b") < pos("0004_merge"));
    }

    /// topological_order called multiple times on the same graph returns identical results.
    #[test]
    fn topological_order_is_stable_across_calls() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0003_z_feature", &["0001_initial"]))
            .unwrap();
        g.add(migration("0002_a_feature", &["0001_initial"]))
            .unwrap();
        g.add(migration(
            "0004_merge",
            &["0002_a_feature", "0003_z_feature"],
        ))
        .unwrap();
        let first = g.topological_order().unwrap();
        let second = g.topological_order().unwrap();
        assert_eq!(first, second);
        let pos = |id: &str| first.iter().position(|&s| s == id).unwrap();
        assert!(pos("0002_a_feature") < pos("0003_z_feature"));
    }

    /// create_merge_migration always produces sorted dependencies regardless of heads() order.
    #[test]
    fn create_merge_migration_deps_are_always_sorted() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("0002_zzz_last", &["0001_initial"]))
            .unwrap();
        g.add(migration("0003_aaa_first", &["0001_initial"]))
            .unwrap();
        let merge = g.create_merge_migration("0004_merge".to_string()).unwrap();
        assert_eq!(merge.dependencies, vec!["0002_zzz_last", "0003_aaa_first"]);
    }

    /// validate_id rejects empty strings and strings with invalid characters.
    #[test]
    fn validate_id_rejects_invalid() {
        assert!(MigrationGraph::validate_id("").is_err());
        assert!(MigrationGraph::validate_id("has space").is_err());
        assert!(MigrationGraph::validate_id("HasUpper").is_err());
        assert!(MigrationGraph::validate_id("has-dash").is_err());
        assert!(MigrationGraph::validate_id("0001_ok").is_ok());
        assert!(MigrationGraph::validate_id("abc123").is_ok());
    }

    /// Resolves a unique migration id prefix using deterministic graph order.
    #[test]
    fn resolve_id_accepts_unique_prefix() {
        let graph = linear_graph();

        assert_eq!(graph.resolve_id("0002").unwrap(), "0002_add_users");
    }

    /// Prefers a full id even when another id begins with the same text.
    #[test]
    fn resolve_id_prefers_exact_match() {
        let mut graph = MigrationGraph::new();
        graph.add(migration("0001_users", &[])).unwrap();
        graph.add(migration("0001_users_index", &[])).unwrap();

        assert_eq!(graph.resolve_id("0001_users").unwrap(), "0001_users");
    }

    /// Reports all deterministic candidates when a prefix is ambiguous.
    #[test]
    fn resolve_id_rejects_ambiguous_prefix() {
        let graph = linear_graph();

        let error = graph.resolve_id("000").unwrap_err();
        assert!(matches!(
            error,
            GraphError::AmbiguousId { prefix, matches }
                if prefix == "000" && matches == "0001_initial, 0002_add_users, 0003_add_posts"
        ));
    }

    /// next_number ignores namespaced ids (containing '/') and only counts primary ones.
    #[test]
    fn next_number_ignores_namespaced_ids() {
        let mut g = MigrationGraph::new();
        g.add(migration("0001_initial", &[])).unwrap();
        g.add(migration("auth/0001_users", &[])).unwrap();
        g.add(migration("auth/0002_sessions", &[])).unwrap();
        // namespaced ids must not inflate the primary counter
        assert_eq!(g.next_number(), 2);
    }

    /// next_number returns 1 when only namespaced ids are present.
    #[test]
    fn next_number_with_only_namespaced_ids() {
        let mut g = MigrationGraph::new();
        g.add(migration("auth/0005_something", &[])).unwrap();
        assert_eq!(g.next_number(), 1);
    }
}
