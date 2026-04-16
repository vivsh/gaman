use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::collections::hash_map::DefaultHasher;

use thiserror::Error;

use crate::operations::Operation;
use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef,
    Index, Schema, Table, TriggerDef, ViewDef,
};

#[derive(Debug, Error)]
pub enum DiffError {}

// ---------------------------------------------------------------------------
// DiffNode — the unified entity representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EntityKind {
    Table,
    Column,
    ForeignKey,
    Index,
    Constraint,
    Trigger,
    Function,
    View,
    Extension,
    Enum,
}

// Carries borrowed references into the state so the detection pass never
// needs to clone. Cloning happens only in the emission pass when building
// actual Operation variants.
#[derive(Debug, Clone)]
struct DiffNode<'a> {
    kind: EntityKind,
    // hash of the identity key (table_name, entity_name, …)
    id: u64,
    // hash of all mutable content — O(1) changed-check once pre-computed
    attrs: u64,
    // 0 for top-level entities; hash of parent identity key for sub-entities
    parent_id: u64,
    payload: Payload<'a>,
}

#[derive(Debug, Clone)]
enum Payload<'a> {
    Table(&'a Table),
    Column { table_name: &'a str, col: &'a Column },
    ForeignKey { table_name: &'a str, fk: &'a ForeignKey },
    Index { table_name: &'a str, idx: &'a Index },
    Constraint { table_name: &'a str, con: &'a Constraint },
    Trigger { table_name: &'a str, trg: &'a TriggerDef },
    Function(&'a FunctionDef),
    View(&'a ViewDef),
    Extension(&'a ExtensionDef),
    Enum(&'a EnumDef),
}

fn hash_one<T: Hash>(v: &T) -> u64 {
    let mut h = DefaultHasher::new();
    v.hash(&mut h);
    h.finish()
}

fn hash_str(s: &str) -> u64 {
    hash_one(&s)
}

// ---------------------------------------------------------------------------
// State → DiffNode flattening
// Pre-computes all node hashes in a single pass over SchemaState.
// Top-level attrs hashes exclude sub-entity content so a column change does
// not falsely mark the parent table as modified.
// ---------------------------------------------------------------------------

fn flatten<'a>(state: &'a Schema) -> Vec<DiffNode<'a>> {
    let mut nodes: Vec<DiffNode<'a>> = Vec::new();

    for (_, table) in &state.tables {
        let table_id = hash_str(&table.name);
        // Table-level attrs: only schema (the only non-child mutable field).
        // Children each become their own nodes, so we do not hash them here.
        let table_attrs = hash_one(&table.schema);
        nodes.push(DiffNode {
            kind: EntityKind::Table,
            id: table_id,
            attrs: table_attrs,
            parent_id: 0,
            payload: Payload::Table(table),
        });

        for col in &table.columns {
            let col_id = hash_str(&format!("{}:{}", table.name, col.name));
            let col_attrs = {
                let mut h = DefaultHasher::new();
                col.col_type.hash(&mut h);
                col.nullable.hash(&mut h);
                col.default.hash(&mut h);
                col.primary_key.hash(&mut h);
                h.finish()
            };
            nodes.push(DiffNode {
                kind: EntityKind::Column,
                id: col_id,
                attrs: col_attrs,
                parent_id: table_id,
                payload: Payload::Column { table_name: &table.name, col },
            });
        }

        for fk in &table.foreign_keys {
            let fk_id = hash_str(&format!("{}:{}", table.name, fk.name));
            let fk_attrs = {
                let mut h = DefaultHasher::new();
                fk.from_column.hash(&mut h);
                fk.to_table.hash(&mut h);
                fk.to_column.hash(&mut h);
                h.finish()
            };
            nodes.push(DiffNode {
                kind: EntityKind::ForeignKey,
                id: fk_id,
                attrs: fk_attrs,
                parent_id: table_id,
                payload: Payload::ForeignKey { table_name: &table.name, fk },
            });
        }

        for idx in &table.indexes {
            let idx_id = hash_str(&format!("{}:{}", table.name, idx.name));
            let idx_attrs = {
                let mut h = DefaultHasher::new();
                idx.columns.hash(&mut h);
                idx.unique.hash(&mut h);
                idx.predicate.hash(&mut h);
                h.finish()
            };
            nodes.push(DiffNode {
                kind: EntityKind::Index,
                id: idx_id,
                attrs: idx_attrs,
                parent_id: table_id,
                payload: Payload::Index { table_name: &table.name, idx },
            });
        }

        for con in &table.constraints {
            let con_id = hash_str(&format!("{}:{}", table.name, con.name()));
            let con_attrs = hash_one(con);
            nodes.push(DiffNode {
                kind: EntityKind::Constraint,
                id: con_id,
                attrs: con_attrs,
                parent_id: table_id,
                payload: Payload::Constraint { table_name: &table.name, con },
            });
        }

        for trg in &table.triggers {
            let name = trg.name.as_deref().unwrap_or("");
            let trg_id = hash_str(&format!("{}:{}", table.name, name));
            let trg_attrs = {
                let mut h = DefaultHasher::new();
                trg.timing.hash(&mut h);
                trg.events.hash(&mut h);
                trg.scope.hash(&mut h);
                trg.function_name.hash(&mut h);
                trg.when.hash(&mut h);
                trg.body.hash(&mut h);
                trg.language.hash(&mut h);
                h.finish()
            };
            nodes.push(DiffNode {
                kind: EntityKind::Trigger,
                id: trg_id,
                attrs: trg_attrs,
                parent_id: table_id,
                payload: Payload::Trigger { table_name: &table.name, trg },
            });
        }
    }

    for (_, func) in &state.functions {
        let fn_id = hash_str(&func.name);
        let fn_attrs = {
            let mut h = DefaultHasher::new();
            func.schema.hash(&mut h);
            func.arguments.hash(&mut h);
            func.returns.hash(&mut h);
            func.language.hash(&mut h);
            func.body.hash(&mut h);
            func.volatility.hash(&mut h);
            func.security_definer.hash(&mut h);
            h.finish()
        };
        nodes.push(DiffNode {
            kind: EntityKind::Function,
            id: fn_id,
            attrs: fn_attrs,
            parent_id: 0,
            payload: Payload::Function(func),
        });
    }

    for (_, view) in &state.views {
        let view_id = hash_str(&view.name);
        let view_attrs = {
            let mut h = DefaultHasher::new();
            view.schema.hash(&mut h);
            view.definition.hash(&mut h);
            h.finish()
        };
        nodes.push(DiffNode {
            kind: EntityKind::View,
            id: view_id,
            attrs: view_attrs,
            parent_id: 0,
            payload: Payload::View(view),
        });
    }

    for (_, ext) in &state.extensions {
        let ext_id = hash_str(&ext.name);
        let ext_attrs = {
            let mut h = DefaultHasher::new();
            ext.schema.hash(&mut h);
            ext.version.hash(&mut h);
            h.finish()
        };
        nodes.push(DiffNode {
            kind: EntityKind::Extension,
            id: ext_id,
            attrs: ext_attrs,
            parent_id: 0,
            payload: Payload::Extension(ext),
        });
    }

    for (_, en) in &state.enums {
        let en_id = hash_str(&en.name);
        let en_attrs = {
            let mut h = DefaultHasher::new();
            en.schema.hash(&mut h);
            en.values.hash(&mut h);
            h.finish()
        };
        nodes.push(DiffNode {
            kind: EntityKind::Enum,
            id: en_id,
            attrs: en_attrs,
            parent_id: 0,
            payload: Payload::Enum(en),
        });
    }

    nodes
}

// ---------------------------------------------------------------------------
// Detection — single unified loop over nodes from both states
// ---------------------------------------------------------------------------

enum Change<'a> {
    Added(DiffNode<'a>),
    Removed(DiffNode<'a>),
    Modified { prev: DiffNode<'a>, curr: DiffNode<'a> },
}

fn detect<'a>(current: &'a Schema, previous: &'a Schema) -> Vec<Change<'a>> {
    let prev_nodes = flatten(previous);
    let curr_nodes = flatten(current);

    let prev_map: HashMap<(u8, u64), DiffNode<'a>> = prev_nodes
        .into_iter()
        .map(|n| ((n.kind as u8, n.id), n))
        .collect();
    let curr_map: HashMap<(u8, u64), DiffNode<'a>> = curr_nodes
        .into_iter()
        .map(|n| ((n.kind as u8, n.id), n))
        .collect();

    let mut changes: Vec<Change<'a>> = Vec::new();

    for (key, curr_node) in &curr_map {
        match prev_map.get(key) {
            None => changes.push(Change::Added(curr_node.clone())),
            Some(prev_node) if prev_node.attrs != curr_node.attrs => {
                changes.push(Change::Modified { prev: prev_node.clone(), curr: curr_node.clone() });
            }
            _ => {}
        }
    }

    for (key, prev_node) in &prev_map {
        if !curr_map.contains_key(key) {
            changes.push(Change::Removed(prev_node.clone()));
        }
    }

    changes
}

// ---------------------------------------------------------------------------
// Emission — kind-specific dispatch from Change to Vec<Operation>
// ---------------------------------------------------------------------------

fn emit(change: Change<'_>) -> Vec<Operation> {
    match change {
        Change::Added(node) => match node.payload {
            Payload::Table(t) => vec![Operation::CreateTable { table: t.clone() }],
            Payload::Column { table_name, col } => vec![Operation::AddColumn {
                table_name: table_name.to_string(),
                column: col.clone(),
            }],
            Payload::ForeignKey { table_name, fk } => vec![Operation::AddForeignKey {
                table_name: table_name.to_string(),
                foreign_key: fk.clone(),
            }],
            Payload::Index { table_name, idx } => vec![Operation::AddIndex {
                table_name: table_name.to_string(),
                index: idx.clone(),
                concurrent: false,
            }],
            Payload::Constraint { table_name, con } => vec![Operation::AddConstraint {
                table_name: table_name.to_string(),
                constraint: con.clone(),
            }],
            Payload::Trigger { table_name, trg } => vec![Operation::CreateTrigger {
                table_name: table_name.to_string(),
                trigger: trg.clone(),
            }],
            Payload::Function(f) => vec![Operation::CreateFunction { function: f.clone() }],
            Payload::View(v) => vec![Operation::CreateView { view: v.clone() }],
            Payload::Extension(e) => vec![Operation::CreateExtension { extension: e.clone() }],
            Payload::Enum(e) => vec![Operation::CreateEnum { enum_def: e.clone() }],
        },

        Change::Removed(node) => match node.payload {
            Payload::Table(t) => vec![Operation::DropTable { table: t.clone() }],
            Payload::Column { table_name, col } => vec![Operation::DropColumn {
                table_name: table_name.to_string(),
                column: col.clone(),
                cascade: false,
            }],
            Payload::ForeignKey { table_name, fk } => vec![Operation::DropForeignKey {
                table_name: table_name.to_string(),
                foreign_key: fk.clone(),
                cascade: false,
            }],
            Payload::Index { table_name, idx } => vec![Operation::DropIndex {
                table_name: table_name.to_string(),
                index: idx.clone(),
                concurrent: false,
            }],
            Payload::Constraint { table_name, con } => vec![Operation::DropConstraint {
                table_name: table_name.to_string(),
                constraint: con.clone(),
            }],
            Payload::Trigger { table_name, trg } => vec![Operation::DropTrigger {
                table_name: table_name.to_string(),
                trigger: trg.clone(),
            }],
            Payload::Function(f) => vec![Operation::DropFunction { function: f.clone() }],
            Payload::View(v) => vec![Operation::DropView { view: v.clone() }],
            Payload::Extension(e) => vec![Operation::DropExtension { extension: e.clone() }],
            Payload::Enum(e) => vec![Operation::DropEnum { enum_def: e.clone() }],
        },

        Change::Modified { prev, curr } => match (prev.payload, curr.payload) {
            (Payload::Column { table_name, col: old }, Payload::Column { col: new, .. }) => {
                vec![Operation::AlterColumn {
                    table_name: table_name.to_string(),
                    old: old.clone(),
                    new: new.clone(),
                    cast_expr: None,
                }]
            }
            (Payload::ForeignKey { table_name, fk: old }, Payload::ForeignKey { fk: new, .. }) => {
                vec![
                    Operation::DropForeignKey { table_name: table_name.to_string(), foreign_key: old.clone(), cascade: false },
                    Operation::AddForeignKey { table_name: table_name.to_string(), foreign_key: new.clone() },
                ]
            }
            (Payload::Index { table_name, idx: old }, Payload::Index { idx: new, .. }) => {
                vec![
                    Operation::DropIndex { table_name: table_name.to_string(), index: old.clone(), concurrent: false },
                    Operation::AddIndex { table_name: table_name.to_string(), index: new.clone(), concurrent: false },
                ]
            }
            (Payload::Constraint { table_name, con: old }, Payload::Constraint { con: new, .. }) => {
                vec![
                    Operation::DropConstraint { table_name: table_name.to_string(), constraint: old.clone() },
                    Operation::AddConstraint { table_name: table_name.to_string(), constraint: new.clone() },
                ]
            }
            (Payload::Trigger { table_name, trg: old }, Payload::Trigger { trg: new, .. }) => {
                vec![Operation::AlterTrigger {
                    table_name: table_name.to_string(),
                    old: old.clone(),
                    new: new.clone(),
                }]
            }
            (Payload::Function(old), Payload::Function(new)) => {
                vec![Operation::AlterFunction { old: old.clone(), new: new.clone() }]
            }
            (Payload::View(old), Payload::View(new)) => {
                vec![Operation::ReplaceView { old: old.clone(), new: new.clone() }]
            }
            (Payload::Extension(old), Payload::Extension(new)) => {
                vec![
                    Operation::DropExtension { extension: old.clone() },
                    Operation::CreateExtension { extension: new.clone() },
                ]
            }
            (Payload::Enum(old), Payload::Enum(new)) => {
                if is_append_only(old, new) {
                    vec![Operation::AlterEnum { old: old.clone(), new: new.clone() }]
                } else {
                    vec![
                        Operation::DropEnum { enum_def: old.clone() },
                        Operation::CreateEnum { enum_def: new.clone() },
                    ]
                }
            }
            // Table-level attrs changed (schema rename etc.) — no operation type exists yet.
            // Silently skip; will be handled when AlterTable is added.
            _ => vec![],
        },
    }
}

fn is_append_only(old: &EnumDef, new: &EnumDef) -> bool {
    let mut old_iter = old.values.iter().peekable();
    for val in &new.values {
        if old_iter.peek() == Some(&val) {
            old_iter.next();
        }
    }
    old_iter.next().is_none()
}

// ---------------------------------------------------------------------------
// Pipeline stage: inject_orphan_triggers
//
// When a function is dropped but a surviving table still carries a trigger
// referencing it, that trigger is not detected by the detection pass (the
// trigger node is identical in both states). We inject the DropTrigger here.
// ---------------------------------------------------------------------------

fn inject_orphan_triggers(ops: Vec<Operation>, previous: &Schema) -> Vec<Operation> {
    let dropped_fns: HashSet<&str> = ops.iter()
        .filter_map(|op| match op {
            Operation::DropFunction { function } => Some(function.name.as_str()),
            _ => None,
        })
        .collect();

    if dropped_fns.is_empty() {
        return ops;
    }

    let dropped_tables: HashSet<&str> = ops.iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        })
        .collect();

    let existing_drop_keys: HashSet<String> = ops.iter()
        .filter_map(|op| match op {
            Operation::DropTrigger { table_name, trigger } => {
                trigger.name.as_deref().map(|n| format!("{}:{}", table_name, n))
            }
            _ => None,
        })
        .collect();

    let mut injected: Vec<Operation> = Vec::new();
    for (table_name, table) in &previous.tables {
        if dropped_tables.contains(table_name.as_str()) {
            continue;
        }
        for trg in &table.triggers {
            let references_dropped = trg.function_name.as_deref()
                .map_or(false, |f| dropped_fns.contains(f));
            if !references_dropped {
                continue;
            }
            let key = trg.name.as_deref()
                .map(|n| format!("{}:{}", table_name, n))
                .unwrap_or_default();
            if !existing_drop_keys.contains(&key) {
                injected.push(Operation::DropTrigger {
                    table_name: table_name.clone(),
                    trigger: trg.clone(),
                });
            }
        }
    }

    let mut result = ops;
    result.extend(injected);
    result
}

// ---------------------------------------------------------------------------
// Pipeline stage: break_fk_cycles (create side)
//
// New tables that form FK cycles cannot all be created with their FKs inline.
// This pass strips cycle-forming FKs from the CreateTable payload and re-emits
// them as AddForeignKey after all tables exist. Self-referential FKs are always
// deferred — the table doesn't exist yet when CreateTable runs.
// ---------------------------------------------------------------------------

fn break_fk_cycles(ops: Vec<Operation>) -> Vec<Operation> {
    let new_table_names: HashSet<String> = ops.iter()
        .filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.name.clone()),
            _ => None,
        })
        .collect();

    if new_table_names.is_empty() {
        return ops;
    }

    // DFS to find cyclic FKs. Colors: 0=unvisited, 1=in-stack, 2=done.
    let tables_by_name: HashMap<&str, &Table> = ops.iter()
        .filter_map(|op| match op {
            Operation::CreateTable { table } => Some((table.name.as_str(), table)),
            _ => None,
        })
        .collect();

    let mut colors: HashMap<&str, u8> = HashMap::new();
    let mut cyclic: HashSet<(String, String)> = HashSet::new();
    let mut sorted_names: Vec<&str> = tables_by_name.keys().copied().collect();
    sorted_names.sort();

    for name in sorted_names {
        if colors.get(name).copied().unwrap_or(0) == 0 {
            if let Some(&table) = tables_by_name.get(name) {
                fk_cycle_visit(table, &tables_by_name, &new_table_names, &mut colors, &mut cyclic);
            }
        }
    }

    if cyclic.is_empty() {
        return ops;
    }

    let mut result: Vec<Operation> = Vec::with_capacity(ops.len());
    let mut deferred: Vec<Operation> = Vec::new();

    for op in ops {
        match op {
            Operation::CreateTable { mut table } => {
                let to_defer: Vec<ForeignKey> = table.foreign_keys.iter()
                    .filter(|fk| cyclic.contains(&(table.name.clone(), fk.name.clone())))
                    .cloned()
                    .collect();
                for fk in &to_defer {
                    deferred.push(Operation::AddForeignKey {
                        table_name: table.name.clone(),
                        foreign_key: fk.clone(),
                    });
                }
                if !to_defer.is_empty() {
                    let defer_names: HashSet<&str> = to_defer.iter().map(|fk| fk.name.as_str()).collect();
                    table.foreign_keys.retain(|fk| !defer_names.contains(fk.name.as_str()));
                }
                result.push(Operation::CreateTable { table });
            }
            other => result.push(other),
        }
    }

    result.extend(deferred);
    result
}

