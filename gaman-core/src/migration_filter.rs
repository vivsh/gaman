//! Invocation-scoped root filtering for migration generation.

use std::collections::{BTreeSet, HashSet};

use crate::clarifier::table_rename_candidates;
use crate::diff::dependency_closure;
use crate::entity_filter::EntityFilter;
use crate::operations::Operation;
use crate::states::{EntityKind, Schema};

pub(crate) struct FilteredOperations {
    pub(crate) operations: Vec<Operation>,
    pub(crate) table_roots: HashSet<String>,
}

pub(crate) fn filter_operations(
    operations: &[Operation],
    filters: &[EntityFilter],
    desired: &Schema,
    previous: &Schema,
) -> Result<FilteredOperations, String> {
    if filters.is_empty() {
        return Ok(FilteredOperations {
            operations: operations.to_vec(),
            table_roots: operations
                .iter()
                .filter_map(operation_root)
                .filter(|(kind, _)| *kind == EntityKind::Table)
                .map(|(_, name)| name)
                .collect(),
        });
    }
    validate_known_filters(filters, desired, previous)?;
    let mut seeds = operations
        .iter()
        .enumerate()
        .filter_map(|(index, operation)| {
            operation_root(operation)
                .filter(|(kind, name)| filters.iter().any(|filter| filter.matches(*kind, name)))
                .map(|_| index)
        })
        .collect::<HashSet<_>>();
    expand_rename_pairs(operations, filters, &mut seeds);
    expand_dropped_table_dependents(operations, previous, &mut seeds);
    let selected = dependency_closure(operations, &seeds);
    let table_roots = selected
        .iter()
        .filter_map(|index| operation_root(&operations[*index]))
        .filter(|(kind, _)| *kind == EntityKind::Table)
        .map(|(_, name)| name)
        .collect();
    Ok(FilteredOperations {
        operations: operations
            .iter()
            .enumerate()
            .filter(|(index, _)| selected.contains(index))
            .map(|(_, operation)| operation.clone())
            .collect(),
        table_roots,
    })
}

/// Includes changed child-table drops required before a selected parent drop;
/// compound table operations do not expose these foreign keys as separate ops.
fn expand_dropped_table_dependents(
    operations: &[Operation],
    previous: &Schema,
    seeds: &mut HashSet<usize>,
) {
    loop {
        let selected = seeds
            .iter()
            .filter_map(|index| match &operations[*index] {
                Operation::DropTable { table } => Some(table.qualified_name()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let mut changed = false;
        for (index, operation) in operations.iter().enumerate() {
            let Operation::DropTable { table } = operation else {
                continue;
            };
            let Some(previous_table) = previous.tables.get(&table.qualified_name()) else {
                continue;
            };
            if previous_table
                .foreign_keys
                .iter()
                .any(|foreign_key| selected.contains(&foreign_key.to_table))
            {
                changed |= seeds.insert(index);
            }
        }
        if !changed {
            break;
        }
    }
}

fn validate_known_filters(
    filters: &[EntityFilter],
    desired: &Schema,
    previous: &Schema,
) -> Result<(), String> {
    for filter in filters {
        if schema_roots(desired)
            .chain(schema_roots(previous))
            .any(|(kind, name)| filter.matches(kind, name))
        {
            continue;
        }
        return Err(format!(
            "migration filter '{}:{pattern}' matched no known root entity",
            kind_name(filter.kind),
            pattern = filter.pattern
        ));
    }
    Ok(())
}

fn expand_rename_pairs(
    operations: &[Operation],
    filters: &[EntityFilter],
    seeds: &mut HashSet<usize>,
) {
    let candidates = table_rename_candidates(operations);
    for (old, candidate) in selected_rename_pairs(&candidates, filters) {
        for (index, operation) in operations.iter().enumerate() {
            if operation_root(operation).is_some_and(|(kind, name)| {
                kind == EntityKind::Table && (name == old || name == candidate)
            }) {
                seeds.insert(index);
            }
        }
    }
}

/// Selects stable one-to-one rename pairs without pulling lower-ranked,
/// unrelated old roots into clarification.
fn selected_rename_pairs(
    candidates: &[(String, Vec<String>)],
    filters: &[EntityFilter],
) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut used_old = HashSet::new();
    let mut used_new = HashSet::new();
    for (old, ranked) in candidates {
        if matches_table_filter(filters, old) {
            claim_pair(
                old,
                ranked.first(),
                &mut pairs,
                &mut used_old,
                &mut used_new,
            );
        }
    }
    let matched_new = candidates
        .iter()
        .flat_map(|(_, ranked)| ranked)
        .filter(|candidate| matches_table_filter(filters, candidate))
        .cloned()
        .collect::<BTreeSet<_>>();
    for new in matched_new {
        if used_new.contains(&new) {
            continue;
        }
        let best = candidates
            .iter()
            .filter(|(old, _)| !used_old.contains(old))
            .filter_map(|(old, ranked)| {
                ranked
                    .iter()
                    .position(|value| value == &new)
                    .map(|rank| (rank, old))
            })
            .min_by(|left, right| left.cmp(right));
        if let Some((_, old)) = best {
            claim_pair(old, Some(&new), &mut pairs, &mut used_old, &mut used_new);
        }
    }
    pairs
}

fn matches_table_filter(filters: &[EntityFilter], identity: &str) -> bool {
    filters
        .iter()
        .any(|filter| filter.matches(EntityKind::Table, identity))
}

fn claim_pair(
    old: &str,
    new: Option<&String>,
    pairs: &mut Vec<(String, String)>,
    used_old: &mut HashSet<String>,
    used_new: &mut HashSet<String>,
) {
    let Some(new) = new.filter(|new| !used_new.contains(*new)) else {
        return;
    };
    pairs.push((old.to_string(), new.clone()));
    used_old.insert(old.to_string());
    used_new.insert(new.clone());
}

fn schema_roots(schema: &Schema) -> impl Iterator<Item = (EntityKind, &str)> {
    schema
        .tables
        .keys()
        .map(|name| (EntityKind::Table, name.as_str()))
        .chain(
            schema
                .views
                .keys()
                .map(|name| (EntityKind::View, name.as_str())),
        )
        .chain(
            schema
                .functions
                .keys()
                .map(|name| (EntityKind::Function, name.as_str())),
        )
        .chain(
            schema
                .enums
                .keys()
                .map(|name| (EntityKind::Enum, name.as_str())),
        )
        .chain(
            schema
                .extensions
                .keys()
                .map(|name| (EntityKind::Extension, name.as_str())),
        )
        .chain(
            schema
                .sequences
                .keys()
                .map(|name| (EntityKind::Sequence, name.as_str())),
        )
}

fn operation_root(operation: &Operation) -> Option<(EntityKind, String)> {
    match operation {
        Operation::CreateTable { table } | Operation::DropTable { table } => {
            Some((EntityKind::Table, table.qualified_name()))
        }
        _ if operation.table_name().is_some() => operation
            .table_name()
            .map(|name| (EntityKind::Table, name.to_string())),
        _ => match operation.entity_kind()? {
            kind @ (EntityKind::View
            | EntityKind::Function
            | EntityKind::Enum
            | EntityKind::Extension
            | EntityKind::Sequence) => Some((kind, operation.entity_name().to_string())),
            _ => None,
        },
    }
}

fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Table => "table",
        EntityKind::View => "view",
        EntityKind::Function => "function",
        EntityKind::Enum => "enum",
        EntityKind::Extension => "extension",
        EntityKind::Sequence => "sequence",
        _ => "unsupported",
    }
}
