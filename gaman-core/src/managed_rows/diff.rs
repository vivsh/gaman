use std::collections::{BTreeSet, HashMap, HashSet};

use crate::diff::DiffError;
use crate::operations::Operation;
use crate::states::Schema;

/// Produces deterministic row operations from desired and replayed state.
pub(crate) fn diff_schemas(current: &Schema, previous: &Schema) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (table_name, desired) in &current.managed_rows {
        let desired_key = current
            .tables
            .get(table_name)
            .and_then(|table| super::validation::resolve_key(table, desired))
            .unwrap_or_default();
        let desired_rows = desired.row_map(&desired_key).unwrap_or_default();
        let previous_rows = previous
            .managed_rows
            .get(table_name)
            .and_then(|rows| {
                previous
                    .tables
                    .get(table_name)
                    .and_then(|table| super::validation::resolve_key(table, rows))
                    .and_then(|key| rows.row_map(&key).ok())
            })
            .unwrap_or_default();
        for (identity, row) in &desired_rows {
            match previous_rows.get(identity) {
                None => operations.push(Operation::InsertRow {
                    table_name: table_name.clone(),
                    key: desired_key.clone(),
                    row: row.clone(),
                }),
                Some(old) if old != row => operations.push(Operation::UpdateRow {
                    table_name: table_name.clone(),
                    key: desired_key.clone(),
                    old: old.clone(),
                    new: row.clone(),
                }),
                _ => {}
            }
        }
        if current.tables.contains_key(table_name) {
            for (identity, row) in previous_rows {
                if !desired_rows.contains_key(&identity) {
                    operations.push(Operation::DeleteRow {
                        table_name: table_name.clone(),
                        key: desired_key.clone(),
                        row,
                    });
                }
            }
        }
    }
    for (table_name, previous_rows) in &previous.managed_rows {
        if current.managed_rows.contains_key(table_name) || !current.tables.contains_key(table_name)
        {
            continue;
        }
        let previous_key = previous
            .tables
            .get(table_name)
            .and_then(|table| super::validation::resolve_key(table, previous_rows))
            .unwrap_or_default();
        for row in &previous_rows.rows {
            operations.push(Operation::DeleteRow {
                table_name: table_name.clone(),
                key: previous_key.clone(),
                row: row.clone(),
            });
        }
    }
    operations
}

/// Orders row writes by modeled foreign keys without disturbing structural slots.
pub(crate) fn order_operations(
    mut operations: Vec<Operation>,
    current: &Schema,
    previous: &Schema,
) -> Result<Vec<Operation>, DiffError> {
    order_group(&mut operations, current, false)?;
    order_group(&mut operations, previous, true)?;
    Ok(operations)
}

fn order_group(
    operations: &mut [Operation],
    schema: &Schema,
    deleting: bool,
) -> Result<(), DiffError> {
    let slots = operations
        .iter()
        .enumerate()
        .filter(|(_, operation)| is_group_member(operation, deleting))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if slots.len() < 2 {
        return Ok(());
    }
    let ordered = topological_row_order(operations, &slots, schema, deleting)?;
    let replacements = ordered
        .into_iter()
        .map(|index| operations[index].clone())
        .collect::<Vec<_>>();
    for (slot, operation) in slots.into_iter().zip(replacements) {
        operations[slot] = operation;
    }
    Ok(())
}

fn topological_row_order(
    operations: &[Operation],
    slots: &[usize],
    schema: &Schema,
    deleting: bool,
) -> Result<Vec<usize>, DiffError> {
    let by_table = row_operations_by_table(operations, slots);
    let mut edges = vec![Vec::new(); slots.len()];
    let mut incoming = vec![0_usize; slots.len()];
    add_foreign_key_edges(schema, &by_table, deleting, &mut edges, &mut incoming);
    let mut ready = BTreeSet::new();
    for (local, slot) in slots.iter().enumerate() {
        if incoming[local] == 0 {
            ready.insert((operations[*slot].entity_name().to_string(), local));
        }
    }
    let mut ordered = Vec::with_capacity(slots.len());
    while let Some((name, local)) = ready.pop_first() {
        let _ = name;
        ordered.push(slots[local]);
        for dependent in &edges[local] {
            incoming[*dependent] -= 1;
            if incoming[*dependent] == 0 {
                let slot = slots[*dependent];
                ready.insert((operations[slot].entity_name().to_string(), *dependent));
            }
        }
    }
    (ordered.len() == slots.len())
        .then_some(ordered)
        .ok_or(DiffError::DependencyCycle)
}

fn row_operations_by_table<'a>(
    operations: &'a [Operation],
    slots: &[usize],
) -> HashMap<&'a str, Vec<usize>> {
    slots
        .iter()
        .enumerate()
        .fold(HashMap::new(), |mut grouped, (local, slot)| {
            if let Some(table) = operations[*slot].table_name() {
                grouped.entry(table).or_default().push(local);
            }
            grouped
        })
}

fn add_foreign_key_edges(
    schema: &Schema,
    by_table: &HashMap<&str, Vec<usize>>,
    deleting: bool,
    edges: &mut [Vec<usize>],
    incoming: &mut [usize],
) {
    let mut seen = HashSet::new();
    for (child, table) in &schema.tables {
        for foreign_key in &table.foreign_keys {
            let (Some(children), Some(parents)) = (
                by_table.get(child.as_str()),
                by_table.get(foreign_key.to_table.as_str()),
            ) else {
                continue;
            };
            for child in children {
                for parent in parents {
                    let edge = if deleting {
                        (*child, *parent)
                    } else {
                        (*parent, *child)
                    };
                    if seen.insert(edge) {
                        edges[edge.0].push(edge.1);
                        incoming[edge.1] += 1;
                    }
                }
            }
        }
    }
}

fn is_group_member(operation: &Operation, deleting: bool) -> bool {
    if deleting {
        matches!(operation, Operation::DeleteRow { .. })
    } else {
        matches!(
            operation,
            Operation::InsertRow { .. } | Operation::UpdateRow { .. }
        )
    }
}