fn fk_cycle_visit<'a>(
    table: &'a Table,
    by_name: &HashMap<&'a str, &'a Table>,
    new_names: &HashSet<String>,
    colors: &mut HashMap<&'a str, u8>,
    cyclic: &mut HashSet<(String, String)>,
) {
    colors.insert(table.name.as_str(), 1);

    for fk in &table.foreign_keys {
        if fk.to_table == table.name {
            cyclic.insert((table.name.clone(), fk.name.clone()));
            continue;
        }
        if !new_names.contains(&fk.to_table) {
            continue;
        }
        match colors.get(fk.to_table.as_str()).copied().unwrap_or(0) {
            0 => {
                if let Some(&dep) = by_name.get(fk.to_table.as_str()) {
                    fk_cycle_visit(dep, by_name, new_names, colors, cyclic);
                }
            }
            1 => {
                cyclic.insert((table.name.clone(), fk.name.clone()));
            }
            _ => {}
        }
    }

    colors.insert(table.name.as_str(), 2);
}

// Pipeline stage: topo_sort_creates
//
// After break_fk_cycles, CreateTable ops have no cycles among their inline FKs.
// This pass re-orders them so every table appears after all tables it references
// via remaining (non-deferred) FKs. Without this, HashMap iteration order gives
// non-deterministic and often wrong CreateTable sequences.
fn topo_sort_creates(ops: Vec<Operation>) -> Vec<Operation> {
    let new_names: HashSet<String> = ops.iter()
        .filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.name.clone()),
            _ => None,
        })
        .collect();

    if new_names.is_empty() {
        return ops;
    }

    let creates: Vec<Table> = ops.iter()
        .filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.clone()),
            _ => None,
        })
        .collect();

    let by_name: HashMap<&str, &Table> = creates.iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    let mut colors: HashMap<&str, u8> = HashMap::new();
    let mut sorted: Vec<Table> = Vec::with_capacity(creates.len());
    let mut roots: Vec<&str> = by_name.keys().copied().collect();
    roots.sort();

    for name in roots {
        if colors.get(name).copied().unwrap_or(0) == 0 {
            if let Some(&table) = by_name.get(name) {
                create_topo_visit(table, &by_name, &new_names, &mut colors, &mut sorted);
            }
        }
    }

    let mut sorted_iter = sorted.into_iter();
    ops.into_iter()
        .map(|op| match op {
            Operation::CreateTable { .. } => Operation::CreateTable {
                table: sorted_iter.next().expect("topo sort count mismatch"),
            },
            other => other,
        })
        .collect()
}

fn create_topo_visit<'a>(
    table: &'a Table,
    by_name: &HashMap<&'a str, &'a Table>,
    new_names: &HashSet<String>,
    colors: &mut HashMap<&'a str, u8>,
    sorted: &mut Vec<Table>,
) {
    colors.insert(table.name.as_str(), 1);

    let mut deps: Vec<&str> = table.foreign_keys.iter()
        .filter(|fk| fk.to_table != table.name && new_names.contains(&fk.to_table))
        .map(|fk| fk.to_table.as_str())
        .collect();
    deps.sort();
    deps.dedup();

    for dep in deps {
        if colors.get(dep).copied().unwrap_or(0) == 0 {
            if let Some(&dep_table) = by_name.get(dep) {
                create_topo_visit(dep_table, by_name, new_names, colors, sorted);
            }
        }
    }

    colors.insert(table.name.as_str(), 2);
    sorted.push(table.clone());
}

// ---------------------------------------------------------------------------
// Pipeline stage: break_drop_cycles (drop side)
//
// Tables being dropped that form FK cycles cannot be dropped without first
// removing the cycle-forming FK. This pass injects DropForeignKey ops before
// the DropTable pair for any FK that forms a cycle within the drop set.
// Self-referential FKs are excluded — they vanish with the table.
// ---------------------------------------------------------------------------

fn break_drop_cycles(ops: Vec<Operation>) -> Vec<Operation> {
    let drop_set: HashSet<String> = ops.iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.clone()),
            _ => None,
        })
        .collect();

    if drop_set.is_empty() {
        return ops;
    }

    // Collect owned table data before consuming ops to avoid borrow/move conflict.
    let owned_tables: Vec<Table> = ops.iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.clone()),
            _ => None,
        })
        .collect();

    let tables_by_name: HashMap<&str, &Table> = owned_tables.iter()
        .map(|t| (t.name.as_str(), t))
        .collect();

    let mut colors: HashMap<&str, u8> = HashMap::new();
    let mut cyclic: HashSet<(String, String)> = HashSet::new();
    let mut order: Vec<&Table> = Vec::new();
    let mut sorted_names: Vec<&str> = tables_by_name.keys().copied().collect();
    sorted_names.sort();

    for name in sorted_names {
        if colors.get(name).copied().unwrap_or(0) == 0 {
            if let Some(&table) = tables_by_name.get(name) {
                drop_cycle_visit(table, &tables_by_name, &drop_set, &mut colors, &mut order, &mut cyclic);
            }
        }
    }

    // Rebuild: strip DropTable ops, replace with ordered cycle-safe sequence.
    let mut non_drop: Vec<Operation> = ops.into_iter()
        .filter(|op| !matches!(op, Operation::DropTable { .. }))
        .collect();

    let mut pre_fk_drops: Vec<Operation> = Vec::new();
    let mut ordered_drops: Vec<Operation> = Vec::new();

    for table in order.iter().rev() {
        for fk in &table.foreign_keys {
            if fk.to_table != table.name && cyclic.contains(&(table.name.clone(), fk.name.clone())) {
                pre_fk_drops.push(Operation::DropForeignKey {
                    table_name: table.name.clone(),
                    foreign_key: fk.clone(),
                    cascade: false,
                });
            }
        }
        ordered_drops.push(Operation::DropTable { table: (*table).clone() });
    }

    let mut result: Vec<Operation> = Vec::with_capacity(non_drop.len() + pre_fk_drops.len() + ordered_drops.len());

    // Preserve existing DropForeignKey ops that came from the detection pass
    // (surviving tables dropping FKs to dropped tables). Split non_drop by
    // whether they precede or follow the drop block.
    let pre_drop_fks: Vec<Operation> = non_drop.iter()
        .filter(|op| match op {
            Operation::DropForeignKey { foreign_key, .. } => drop_set.contains(&foreign_key.to_table),
            _ => false,
        })
        .cloned()
        .collect();
    non_drop.retain(|op| match op {
        Operation::DropForeignKey { foreign_key, .. } => !drop_set.contains(&foreign_key.to_table),
        _ => true,
    });

    result.extend(pre_drop_fks);
    result.extend(pre_fk_drops);
    result.extend(ordered_drops);
    result.extend(non_drop);
    result
}

fn drop_cycle_visit<'a>(
    table: &'a Table,
    by_name: &HashMap<&'a str, &'a Table>,
    drop_set: &HashSet<String>,
    colors: &mut HashMap<&'a str, u8>,
    order: &mut Vec<&'a Table>,
    cyclic: &mut HashSet<(String, String)>,
) {
    colors.insert(table.name.as_str(), 1);

    let mut deps: Vec<(&str, &str)> = table.foreign_keys.iter()
        .filter(|fk| drop_set.contains(&fk.to_table) && fk.to_table != table.name)
        .map(|fk| (fk.to_table.as_str(), fk.name.as_str()))
        .collect();
    deps.sort();

    for (dep_name, fk_name) in deps {
        match colors.get(dep_name).copied().unwrap_or(0) {
            0 => {
                if let Some(&dep) = by_name.get(dep_name) {
                    drop_cycle_visit(dep, by_name, drop_set, colors, order, cyclic);
                }
            }
            1 => {
                cyclic.insert((table.name.to_string(), fk_name.to_string()));
            }
            _ => {}
        }
    }

    colors.insert(table.name.as_str(), 2);
    order.push(table);
}

// ---------------------------------------------------------------------------
// Pipeline stage: sort_operations
//
// Assigns each operation a phase number and sorts stably within phases.
// All ordering rules that were previously encoded as hardcoded phase blocks
// are now expressed here as a pure function on the operation list.
//
// Phase order (lower runs first):
//   0  — CreateExtension
//   1  — CreateEnum
//   2  — DropView / ReplaceView (drop half)
//   3  — DropForeignKey (pre-drop FKs from surviving tables to dropped tables)
//   4  — DropTable (already ordered by break_drop_cycles)
//   5  — CreateFunction / AlterFunction
//   6  — CreateTable (already ordered by break_fk_cycles)
//   7  — AddForeignKey (deferred from break_fk_cycles)
//   8  — per-table changes: within a table, drop-sub-entities < add-sub-entities
//         Sub-entity drop order: Trigger < Constraint < Index < FK < Column
//         Sub-entity add order:  Column < FK < Index < Constraint < Trigger
//   9  — DropTrigger (orphan, injected by inject_orphan_triggers)
//  10  — DropFunction
//  11  — CreateView / ReplaceView (create half)
//  12  — DropEnum
//  13  — DropExtension
// ---------------------------------------------------------------------------

fn phase(op: &Operation) -> u8 {
    match op {
        Operation::CreateExtension { .. } => 0,
        Operation::CreateEnum { .. } | Operation::AlterEnum { .. } => 1,
        Operation::DropView { .. } => 2,
        Operation::DropForeignKey { .. } => 3,
        Operation::DropTable { .. } => 4,
        Operation::CreateFunction { .. } | Operation::AlterFunction { .. } => 5,
        Operation::CreateTable { .. } => 6,
        Operation::AddForeignKey { .. } => 7,
        // per-table sub-entity changes — phase 8, sub-sorted by sub_phase
        Operation::DropTrigger { .. } => 8,
        Operation::DropConstraint { .. } => 8,
        Operation::DropIndex { .. } => 8,
        Operation::DropColumn { .. } => 8,
        Operation::AddColumn { .. } => 8,
        Operation::AlterColumn { .. } => 8,
        Operation::AddIndex { .. } => 8,
        Operation::AddConstraint { .. } => 8,
        Operation::CreateTrigger { .. } => 8,
        Operation::AlterTrigger { .. } => 8,
        // orphan trigger drops injected by inject_orphan_triggers land here
        // (they have no table_name that matches a surviving table change)
        // They are already DropTrigger which maps to phase 8 above, but we
        // need them after per-table changes. The inject_orphan_triggers pass
        // appends them after all other ops, and sort_operations is stable,
        // so their relative position is preserved within phase 8.
        Operation::DropFunction { .. } => 10,
        Operation::CreateView { .. } | Operation::ReplaceView { .. } => 11,
        Operation::DropEnum { .. } => 12,
        Operation::DropExtension { .. } => 13,
        // These are not generated by the diff engine but must not panic.
        Operation::Statement { .. }
        | Operation::Invoke { .. }
        | Operation::RenameTable { .. }
        | Operation::RenameColumn { .. } => 8,
    }
}

// Within phase 8, further ordering by sub-entity type.
// Drops before adds; within drops: trigger < constraint < index < fk < column;
// within adds: column < fk < index < constraint < trigger.
fn sub_phase(op: &Operation) -> u8 {
    match op {
        Operation::DropTrigger { .. } => 0,
        Operation::DropConstraint { .. } => 1,
        Operation::DropIndex { .. } => 2,
        Operation::DropForeignKey { .. } => 3,
        Operation::DropColumn { .. } => 4,
        Operation::AlterColumn { .. } => 5,
        Operation::AddColumn { .. } => 6,
        Operation::AddForeignKey { .. } => 7,
        Operation::AddIndex { .. } => 8,
        Operation::AddConstraint { .. } => 9,
        Operation::CreateTrigger { .. } | Operation::AlterTrigger { .. } => 10,
        _ => 5,
    }
}

fn sort_operations(ops: Vec<Operation>) -> Vec<Operation> {
    let mut indexed: Vec<(usize, Operation)> = ops.into_iter().enumerate().collect();
    indexed.sort_by_key(|(i, op)| {
        let p = phase(op);
        let sp = if p == 8 { sub_phase(op) } else { 0 };
        (p, sp, *i)
    });
    indexed.into_iter().map(|(_, op)| op).collect()
}

// ---------------------------------------------------------------------------
// DiffEngine — public entry point
// ---------------------------------------------------------------------------

pub struct DiffEngine;

impl DiffEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn diff(&self, current: &Schema, previous: &Schema) -> Result<Vec<Operation>, DiffError> {
        let changes = detect(current, previous);
        let raw_ops: Vec<Operation> = changes.into_iter().flat_map(emit).collect();
        let after_orphans = inject_orphan_triggers(raw_ops, previous);
        let after_fk_cycles = break_fk_cycles(after_orphans);
        let after_topo = topo_sort_creates(after_fk_cycles);
        let after_drop_cycles = break_drop_cycles(after_topo);
        let sorted = sort_operations(after_drop_cycles);
        Ok(sorted)
    }
}

impl Default for DiffEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Hash impls required by flatten() for types that derive PartialEq but not Hash
// ---------------------------------------------------------------------------

use crate::states::{Volatility, TriggerTiming, TriggerScope, TriggerEvent};

impl Hash for Volatility {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl Hash for TriggerTiming {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl Hash for TriggerScope {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl Hash for TriggerEvent {
    fn hash<H: Hasher>(&self, state: &mut H) {
        std::mem::discriminant(self).hash(state);
    }
}

impl Hash for Constraint {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Constraint::Unique { name, columns } => {
                0u8.hash(state);
                name.hash(state);
                columns.hash(state);
            }
            Constraint::Check { name, expression } => {
                1u8.hash(state);
                name.hash(state);
                expression.hash(state);
            }
        }
    }
}
