use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::operations::Operation;
use crate::states::{Column, Constraint, EnumDef, ForeignKey, Index, Schema, Table, TriggerDef};

#[derive(Debug, Error)]
pub enum DiffError {}

/// Compares two schema states and produces an ordered list of operations
/// to transform `previous` into `current`.
///
/// Safe execution order for PostgreSQL:
///   1. DropView — before drops/changes so dependent views don't block table ops
///   2. DropTable — reverse-sorted for determinism
///   3. CreateFunction / AlterFunction — before tables that reference them in triggers
///   4. CreateTable — FK-topo sorted; mutually-referencing pairs have one FK deferred
///   5. AddForeignKey — deferred FKs from cycles, after all participants are created
///   6. Per-table changes in sorted order (drops before adds within each table)
///   7. DropFunction — after trigger drops that already ran in per-table changes
///   8. CreateView — last; depends on tables and functions
///
/// RenameTable / RenameColumn are Phase 2 — this diff emits Drop+Create pairs instead.
pub struct DiffEngine;

impl DiffEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn diff(&self, current: &Schema, previous: &Schema) -> Result<Vec<Operation>, DiffError> {
        let mut drops: Vec<&Table> = Vec::new();
        let mut new_tables: Vec<&Table> = Vec::new();
        let mut changes: Vec<Operation> = Vec::new();

        // Single merge-walk over two already-sorted BTreeMaps: O(n) total.
        // Three separate filter passes with contains_key would be O(n log n).
        let mut prev_iter = previous.tables.iter().peekable();
        let mut curr_iter = current.tables.iter().peekable();

        loop {
            match (prev_iter.peek(), curr_iter.peek()) {
                (None, None) => break,
                (Some(_), None) => {
                    drops.push(prev_iter.next().unwrap().1);
                }
                (None, Some(_)) => {
                    new_tables.push(curr_iter.next().unwrap().1);
                }
                (Some((pk, _)), Some((ck, _))) => match pk.as_str().cmp(ck.as_str()) {
                    std::cmp::Ordering::Less => {
                        drops.push(prev_iter.next().unwrap().1);
                    }
                    std::cmp::Ordering::Greater => {
                        new_tables.push(curr_iter.next().unwrap().1);
                    }
                    std::cmp::Ordering::Equal => {
                        let (name, pt) = prev_iter.next().unwrap();
                        let (_, ct) = curr_iter.next().unwrap();
                        diff_table(name, pt, ct, &mut changes);
                    }
                },
            }
        }

        let new_names: HashSet<&str> = new_tables.iter().map(|t| t.name.as_str()).collect();
        let (creates, deferred_fk_adds) = fk_topo_sort(new_tables, &new_names);
        let (fn_creates, fn_drops) = diff_functions(current, previous);
        let (view_creates, view_drops) = diff_views(current, previous);
        let (ext_creates, ext_drops) = diff_extensions(current, previous);
        let (enum_creates, enum_drops) = diff_enums(current, previous);

        // When a function is dropped, any surviving trigger that references it becomes
        // invalid. The trigger itself may be "unchanged" from the diff's perspective
        // (same name, same body), so diff_table won't generate a DropTrigger for it.
        // We detect these orphaned triggers here and prepend their DropTrigger ops before
        // fn_drops so they are removed before the function disappears.
        let dropped_fns: HashSet<&str> = fn_drops.iter()
            .filter_map(|op| match op {
                Operation::DropFunction { function } => Some(function.name.as_str()),
                _ => None,
            })
            .collect();
        let dropped_table_names: HashSet<&str> = drops.iter().map(|t| t.name.as_str()).collect();
        let existing_drop_trigger_keys: HashSet<String> = changes.iter()
            .filter_map(|op| match op {
                Operation::DropTrigger { table_name, trigger } => {
                    trigger.name.as_deref().map(|n| format!("{}:{}", table_name, n))
                }
                _ => None,
            })
            .collect();
        let mut orphan_trigger_drops: Vec<Operation> = Vec::new();
        if !dropped_fns.is_empty() {
            for (table_name, table) in &previous.tables {
                if dropped_table_names.contains(table_name.as_str()) {
                    continue; // table is being dropped; its triggers vanish with it
                }
                for trigger in &table.triggers {
                    if trigger.function_name.as_deref().map_or(false, |f| dropped_fns.contains(f)) {
                        let key = trigger.name.as_deref()
                            .map(|n| format!("{}:{}", table_name, n))
                            .unwrap_or_default();
                        if !existing_drop_trigger_keys.contains(&key) {
                            orphan_trigger_drops.push(Operation::DropTrigger {
                                table_name: table_name.clone(),
                                trigger: trigger.clone(),
                            });
                        }
                    }
                }
            }
        }

        // Partition per-table changes: FK drops targeting tables being dropped must
        // run before those DropTable ops; everything else stays in normal position.
        let mut pre_drop_fk_ops: Vec<Operation> = Vec::new();
        let mut remaining_changes: Vec<Operation> = Vec::new();
        for op in changes {
            match &op {
                Operation::DropForeignKey { foreign_key, .. }
                    if dropped_table_names.contains(foreign_key.to_table.as_str()) =>
                {
                    pre_drop_fk_ops.push(op);
                }
                _ => remaining_changes.push(op),
            }
        }

        let mut result = Vec::with_capacity(drops.len() + creates.len() + remaining_changes.len());
        result.extend(ext_creates);
        result.extend(enum_creates);
        result.extend(view_drops);
        result.extend(pre_drop_fk_ops);
        result.extend(drop_ordered(drops));
        result.extend(fn_creates);
        result.extend(creates);
        result.extend(deferred_fk_adds);
        result.extend(remaining_changes);
        result.extend(orphan_trigger_drops);
        result.extend(fn_drops);
        result.extend(view_creates);
        result.extend(enum_drops);
        result.extend(ext_drops);
        Ok(result)
    }
}

fn diff_table(name: &str, prev: &Table, curr: &Table, ops: &mut Vec<Operation>) {
    // Fast path: skip all HashMap allocation if table is structurally identical.
    // In practice the majority of tables are unchanged per migration.
    if prev == curr {
        return;
    }

    // Build name-keyed maps once for O(1) lookups.
    // Without these, each iter().any() scan is O(n×m) across all entities.
    // curr_cols is not needed: drop-side uses curr_cols, add-side uses prev_cols;
    // we iterate curr.columns directly for the add/alter pass.
    let prev_cols: HashMap<&str, &Column> = prev.columns.iter().map(|c| (c.name.as_str(), c)).collect();
    let curr_cols: HashMap<&str, &Column> = curr.columns.iter().map(|c| (c.name.as_str(), c)).collect();
    let prev_fks: HashMap<&str, &ForeignKey> = prev.foreign_keys.iter().map(|fk| (fk.name.as_str(), fk)).collect();
    let curr_fks: HashMap<&str, &ForeignKey> = curr.foreign_keys.iter().map(|fk| (fk.name.as_str(), fk)).collect();
    let prev_idxs: HashMap<&str, &Index> = prev.indexes.iter().map(|i| (i.name.as_str(), i)).collect();
    let curr_idxs: HashMap<&str, &Index> = curr.indexes.iter().map(|i| (i.name.as_str(), i)).collect();
    let prev_cons: HashMap<&str, &Constraint> = prev.constraints.iter().map(|c| (c.name(), c)).collect();
    let curr_cons: HashMap<&str, &Constraint> = curr.constraints.iter().map(|c| (c.name(), c)).collect();

    let prev_trgs: HashMap<&str, &TriggerDef> = prev.triggers.iter()
        .filter_map(|t| t.name.as_deref().map(|n| (n, t))).collect();
    let curr_trgs: HashMap<&str, &TriggerDef> = curr.triggers.iter()
        .filter_map(|t| t.name.as_deref().map(|n| (n, t))).collect();

    // --- removals: triggers first (may reference columns/constraints via WHEN),
    //     then constraints, indexes, FKs, columns ---

    for t in &prev.triggers {
        let tname = t.name.as_deref().unwrap_or("");
        if !curr_trgs.contains_key(tname) {
            ops.push(Operation::DropTrigger { table_name: name.to_string(), trigger: t.clone() });
        }
    }
    for c in &prev.constraints {
        if curr_cons.get(c.name()) != Some(&c) {
            ops.push(Operation::DropConstraint { table_name: name.to_string(), constraint: c.clone() });
        }
    }
    for i in &prev.indexes {
        if curr_idxs.get(i.name.as_str()) != Some(&i) {
            ops.push(Operation::DropIndex { table_name: name.to_string(), index: i.clone(), concurrent: false });
        }
    }
    for fk in &prev.foreign_keys {
        if curr_fks.get(fk.name.as_str()) != Some(&fk) {
            ops.push(Operation::DropForeignKey { table_name: name.to_string(), foreign_key: fk.clone(), cascade: false });
        }
    }
    for col in &prev.columns {
        if !curr_cols.contains_key(col.name.as_str()) {
            ops.push(Operation::DropColumn { table_name: name.to_string(), column: col.clone(), cascade: false });
        }
    }

    // --- additions: columns, FKs, indexes, constraints, triggers ---

    for col in &curr.columns {
        match prev_cols.get(col.name.as_str()) {
            None => ops.push(Operation::AddColumn { table_name: name.to_string(), column: col.clone() }),
            Some(old) if *old != col => ops.push(Operation::AlterColumn {
                table_name: name.to_string(),
                old: (*old).clone(),
                new: col.clone(),
                cast_expr: None,
            }),
            _ => {}
        }
    }
    for fk in &curr.foreign_keys {
        if prev_fks.get(fk.name.as_str()) != Some(&fk) {
            ops.push(Operation::AddForeignKey { table_name: name.to_string(), foreign_key: fk.clone() });
        }
    }
    for i in &curr.indexes {
        if prev_idxs.get(i.name.as_str()) != Some(&i) {
            ops.push(Operation::AddIndex { table_name: name.to_string(), index: i.clone(), concurrent: false });
        }
    }
    for c in &curr.constraints {
        if prev_cons.get(c.name()) != Some(&c) {
            ops.push(Operation::AddConstraint { table_name: name.to_string(), constraint: c.clone() });
        }
    }
    for t in &curr.triggers {
        let tname = t.name.as_deref().unwrap_or("");
        match prev_trgs.get(tname) {
            None => ops.push(Operation::CreateTrigger { table_name: name.to_string(), trigger: t.clone() }),
            Some(old) if *old != t => ops.push(Operation::AlterTrigger {
                table_name: name.to_string(),
                old: (*old).clone(),
                new: t.clone(),
            }),
            _ => {}
        }
    }
}

fn diff_functions(current: &Schema, previous: &Schema) -> (Vec<Operation>, Vec<Operation>) {
    let mut creates: Vec<Operation> = Vec::new();
    let mut drops: Vec<Operation> = Vec::new();
    let mut prev_iter = previous.functions.iter().peekable();
    let mut curr_iter = current.functions.iter().peekable();
    loop {
        match (prev_iter.peek(), curr_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (_, f) = prev_iter.next().unwrap();
                drops.push(Operation::DropFunction { function: f.clone() });
            }
            (None, Some(_)) => {
                let (_, f) = curr_iter.next().unwrap();
                creates.push(Operation::CreateFunction { function: f.clone() });
            }
            (Some((pk, _)), Some((ck, _))) => match pk.as_str().cmp(ck.as_str()) {
                std::cmp::Ordering::Less => {
                    let (_, f) = prev_iter.next().unwrap();
                    drops.push(Operation::DropFunction { function: f.clone() });
                }
                std::cmp::Ordering::Greater => {
                    let (_, f) = curr_iter.next().unwrap();
                    creates.push(Operation::CreateFunction { function: f.clone() });
                }
                std::cmp::Ordering::Equal => {
                    let (_, pf) = prev_iter.next().unwrap();
                    let (_, cf) = curr_iter.next().unwrap();
                    if pf != cf {
                        creates.push(Operation::AlterFunction { old: pf.clone(), new: cf.clone() });
                    }
                }
            },
        }
    }
    (creates, drops)
}

impl Default for DiffEngine {
    fn default() -> Self {
        Self::new()
    }
}

fn diff_extensions(current: &Schema, previous: &Schema) -> (Vec<Operation>, Vec<Operation>) {
    let mut creates: Vec<Operation> = Vec::new();
    let mut drops: Vec<Operation> = Vec::new();
    let mut prev_iter = previous.extensions.iter().peekable();
    let mut curr_iter = current.extensions.iter().peekable();
    loop {
        match (prev_iter.peek(), curr_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (_, e) = prev_iter.next().unwrap();
                drops.push(Operation::DropExtension { extension: e.clone() });
            }
            (None, Some(_)) => {
                let (_, e) = curr_iter.next().unwrap();
                creates.push(Operation::CreateExtension { extension: e.clone() });
            }
            (Some((pk, _)), Some((ck, _))) => match pk.as_str().cmp(ck.as_str()) {
                std::cmp::Ordering::Less => {
                    let (_, e) = prev_iter.next().unwrap();
                    drops.push(Operation::DropExtension { extension: e.clone() });
                }
                std::cmp::Ordering::Greater => {
                    let (_, e) = curr_iter.next().unwrap();
                    creates.push(Operation::CreateExtension { extension: e.clone() });
                }
                std::cmp::Ordering::Equal => {
                    let (_, pe) = prev_iter.next().unwrap();
                    let (_, ce) = curr_iter.next().unwrap();
                    if pe != ce {
                        drops.push(Operation::DropExtension { extension: pe.clone() });
                        creates.push(Operation::CreateExtension { extension: ce.clone() });
                    }
                }
            },
        }
    }
    (creates, drops)
}

// Returns (creates+alters, drops). AlterEnum is emitted when values are only appended;
// a removal or reordering is treated as DropEnum + CreateEnum because PostgreSQL has no
// DROP VALUE and the diff engine must remain deterministic.
fn diff_enums(current: &Schema, previous: &Schema) -> (Vec<Operation>, Vec<Operation>) {
    let mut creates: Vec<Operation> = Vec::new();
    let mut drops: Vec<Operation> = Vec::new();
    let mut prev_iter = previous.enums.iter().peekable();
    let mut curr_iter = current.enums.iter().peekable();
    loop {
        match (prev_iter.peek(), curr_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (_, e) = prev_iter.next().unwrap();
                drops.push(Operation::DropEnum { enum_def: e.clone() });
            }
            (None, Some(_)) => {
                let (_, e) = curr_iter.next().unwrap();
                creates.push(Operation::CreateEnum { enum_def: e.clone() });
            }
            (Some((pk, _)), Some((ck, _))) => match pk.as_str().cmp(ck.as_str()) {
                std::cmp::Ordering::Less => {
                    let (_, e) = prev_iter.next().unwrap();
                    drops.push(Operation::DropEnum { enum_def: e.clone() });
                }
                std::cmp::Ordering::Greater => {
                    let (_, e) = curr_iter.next().unwrap();
                    creates.push(Operation::CreateEnum { enum_def: e.clone() });
                }
                std::cmp::Ordering::Equal => {
                    let (_, pe) = prev_iter.next().unwrap();
                    let (_, ce) = curr_iter.next().unwrap();
                    if pe != ce {
                        if is_append_only(pe, ce) {
                            creates.push(Operation::AlterEnum { old: pe.clone(), new: ce.clone() });
                        } else {
                            drops.push(Operation::DropEnum { enum_def: pe.clone() });
                            creates.push(Operation::CreateEnum { enum_def: ce.clone() });
                        }
                    }
                }
            },
        }
    }
    (creates, drops)
}

// Returns true when new's values contain all of old's values in the same relative order
// with zero removals — i.e. old is a subsequence of new.
fn is_append_only(old: &EnumDef, new: &EnumDef) -> bool {
    let mut old_iter = old.values.iter().peekable();
    for val in &new.values {
        if old_iter.peek() == Some(&val) {
            old_iter.next();
        }
    }
    old_iter.next().is_none()
}

fn diff_views(current: &Schema, previous: &Schema) -> (Vec<Operation>, Vec<Operation>) {
    let mut creates: Vec<Operation> = Vec::new();
    let mut drops: Vec<Operation> = Vec::new();
    let mut prev_iter = previous.views.iter().peekable();
    let mut curr_iter = current.views.iter().peekable();
    loop {
        match (prev_iter.peek(), curr_iter.peek()) {
            (None, None) => break,
            (Some(_), None) => {
                let (_, v) = prev_iter.next().unwrap();
                drops.push(Operation::DropView { view: v.clone() });
            }
            (None, Some(_)) => {
                let (_, v) = curr_iter.next().unwrap();
                creates.push(Operation::CreateView { view: v.clone() });
            }
            (Some((pk, _)), Some((ck, _))) => match pk.as_str().cmp(ck.as_str()) {
                std::cmp::Ordering::Less => {
                    let (_, v) = prev_iter.next().unwrap();
                    drops.push(Operation::DropView { view: v.clone() });
                }
                std::cmp::Ordering::Greater => {
                    let (_, v) = curr_iter.next().unwrap();
                    creates.push(Operation::CreateView { view: v.clone() });
                }
                std::cmp::Ordering::Equal => {
                    let (_, pv) = prev_iter.next().unwrap();
                    let (_, cv) = curr_iter.next().unwrap();
                    if pv != cv {
                        creates.push(Operation::ReplaceView { old: pv.clone(), new: cv.clone() });
                    }
                }
            },
        }
    }
    (creates, drops)
}

// Returns (ordered CreateTable ops, deferred AddForeignKey ops for cyclic pairs).
// Uses grey/black DFS coloring: a back edge to a grey node means a cycle — the FK
// causing it is stripped from the inline CreateTable and emitted as AddForeignKey after
// all tables in the cycle exist.
// Returns DropTable operations in safe order: tables that are FK-referenced by other
// dropped tables come last. Cycles within the drop set are broken by emitting a leading
// DropForeignKey for the cycle-forming constraint before either DropTable.
// Self-referential FKs don't block drops (the row and constraint vanish together).
fn drop_ordered(tables: Vec<&Table>) -> Vec<Operation> {
    let drop_set: HashSet<&str> = tables.iter().map(|t| t.name.as_str()).collect();
    let by_name: HashMap<&str, &Table> = tables.iter().map(|t| (t.name.as_str(), *t)).collect();
    let mut colors: HashMap<&str, u8> = HashMap::new();
    let mut order: Vec<&Table> = Vec::new();
    let mut cyclic: HashSet<(String, String)> = HashSet::new();
    let mut sorted = tables;
    sorted.sort_by_key(|t| t.name.as_str());
    for table in sorted {
        if colors.get(table.name.as_str()).copied().unwrap_or(0) == 0 {
            fk_topo_visit(table, &by_name, &drop_set, &mut colors, &mut order, &mut cyclic);
        }
    }
    // order is create-order (dependencies first); reverse gives safe drop order.
    // For cyclic pairs, emit DropForeignKey for the cycle-forming constraint first so
    // either table can be dropped without a dependency block.
    let mut pre: Vec<Operation> = Vec::new();
    let mut drops: Vec<Operation> = Vec::new();
    for table in order.iter().rev() {
        for fk in &table.foreign_keys {
            // Self-referential FKs don't block drops — the row and FK vanish together.
            if fk.to_table != table.name && cyclic.contains(&(table.name.clone(), fk.name.clone())) {
                pre.push(Operation::DropForeignKey {
                    table_name: table.name.clone(),
                    foreign_key: fk.clone(),
                    cascade: false,
                });
            }
        }
        drops.push(Operation::DropTable { table: (*table).clone() });
    }
    pre.extend(drops);
    pre
}

fn fk_topo_sort<'a>(tables: Vec<&'a Table>, new_names: &HashSet<&str>) -> (Vec<Operation>, Vec<Operation>) {
    let by_name: HashMap<&'a str, &'a Table> = tables.iter().map(|t| (t.name.as_str(), *t)).collect();
    let mut colors: HashMap<&'a str, u8> = HashMap::new(); // 0=unvisited, 1=in-stack, 2=done
    let mut order: Vec<&'a Table> = Vec::new();
    let mut cyclic: HashSet<(String, String)> = HashSet::new(); // (table_name, fk_name)
    let mut sorted = tables;
    sorted.sort_by_key(|t| t.name.as_str());
    for table in sorted {
        if colors.get(table.name.as_str()).copied().unwrap_or(0) == 0 {
            fk_topo_visit(table, &by_name, new_names, &mut colors, &mut order, &mut cyclic);
        }
    }
    let mut creates: Vec<Operation> = Vec::with_capacity(order.len());
    let mut deferred: Vec<Operation> = Vec::new();
    for table in order {
        let has_cyclic = cyclic.iter().any(|(t, _)| t == &table.name);
        if has_cyclic {
            let mut t = table.clone();
            t.foreign_keys.retain(|fk| !cyclic.contains(&(table.name.clone(), fk.name.clone())));
            creates.push(Operation::CreateTable { table: t });
        } else {
            creates.push(Operation::CreateTable { table: table.clone() });
        }
        for fk in &table.foreign_keys {
            if cyclic.contains(&(table.name.clone(), fk.name.clone())) {
                deferred.push(Operation::AddForeignKey {
                    table_name: table.name.clone(),
                    foreign_key: fk.clone(),
                });
            }
        }
    }
    (creates, deferred)
}

fn fk_topo_visit<'a>(
    table: &'a Table,
    by_name: &HashMap<&'a str, &'a Table>,
    new_names: &HashSet<&str>,
    colors: &mut HashMap<&'a str, u8>,
    result: &mut Vec<&'a Table>,
    cyclic: &mut HashSet<(String, String)>,
) {
    colors.insert(table.name.as_str(), 1);
    // Self-referential FKs must be deferred: the table doesn't exist in state yet
    // when CreateTable is validated, so they'd always fail the reference check.
    for fk in &table.foreign_keys {
        if fk.to_table == table.name {
            cyclic.insert((table.name.to_string(), fk.name.to_string()));
        }
    }
    let mut deps: Vec<(&str, &str)> = table.foreign_keys.iter()
        .filter(|fk| new_names.contains(fk.to_table.as_str()) && fk.to_table != table.name)
        .map(|fk| (fk.to_table.as_str(), fk.name.as_str()))
        .collect();
    deps.sort();
    for (dep_name, fk_name) in deps {
        match colors.get(dep_name).copied().unwrap_or(0) {
            0 => {
                if let Some(&dep) = by_name.get(dep_name) {
                    fk_topo_visit(dep, by_name, new_names, colors, result, cyclic);
                }
            }
            1 => {
                // back edge — cycle; defer this FK to after all tables are created
                cyclic.insert((table.name.to_string(), fk_name.to_string()));
            }
            _ => {}
        }
    }
    colors.insert(table.name.as_str(), 2);
    result.push(table);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{Column, Constraint, ForeignKey, FunctionDef, Index, Table, TriggerDef, TriggerEvent, TriggerScope, TriggerTiming, ViewDef, Volatility};

    fn engine() -> DiffEngine {
        DiffEngine::new()
    }

    fn empty_table(name: &str) -> Table {
        Table { name: name.to_string(), schema: None, columns: vec![], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] }
    }

    fn basic_function(name: &str) -> FunctionDef {
        FunctionDef {
            name: name.to_string(),
            schema: None,
            arguments: String::new(),
            returns: "void".to_string(),
            language: "sql".to_string(),
            body: "SELECT 1".to_string(),
            volatility: Volatility::Volatile,
            security_definer: false,
        }
    }

    fn basic_trigger(name: &str) -> TriggerDef {
        TriggerDef {
            name: Some(name.to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("some_fn".to_string()),
            when: None,
            body: None,
            language: None,
        }
    }

    fn text_col(name: &str) -> Column {
        Column { name: name.to_string(), col_type: "text".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() }
    }

    fn state_with_table(t: Table) -> Schema {
        let mut s = Schema::default();
        s.tables.insert(t.name.clone(), t);
        s
    }

    /// No differences between identical states produces no operations.
    #[test]
    fn identical_states_produce_no_ops() {
        let s = state_with_table(empty_table("users"));
        let ops = engine().diff(&s, &s).unwrap();
        assert!(ops.is_empty());
    }

    /// A table present in current but not previous generates CreateTable.
    #[test]
    fn new_table_generates_create() {
        let prev = Schema::default();
        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "users"));
    }

    /// A table present in previous but not current generates DropTable.
    #[test]
    fn removed_table_generates_drop() {
        let prev = state_with_table(empty_table("users"));
        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropTable { table } if table.name == "users"));
    }

    /// Multiple new tables are emitted in sorted order.
    #[test]
    fn multiple_new_tables_sorted() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        curr.tables.insert("zebra".to_string(), empty_table("zebra"));
        curr.tables.insert("apple".to_string(), empty_table("apple"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "apple"));
        assert!(matches!(&ops[1], Operation::CreateTable { table } if table.name == "zebra"));
    }

    /// Multiple dropped tables are emitted in reverse-sorted order when no FK deps exist.
    #[test]
    fn multiple_dropped_tables_reverse_sorted() {
        let mut prev = Schema::default();
        prev.tables.insert("zebra".to_string(), empty_table("zebra"));
        prev.tables.insert("apple".to_string(), empty_table("apple"));
        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropTable { table } if table.name == "zebra"));
        assert!(matches!(&ops[1], Operation::DropTable { table } if table.name == "apple"));
    }

    /// When dropping tables with FK dependencies between them, the referencing table
    /// (the one with the FK) is dropped before the referenced table.
    #[test]
    fn dropped_tables_fk_dep_order() {
        let products = empty_table("products");
        let mut inventory = empty_table("inventory");
        inventory.foreign_keys.push(ForeignKey {
            name: "inventory_product_id_fkey".to_string(),
            from_column: "product_id".to_string(),
            to_table: "products".to_string(),
            to_column: "id".to_string(),
        });
        let mut prev = Schema::default();
        prev.tables.insert("products".to_string(), products);
        prev.tables.insert("inventory".to_string(), inventory);
        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        // inventory (referencing table) must be dropped before products (referenced)
        let drop_names: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(drop_names, vec!["inventory", "products"]);
    }

    /// A new column on an existing table generates AddColumn.
    #[test]
    fn added_column_generates_add() {
        let prev = state_with_table(empty_table("users"));
        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("email"));
        let curr = state_with_table(curr_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AddColumn { table_name, column } if table_name == "users" && column.name == "email"));
    }

    /// A removed column generates DropColumn.
    #[test]
    fn removed_column_generates_drop() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("email"));
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropColumn { table_name, column, .. } if table_name == "users" && column.name == "email"));
    }

    /// A column whose type changes generates AlterColumn.
    #[test]
    fn altered_column_type_generates_alter() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("bio"));
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.columns.push(Column { name: "bio".to_string(), col_type: "varchar(500)".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AlterColumn { table_name, .. } if table_name == "users"));
    }

    /// A column whose nullable flag changes generates AlterColumn.
    #[test]
    fn altered_column_nullable_generates_alter() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(Column { name: "bio".to_string(), col_type: "text".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() });
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.columns.push(Column { name: "bio".to_string(), col_type: "text".to_string(), nullable: true, default: None, primary_key: false, ..Default::default() });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AlterColumn { .. }));
    }

    /// A column whose default changes generates AlterColumn.
    #[test]
    fn altered_column_default_generates_alter() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(Column { name: "score".to_string(), col_type: "int".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() });
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.columns.push(Column { name: "score".to_string(), col_type: "int".to_string(), nullable: false, default: Some("0".to_string()), primary_key: false, ..Default::default() });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AlterColumn { .. }));
    }

    /// An unchanged column produces no operations.
    #[test]
    fn unchanged_column_produces_no_op() {
        let mut table = empty_table("users");
        table.columns.push(text_col("email"));
        let s = state_with_table(table);
        let ops = engine().diff(&s, &s).unwrap();
        assert!(ops.is_empty());
    }

    /// A new foreign key generates AddForeignKey.
    #[test]
    fn added_fk_generates_add() {
        let prev = state_with_table(empty_table("posts"));
        let mut curr_table = empty_table("posts");
        curr_table.foreign_keys.push(ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() });
        let curr = state_with_table(curr_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AddForeignKey { foreign_key, .. } if foreign_key.name == "fk_user"));
    }

    /// A removed foreign key generates DropForeignKey.
    #[test]
    fn removed_fk_generates_drop() {
        let mut prev_table = empty_table("posts");
        prev_table.foreign_keys.push(ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() });
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("posts"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropForeignKey { foreign_key, .. } if foreign_key.name == "fk_user"));
    }

    /// A new index generates AddIndex.
    #[test]
    fn added_index_generates_add() {
        let prev = state_with_table(empty_table("users"));
        let mut curr_table = empty_table("users");
        curr_table.indexes.push(Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None });
        let curr = state_with_table(curr_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AddIndex { index, .. } if index.name == "idx_email"));
    }

    /// A removed index generates DropIndex.
    #[test]
    fn removed_index_generates_drop() {
        let mut prev_table = empty_table("users");
        prev_table.indexes.push(Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None });
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropIndex { index, .. } if index.name == "idx_email"));
    }

    /// A new constraint generates AddConstraint.
    #[test]
    fn added_constraint_generates_add() {
        let prev = state_with_table(empty_table("users"));
        let mut curr_table = empty_table("users");
        curr_table.constraints.push(Constraint::Check { name: "chk_age".to_string(), expression: "age > 0".to_string() });
        let curr = state_with_table(curr_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AddConstraint { constraint, .. } if constraint.name() == "chk_age"));
    }

    /// A removed constraint generates DropConstraint.
    #[test]
    fn removed_constraint_generates_drop() {
        let mut prev_table = empty_table("users");
        prev_table.constraints.push(Constraint::Unique { name: "uq_email".to_string(), columns: vec!["email".to_string()] });
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropConstraint { constraint, .. } if constraint.name() == "uq_email"));
    }

    /// Removals (Drop*) are emitted before additions (Add*) within the same table.
    #[test]
    fn removals_before_additions_within_table() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("old_col"));
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("new_col"));
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropColumn { .. }), "drop should come first");
        assert!(matches!(&ops[1], Operation::AddColumn { .. }), "add should come second");
    }

    /// Drops come before Creates at the table level.
    #[test]
    fn table_drops_before_creates() {
        let mut prev = Schema::default();
        prev.tables.insert("old".to_string(), empty_table("old"));
        let mut curr = Schema::default();
        curr.tables.insert("new".to_string(), empty_table("new"));

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropTable { .. }), "drop should come first");
        assert!(matches!(&ops[1], Operation::CreateTable { .. }), "create should come second");
    }

    /// The diff result, when applied to previous state, produces a state equal to current.
    #[test]
    fn diff_is_applicable_end_to_end() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("name"));
        prev_table.columns.push(text_col("old_field"));
        let mut prev = Schema::default();
        prev.tables.insert("users".to_string(), prev_table);
        prev.tables.insert("to_drop".to_string(), empty_table("to_drop"));

        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("name"));
        curr_table.columns.push(Column { name: "old_field".to_string(), col_type: "int".to_string(), nullable: true, default: None, primary_key: false, ..Default::default() });
        curr_table.columns.push(text_col("new_field"));
        let mut curr = Schema::default();
        curr.tables.insert("users".to_string(), curr_table);
        curr.tables.insert("to_create".to_string(), empty_table("to_create"));

        let ops = engine().diff(&curr, &prev).unwrap();

        let mut replayed = prev.clone();
        for op in &ops {
            replayed.apply(op).expect("op should apply cleanly");
        }

        assert_eq!(replayed.tables.keys().collect::<Vec<_>>(), curr.tables.keys().collect::<Vec<_>>());
        let replayed_users = &replayed.tables["users"];
        let curr_users = &curr.tables["users"];
        assert_eq!(replayed_users.columns.len(), curr_users.columns.len());
        for (r, c) in replayed_users.columns.iter().zip(curr_users.columns.iter()) {
            assert_eq!(r.name, c.name);
            assert_eq!(r.col_type, c.col_type);
            assert_eq!(r.nullable, c.nullable);
        }
    }

    /// When two new tables have a FK relationship, the referenced table is emitted first.
    #[test]
    fn fk_ordered_creates() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let users = empty_table("users");
        let mut posts = empty_table("posts");
        posts.foreign_keys.push(ForeignKey {
            name: "fk_posts_user".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("users".to_string(), users);
        curr.tables.insert("posts".to_string(), posts);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "users"),
            "users (referenced) must come before posts (referencing)");
        assert!(matches!(&ops[1], Operation::CreateTable { table } if table.name == "posts"));
    }

    /// A new table with a FK to an already-existing table has no ordering constraint forced.
    #[test]
    fn fk_to_existing_table_does_not_affect_order() {
        let mut prev = Schema::default();
        prev.tables.insert("users".to_string(), empty_table("users"));
        let mut curr = prev.clone();
        let mut posts = empty_table("posts");
        posts.foreign_keys.push(ForeignKey {
            name: "fk_posts_user".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("posts".to_string(), posts);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "posts"));
    }

    /// Changing an FK's target table emits DropForeignKey then AddForeignKey.
    #[test]
    fn modified_fk_generates_drop_and_add() {
        let mut prev_table = empty_table("orders");
        prev_table.foreign_keys.push(ForeignKey {
            name: "fk_user".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("orders");
        curr_table.foreign_keys.push(ForeignKey {
            name: "fk_user".to_string(),
            from_column: "user_id".to_string(),
            to_table: "accounts".to_string(),
            to_column: "id".to_string(),
        });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropForeignKey { foreign_key, .. } if foreign_key.name == "fk_user"));
        assert!(matches!(&ops[1], Operation::AddForeignKey { foreign_key, .. } if foreign_key.to_table == "accounts"));
    }

    /// Changing an index's unique flag emits DropIndex then AddIndex.
    #[test]
    fn modified_index_generates_drop_and_add() {
        let mut prev_table = empty_table("users");
        prev_table.indexes.push(Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: false, predicate: None });
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.indexes.push(Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropIndex { index, .. } if index.name == "idx_email"));
        assert!(matches!(&ops[1], Operation::AddIndex { index, .. } if index.unique));
    }

    /// Changing a check constraint's expression emits DropConstraint then AddConstraint.
    #[test]
    fn modified_constraint_generates_drop_and_add() {
        let mut prev_table = empty_table("users");
        prev_table.constraints.push(Constraint::Check { name: "chk_age".to_string(), expression: "age > 0".to_string() });
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.constraints.push(Constraint::Check { name: "chk_age".to_string(), expression: "age >= 18".to_string() });
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropConstraint { constraint, .. } if constraint.name() == "chk_age"));
        assert!(matches!(&ops[1], Operation::AddConstraint { constraint, .. }
            if matches!(constraint, Constraint::Check { expression, .. } if expression == "age >= 18")));
    }

    /// A function present in current but not previous generates CreateFunction.
    #[test]
    fn new_function_generates_create() {
        let mut curr = Schema::default();
        curr.functions.insert("notify".to_string(), basic_function("notify"));
        let prev = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateFunction { function } if function.name == "notify"));
    }

    /// A function present in previous but not current generates DropFunction.
    #[test]
    fn removed_function_generates_drop() {
        let curr = Schema::default();
        let mut prev = Schema::default();
        prev.functions.insert("notify".to_string(), basic_function("notify"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropFunction { function } if function.name == "notify"));
    }

    /// A changed function body generates AlterFunction with correct old and new.
    #[test]
    fn modified_function_generates_alter() {
        let mut curr = Schema::default();
        let mut updated = basic_function("notify");
        updated.body = "SELECT 2".to_string();
        curr.functions.insert("notify".to_string(), updated.clone());
        let mut prev = Schema::default();
        prev.functions.insert("notify".to_string(), basic_function("notify"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AlterFunction { old, new }
            if old.body == "SELECT 1" && new.body == "SELECT 2"));
    }

    /// A new trigger on an existing table generates CreateTrigger.
    #[test]
    fn new_trigger_generates_create() {
        let prev = state_with_table(empty_table("users"));
        let mut curr_table = empty_table("users");
        curr_table.triggers.push(basic_trigger("audit_trg"));
        let curr = state_with_table(curr_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateTrigger { trigger, .. } if trigger.name.as_deref() == Some("audit_trg")));
    }

    /// A removed trigger generates DropTrigger.
    #[test]
    fn removed_trigger_generates_drop() {
        let curr = state_with_table(empty_table("users"));
        let mut prev_table = empty_table("users");
        prev_table.triggers.push(basic_trigger("audit_trg"));
        let prev = state_with_table(prev_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropTrigger { trigger, .. } if trigger.name.as_deref() == Some("audit_trg")));
    }

    /// A changed trigger generates AlterTrigger.
    #[test]
    fn modified_trigger_generates_alter() {
        let mut updated = basic_trigger("audit_trg");
        updated.function_name = Some("new_fn".to_string());
        let mut curr_table = empty_table("users");
        curr_table.triggers.push(updated);
        let curr = state_with_table(curr_table);
        let mut prev_table = empty_table("users");
        prev_table.triggers.push(basic_trigger("audit_trg"));
        let prev = state_with_table(prev_table);
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::AlterTrigger { old, new, .. }
            if old.function_name.as_deref() == Some("some_fn") && new.function_name.as_deref() == Some("new_fn")));
    }

    /// CreateFunction ops appear before CreateTable ops in the result.
    #[test]
    fn function_creates_before_table_creates() {
        let mut curr = Schema::default();
        curr.tables.insert("users".to_string(), empty_table("users"));
        curr.functions.insert("notify".to_string(), basic_function("notify"));
        let prev = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::CreateFunction { .. }), "first op should be CreateFunction");
        assert!(matches!(&ops[1], Operation::CreateTable { .. }), "second op should be CreateTable");
    }

    /// DropView must precede DropTable so a view over a dropped table doesn't block the op.
    #[test]
    fn view_drops_before_table_drops() {
        let mut prev = Schema::default();
        prev.tables.insert("users".to_string(), empty_table("users"));
        prev.views.insert("v_users".to_string(), ViewDef {
            name: "v_users".to_string(),
            schema: None,
            definition: "SELECT * FROM users".to_string(),
        });
        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        let drop_view_pos = ops.iter().position(|op| matches!(op, Operation::DropView { .. })).unwrap();
        let drop_table_pos = ops.iter().position(|op| matches!(op, Operation::DropTable { .. })).unwrap();
        assert!(drop_view_pos < drop_table_pos, "DropView must precede DropTable");
    }

    /// CreateView must follow CreateTable so the table already exists when the view is created.
    #[test]
    fn view_creates_after_table_creates() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        curr.tables.insert("users".to_string(), empty_table("users"));
        curr.views.insert("v_users".to_string(), ViewDef {
            name: "v_users".to_string(),
            schema: None,
            definition: "SELECT * FROM users".to_string(),
        });
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        let create_table_pos = ops.iter().position(|op| matches!(op, Operation::CreateTable { .. })).unwrap();
        let create_view_pos = ops.iter().position(|op| matches!(op, Operation::CreateView { .. })).unwrap();
        assert!(create_table_pos < create_view_pos, "CreateTable must precede CreateView");
    }

    /// Two tables referencing each other: both are created (one with its back-edge FK stripped)
    /// and the deferred FK is emitted as AddForeignKey after both tables exist.
    #[test]
    fn mutual_fk_cycle_resolved_with_deferred_add() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let mut a = empty_table("a");
        a.foreign_keys.push(ForeignKey {
            name: "fk_a_b".to_string(),
            from_column: "b_id".to_string(),
            to_table: "b".to_string(),
            to_column: "id".to_string(),
        });
        let mut b = empty_table("b");
        b.foreign_keys.push(ForeignKey {
            name: "fk_b_a".to_string(),
            from_column: "a_id".to_string(),
            to_table: "a".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("a".to_string(), a);
        curr.tables.insert("b".to_string(), b);
        let ops = engine().diff(&curr, &prev).unwrap();
        let creates: Vec<_> = ops.iter().filter(|op| matches!(op, Operation::CreateTable { .. })).collect();
        let fk_adds: Vec<_> = ops.iter().filter(|op| matches!(op, Operation::AddForeignKey { .. })).collect();
        assert_eq!(creates.len(), 2, "both tables must be created");
        assert_eq!(fk_adds.len(), 1, "exactly one FK must be deferred to break the cycle");
        let fk_add_pos = ops.iter().position(|op| matches!(op, Operation::AddForeignKey { .. })).unwrap();
        let last_create_pos = ops.iter().rposition(|op| matches!(op, Operation::CreateTable { .. })).unwrap();
        assert!(last_create_pos < fk_add_pos, "both CreateTable ops must precede the deferred AddForeignKey");
    }

    // --- Phase ordering: cross-entity dependency tests ---

    /// When a surviving table removes its FK to a table being dropped,
    /// the DropForeignKey must come before the DropTable.
    #[test]
    fn surviving_table_fk_to_dropped_table_order() {
        let mut prev = Schema::default();
        let mut orders = empty_table("orders");
        orders.foreign_keys.push(ForeignKey {
            name: "fk_orders_users".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        prev.tables.insert("users".to_string(), empty_table("users"));
        prev.tables.insert("orders".to_string(), orders);

        let mut curr = Schema::default();
        curr.tables.insert("orders".to_string(), empty_table("orders"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_fk_pos = ops.iter().position(|op| matches!(op, Operation::DropForeignKey { .. }))
            .expect("should have DropForeignKey");
        let drop_table_pos = ops.iter().position(|op| matches!(op, Operation::DropTable { table } if table.name == "users"))
            .expect("should have DropTable for users");
        assert!(
            drop_fk_pos < drop_table_pos,
            "DropForeignKey on surviving table must precede DropTable of referenced table, got fk@{} table@{}",
            drop_fk_pos, drop_table_pos,
        );
    }

    /// Multiple surviving tables with FKs to a single dropped table: all FK drops
    /// must precede the table drop.
    #[test]
    fn multiple_surviving_fks_to_dropped_table() {
        let mut prev = Schema::default();
        prev.tables.insert("users".to_string(), empty_table("users"));
        let mut orders = empty_table("orders");
        orders.foreign_keys.push(ForeignKey {
            name: "fk_orders_users".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        let mut reviews = empty_table("reviews");
        reviews.foreign_keys.push(ForeignKey {
            name: "fk_reviews_users".to_string(),
            from_column: "author_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        prev.tables.insert("orders".to_string(), orders);
        prev.tables.insert("reviews".to_string(), reviews);

        let mut curr = Schema::default();
        curr.tables.insert("orders".to_string(), empty_table("orders"));
        curr.tables.insert("reviews".to_string(), empty_table("reviews"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_table_pos = ops.iter().position(|op| matches!(op, Operation::DropTable { .. }))
            .expect("should have DropTable");
        for (i, op) in ops.iter().enumerate() {
            if matches!(op, Operation::DropForeignKey { .. }) {
                assert!(i < drop_table_pos,
                    "DropForeignKey at {} must precede DropTable at {}", i, drop_table_pos);
            }
        }
    }

    /// Within a table diff, DropTrigger must precede DropColumn so that triggers
    /// referencing the column (in WHEN clause) don't block the column drop.
    #[test]
    fn trigger_drops_before_column_drops_within_table() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("status"));
        prev_table.triggers.push(TriggerDef {
            name: Some("status_audit_trg".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Update],
            scope: TriggerScope::Row,
            function_name: Some("audit_fn".to_string()),
            when: Some("OLD.status IS DISTINCT FROM NEW.status".to_string()),
            body: None,
            language: None,
        });
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("users"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_trigger_pos = ops.iter().position(|op| matches!(op, Operation::DropTrigger { .. }))
            .expect("should have DropTrigger");
        let drop_col_pos = ops.iter().position(|op| matches!(op, Operation::DropColumn { .. }))
            .expect("should have DropColumn");
        assert!(
            drop_trigger_pos < drop_col_pos,
            "DropTrigger must precede DropColumn, got trigger@{} col@{}",
            drop_trigger_pos, drop_col_pos,
        );
    }

    /// DropTrigger must precede DropConstraint in case the trigger depends
    /// on the constraint indirectly.
    #[test]
    fn trigger_drops_before_constraint_drops() {
        let mut prev_table = empty_table("users");
        prev_table.constraints.push(Constraint::Check {
            name: "chk_age".to_string(),
            expression: "age > 0".to_string(),
        });
        prev_table.triggers.push(basic_trigger("age_trg"));
        let prev = state_with_table(prev_table);
        let curr = state_with_table(empty_table("users"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_trigger_pos = ops.iter().position(|op| matches!(op, Operation::DropTrigger { .. }))
            .expect("should have DropTrigger");
        let drop_constraint_pos = ops.iter().position(|op| matches!(op, Operation::DropConstraint { .. }))
            .expect("should have DropConstraint");
        assert!(
            drop_trigger_pos < drop_constraint_pos,
            "DropTrigger must precede DropConstraint, got trigger@{} constraint@{}",
            drop_trigger_pos, drop_constraint_pos,
        );
    }

    /// When a function is dropped and a surviving table's trigger references it
    /// (orphan trigger), the DropTrigger must precede DropFunction.
    #[test]
    fn orphan_trigger_drop_precedes_function_drop() {
        let mut prev = Schema::default();
        prev.functions.insert("audit_fn".to_string(), basic_function("audit_fn"));
        let mut users = empty_table("users");
        users.triggers.push(TriggerDef {
            name: Some("audit_trg".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("audit_fn".to_string()),
            when: None,
            body: None,
            language: None,
        });
        prev.tables.insert("users".to_string(), users.clone());

        let mut curr = Schema::default();
        curr.tables.insert("users".to_string(), users);
        // function removed, trigger stays in table definition but should be force-dropped

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_trigger_pos = ops.iter().position(|op| matches!(op, Operation::DropTrigger { .. }))
            .expect("should have orphan DropTrigger");
        let drop_fn_pos = ops.iter().position(|op| matches!(op, Operation::DropFunction { .. }))
            .expect("should have DropFunction");
        assert!(
            drop_trigger_pos < drop_fn_pos,
            "orphan DropTrigger must precede DropFunction, got trigger@{} fn@{}",
            drop_trigger_pos, drop_fn_pos,
        );
    }

    /// Dropping a table that has triggers referencing a function being dropped:
    /// no orphan trigger drops needed since the table is gone entirely.
    #[test]
    fn no_orphan_trigger_when_table_also_dropped() {
        let mut prev = Schema::default();
        prev.functions.insert("audit_fn".to_string(), basic_function("audit_fn"));
        let mut users = empty_table("users");
        users.triggers.push(TriggerDef {
            name: Some("audit_trg".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("audit_fn".to_string()),
            when: None,
            body: None,
            language: None,
        });
        prev.tables.insert("users".to_string(), users);

        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        let trigger_drops: Vec<_> = ops.iter().filter(|op| matches!(op, Operation::DropTrigger { .. })).collect();
        assert!(trigger_drops.is_empty(), "no orphan trigger drops when table is also dropped");
    }

    // --- FK topology: advanced cycle and chain tests ---

    /// Self-referential FK (e.g. employee.manager_id → employee.id) must be deferred.
    #[test]
    fn self_referential_fk_deferred() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let mut emp = empty_table("employees");
        emp.foreign_keys.push(ForeignKey {
            name: "fk_manager".to_string(),
            from_column: "manager_id".to_string(),
            to_table: "employees".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("employees".to_string(), emp);

        let ops = engine().diff(&curr, &prev).unwrap();
        let create = ops.iter().find(|op| matches!(op, Operation::CreateTable { .. }))
            .expect("should have CreateTable");
        if let Operation::CreateTable { table } = create {
            assert!(table.foreign_keys.is_empty(),
                "self-referential FK must be stripped from CreateTable");
        }
        let deferred = ops.iter().find(|op| matches!(op, Operation::AddForeignKey { .. }))
            .expect("self-referential FK must be deferred as AddForeignKey");
        if let Operation::AddForeignKey { foreign_key, .. } = deferred {
            assert_eq!(foreign_key.name, "fk_manager");
        }
    }

    /// Three-way FK cycle: A→B, B→C, C→A. All three tables must be created,
    /// and exactly one FK must be deferred to break the cycle.
    #[test]
    fn three_way_fk_cycle() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let mut a = empty_table("a");
        a.foreign_keys.push(ForeignKey {
            name: "fk_a_b".to_string(),
            from_column: "b_id".to_string(),
            to_table: "b".to_string(),
            to_column: "id".to_string(),
        });
        let mut b = empty_table("b");
        b.foreign_keys.push(ForeignKey {
            name: "fk_b_c".to_string(),
            from_column: "c_id".to_string(),
            to_table: "c".to_string(),
            to_column: "id".to_string(),
        });
        let mut c = empty_table("c");
        c.foreign_keys.push(ForeignKey {
            name: "fk_c_a".to_string(),
            from_column: "a_id".to_string(),
            to_table: "a".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("a".to_string(), a);
        curr.tables.insert("b".to_string(), b);
        curr.tables.insert("c".to_string(), c);

        let ops = engine().diff(&curr, &prev).unwrap();
        let creates: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        let fk_adds: Vec<_> = ops.iter().filter(|op| matches!(op, Operation::AddForeignKey { .. })).collect();
        assert_eq!(creates.len(), 3, "all three tables must be created");
        assert!(!fk_adds.is_empty(), "at least one FK must be deferred for the cycle");
        let last_create_pos = ops.iter().rposition(|op| matches!(op, Operation::CreateTable { .. })).unwrap();
        for (i, op) in ops.iter().enumerate() {
            if matches!(op, Operation::AddForeignKey { .. }) {
                assert!(last_create_pos < i,
                    "deferred AddForeignKey at {} must follow last CreateTable at {}", i, last_create_pos);
            }
        }
    }

    /// Linear FK chain: orders→users→accounts. Create order must be
    /// accounts first, then users, then orders.
    #[test]
    fn linear_fk_chain_create_order() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let accounts = empty_table("accounts");
        let mut users = empty_table("users");
        users.foreign_keys.push(ForeignKey {
            name: "fk_users_accounts".to_string(),
            from_column: "account_id".to_string(),
            to_table: "accounts".to_string(),
            to_column: "id".to_string(),
        });
        let mut orders = empty_table("orders");
        orders.foreign_keys.push(ForeignKey {
            name: "fk_orders_users".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        curr.tables.insert("accounts".to_string(), accounts);
        curr.tables.insert("users".to_string(), users);
        curr.tables.insert("orders".to_string(), orders);

        let ops = engine().diff(&curr, &prev).unwrap();
        let create_names: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(create_names, vec!["accounts", "users", "orders"],
            "must create in dependency order: accounts, users, orders");
    }

    /// Linear FK chain dropped: orders→users→accounts. Drop order must be
    /// orders first (referencing), then users, then accounts (referenced).
    #[test]
    fn linear_fk_chain_drop_order() {
        let mut prev = Schema::default();
        let accounts = empty_table("accounts");
        let mut users = empty_table("users");
        users.foreign_keys.push(ForeignKey {
            name: "fk_users_accounts".to_string(),
            from_column: "account_id".to_string(),
            to_table: "accounts".to_string(),
            to_column: "id".to_string(),
        });
        let mut orders = empty_table("orders");
        orders.foreign_keys.push(ForeignKey {
            name: "fk_orders_users".to_string(),
            from_column: "user_id".to_string(),
            to_table: "users".to_string(),
            to_column: "id".to_string(),
        });
        prev.tables.insert("accounts".to_string(), accounts);
        prev.tables.insert("users".to_string(), users);
        prev.tables.insert("orders".to_string(), orders);

        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_names: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(drop_names, vec!["orders", "users", "accounts"],
            "must drop in reverse dependency order");
    }

    /// Mutual FK cycle among dropped tables must emit DropForeignKey
    /// before either DropTable.
    #[test]
    fn mutual_fk_cycle_drops_emit_drop_fk_first() {
        let mut prev = Schema::default();
        let mut a = empty_table("a");
        a.foreign_keys.push(ForeignKey {
            name: "fk_a_b".to_string(),
            from_column: "b_id".to_string(),
            to_table: "b".to_string(),
            to_column: "id".to_string(),
        });
        let mut b = empty_table("b");
        b.foreign_keys.push(ForeignKey {
            name: "fk_b_a".to_string(),
            from_column: "a_id".to_string(),
            to_table: "a".to_string(),
            to_column: "id".to_string(),
        });
        prev.tables.insert("a".to_string(), a);
        prev.tables.insert("b".to_string(), b);

        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_fk_ops: Vec<_> = ops.iter().filter(|op| matches!(op, Operation::DropForeignKey { .. })).collect();
        assert!(!drop_fk_ops.is_empty(), "cycle-breaking DropForeignKey must be emitted");
        let first_drop_fk = ops.iter().position(|op| matches!(op, Operation::DropForeignKey { .. })).unwrap();
        let first_drop_table = ops.iter().position(|op| matches!(op, Operation::DropTable { .. })).unwrap();
        assert!(first_drop_fk < first_drop_table,
            "DropForeignKey must precede all DropTable ops");
    }

    // --- Within-table ordering: comprehensive sub-entity tests ---

    /// All drop types on the same table must be in safe order:
    /// triggers, constraints, indexes, FKs, columns.
    #[test]
    fn within_table_drop_ordering_comprehensive() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("email"));
        prev_table.columns.push(text_col("status"));
        prev_table.constraints.push(Constraint::Check {
            name: "chk_status".to_string(),
            expression: "status IN ('active','inactive')".to_string(),
        });
        prev_table.indexes.push(Index {
            name: "idx_email".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            predicate: None,
        });
        prev_table.foreign_keys.push(ForeignKey {
            name: "fk_users_org".to_string(),
            from_column: "org_id".to_string(),
            to_table: "orgs".to_string(),
            to_column: "id".to_string(),
        });
        prev_table.triggers.push(basic_trigger("audit_trg"));
        let prev = state_with_table(prev_table);

        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();

        let pos = |pred: &dyn Fn(&Operation) -> bool| -> usize {
            ops.iter().position(pred).unwrap()
        };
        let trigger_pos = pos(&|op| matches!(op, Operation::DropTrigger { .. }));
        let constraint_pos = pos(&|op| matches!(op, Operation::DropConstraint { .. }));
        let index_pos = pos(&|op| matches!(op, Operation::DropIndex { .. }));
        let fk_pos = pos(&|op| matches!(op, Operation::DropForeignKey { .. }));
        let col_pos = ops.iter().position(|op| matches!(op, Operation::DropColumn { .. })).unwrap();

        assert!(trigger_pos < constraint_pos,
            "DropTrigger({trigger_pos}) must precede DropConstraint({constraint_pos})");
        assert!(constraint_pos < index_pos,
            "DropConstraint({constraint_pos}) must precede DropIndex({index_pos})");
        assert!(index_pos < fk_pos,
            "DropIndex({index_pos}) must precede DropForeignKey({fk_pos})");
        assert!(fk_pos < col_pos,
            "DropForeignKey({fk_pos}) must precede DropColumn({col_pos})");
    }

    /// All add types on the same table must be in safe order:
    /// columns, FKs, indexes, constraints, triggers.
    #[test]
    fn within_table_add_ordering_comprehensive() {
        let prev = state_with_table(empty_table("users"));

        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("email"));
        curr_table.foreign_keys.push(ForeignKey {
            name: "fk_users_org".to_string(),
            from_column: "org_id".to_string(),
            to_table: "orgs".to_string(),
            to_column: "id".to_string(),
        });
        curr_table.indexes.push(Index {
            name: "idx_email".to_string(),
            columns: vec!["email".to_string()],
            unique: true,
            predicate: None,
        });
        curr_table.constraints.push(Constraint::Check {
            name: "chk_email".to_string(),
            expression: "email IS NOT NULL".to_string(),
        });
        curr_table.triggers.push(basic_trigger("audit_trg"));
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        let pos = |pred: &dyn Fn(&Operation) -> bool| -> usize {
            ops.iter().position(pred).unwrap()
        };
        let col_pos = pos(&|op| matches!(op, Operation::AddColumn { .. }));
        let fk_pos = pos(&|op| matches!(op, Operation::AddForeignKey { .. }));
        let index_pos = pos(&|op| matches!(op, Operation::AddIndex { .. }));
        let constraint_pos = pos(&|op| matches!(op, Operation::AddConstraint { .. }));
        let trigger_pos = pos(&|op| matches!(op, Operation::CreateTrigger { .. }));

        assert!(col_pos < fk_pos, "AddColumn({col_pos}) must precede AddForeignKey({fk_pos})");
        assert!(fk_pos < index_pos, "AddForeignKey({fk_pos}) must precede AddIndex({index_pos})");
        assert!(index_pos < constraint_pos, "AddIndex({index_pos}) must precede AddConstraint({constraint_pos})");
        assert!(constraint_pos < trigger_pos, "AddConstraint({constraint_pos}) must precede CreateTrigger({trigger_pos})");
    }

    /// Replacing an entity (drop old + add new) within the same table:
    /// drop must come before add for every entity type.
    #[test]
    fn replace_all_entity_types_drop_before_add() {
        let mut prev_table = empty_table("users");
        prev_table.columns.push(text_col("old_col"));
        prev_table.foreign_keys.push(ForeignKey {
            name: "fk_old".to_string(), from_column: "a".to_string(),
            to_table: "other".to_string(), to_column: "id".to_string(),
        });
        prev_table.indexes.push(Index {
            name: "idx_old".to_string(), columns: vec!["old_col".to_string()],
            unique: false, predicate: None,
        });
        prev_table.constraints.push(Constraint::Check {
            name: "chk_old".to_string(), expression: "1=1".to_string(),
        });
        prev_table.triggers.push(basic_trigger("old_trg"));
        let prev = state_with_table(prev_table);

        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("new_col"));
        curr_table.foreign_keys.push(ForeignKey {
            name: "fk_new".to_string(), from_column: "b".to_string(),
            to_table: "other2".to_string(), to_column: "id".to_string(),
        });
        curr_table.indexes.push(Index {
            name: "idx_new".to_string(), columns: vec!["new_col".to_string()],
            unique: true, predicate: None,
        });
        curr_table.constraints.push(Constraint::Check {
            name: "chk_new".to_string(), expression: "2=2".to_string(),
        });
        curr_table.triggers.push(basic_trigger("new_trg"));
        let curr = state_with_table(curr_table);

        let ops = engine().diff(&curr, &prev).unwrap();
        let is_drop = |op: &Operation| -> bool {
            matches!(op,
                Operation::DropColumn { .. } | Operation::DropForeignKey { .. } |
                Operation::DropIndex { .. } | Operation::DropConstraint { .. } |
                Operation::DropTrigger { .. }
            )
        };
        let is_add = |op: &Operation| -> bool {
            matches!(op,
                Operation::AddColumn { .. } | Operation::AddForeignKey { .. } |
                Operation::AddIndex { .. } | Operation::AddConstraint { .. } |
                Operation::CreateTrigger { .. }
            )
        };
        let last_drop = ops.iter().rposition(|op| is_drop(op)).expect("should have drops");
        let first_add = ops.iter().position(|op| is_add(op)).expect("should have adds");
        assert!(last_drop < first_add,
            "all drops must precede all adds, got last_drop@{} first_add@{}", last_drop, first_add);
    }

    // --- Global phase ordering tests ---

    /// Full phase ordering: DropView < DropTable < CreateFunction < CreateTable
    /// < deferred AddFK < changes < orphan trigger drops < DropFunction < CreateView.
    #[test]
    fn full_phase_ordering() {
        let mut prev = Schema::default();
        prev.tables.insert("old_table".to_string(), empty_table("old_table"));
        prev.views.insert("old_view".to_string(), ViewDef {
            name: "old_view".to_string(), schema: None,
            definition: "SELECT 1".to_string(),
        });
        prev.functions.insert("old_fn".to_string(), basic_function("old_fn"));

        let mut curr = Schema::default();
        curr.tables.insert("new_table".to_string(), empty_table("new_table"));
        curr.views.insert("new_view".to_string(), ViewDef {
            name: "new_view".to_string(), schema: None,
            definition: "SELECT 2".to_string(),
        });
        curr.functions.insert("new_fn".to_string(), basic_function("new_fn"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let phase_of = |op: &Operation| -> u8 {
            match op {
                Operation::DropView { .. } => 1,
                Operation::DropTable { .. } => 2,
                Operation::CreateFunction { .. } | Operation::AlterFunction { .. } => 3,
                Operation::CreateTable { .. } => 4,
                Operation::DropFunction { .. } => 6,
                Operation::CreateView { .. } | Operation::ReplaceView { .. } => 7,
                _ => 5,
            }
        };
        for window in ops.windows(2) {
            let p0 = phase_of(&window[0]);
            let p1 = phase_of(&window[1]);
            assert!(p0 <= p1,
                "phase ordering violated: {:?} (phase {}) before {:?} (phase {})",
                window[0].type_name(), p0, window[1].type_name(), p1);
        }
    }

    /// DropFunction must come after all per-table changes that might drop triggers
    /// referencing the function.
    #[test]
    fn fn_drop_after_explicit_trigger_drop() {
        let mut prev = Schema::default();
        prev.functions.insert("audit_fn".to_string(), basic_function("audit_fn"));
        let mut users = empty_table("users");
        users.triggers.push(TriggerDef {
            name: Some("audit_trg".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("audit_fn".to_string()),
            when: None, body: None, language: None,
        });
        prev.tables.insert("users".to_string(), users);

        let mut curr = Schema::default();
        curr.tables.insert("users".to_string(), empty_table("users"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_trigger_pos = ops.iter().position(|op| matches!(op, Operation::DropTrigger { .. }))
            .expect("should have DropTrigger");
        let drop_fn_pos = ops.iter().position(|op| matches!(op, Operation::DropFunction { .. }))
            .expect("should have DropFunction");
        assert!(drop_trigger_pos < drop_fn_pos,
            "DropTrigger({drop_trigger_pos}) must precede DropFunction({drop_fn_pos})");
    }

    /// CreateFunction must come before CreateTrigger when a new trigger
    /// references a new function, across different tables.
    #[test]
    fn fn_create_before_trigger_create() {
        let mut prev = Schema::default();
        prev.tables.insert("users".to_string(), empty_table("users"));

        let mut curr = Schema::default();
        curr.functions.insert("new_fn".to_string(), basic_function("new_fn"));
        let mut users = empty_table("users");
        users.triggers.push(TriggerDef {
            name: Some("new_trg".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("new_fn".to_string()),
            when: None, body: None, language: None,
        });
        curr.tables.insert("users".to_string(), users);

        let ops = engine().diff(&curr, &prev).unwrap();
        let create_fn_pos = ops.iter().position(|op| matches!(op, Operation::CreateFunction { .. }))
            .expect("should have CreateFunction");
        let create_trigger_pos = ops.iter().position(|op| matches!(op, Operation::CreateTrigger { .. }))
            .expect("should have CreateTrigger");
        assert!(create_fn_pos < create_trigger_pos,
            "CreateFunction({create_fn_pos}) must precede CreateTrigger({create_trigger_pos})");
    }

    // --- Determinism tests ---

    /// Running diff twice on the same inputs produces identical operation lists.
    #[test]
    fn diff_is_deterministic() {
        let mut prev = Schema::default();
        let mut users = empty_table("users");
        users.columns.push(text_col("name"));
        users.foreign_keys.push(ForeignKey {
            name: "fk_users_org".to_string(), from_column: "org_id".to_string(),
            to_table: "orgs".to_string(), to_column: "id".to_string(),
        });
        prev.tables.insert("users".to_string(), users);
        prev.tables.insert("orgs".to_string(), empty_table("orgs"));

        let mut curr = Schema::default();
        let mut users2 = empty_table("users");
        users2.columns.push(text_col("email"));
        curr.tables.insert("users".to_string(), users2);
        curr.tables.insert("products".to_string(), empty_table("products"));

        let e = engine();
        let ops1 = e.diff(&curr, &prev).unwrap();
        let ops2 = e.diff(&curr, &prev).unwrap();
        assert_eq!(ops1.len(), ops2.len(), "different number of operations");
        for (a, b) in ops1.iter().zip(ops2.iter()) {
            assert_eq!(a.type_name(), b.type_name(),
                "operation types differ at same position");
        }
    }

    // --- State replay: ops applied to previous must yield current ---

    /// Comprehensive replay test: all entity types changed, state must match after applying ops.
    #[test]
    fn replay_comprehensive() {
        let mut prev = Schema::default();
        let mut users = empty_table("users");
        users.columns.push(text_col("name"));
        users.columns.push(text_col("old_col"));
        users.indexes.push(Index {
            name: "idx_old".to_string(), columns: vec!["name".to_string()],
            unique: false, predicate: None,
        });
        users.constraints.push(Constraint::Check {
            name: "chk_old".to_string(), expression: "1=1".to_string(),
        });
        prev.tables.insert("users".to_string(), users);
        prev.tables.insert("to_drop".to_string(), empty_table("to_drop"));
        prev.functions.insert("old_fn".to_string(), basic_function("old_fn"));
        prev.views.insert("old_view".to_string(), ViewDef {
            name: "old_view".to_string(), schema: None,
            definition: "SELECT 1".to_string(),
        });

        let mut curr = Schema::default();
        let mut users2 = empty_table("users");
        users2.columns.push(text_col("name"));
        users2.columns.push(text_col("new_col"));
        users2.indexes.push(Index {
            name: "idx_new".to_string(), columns: vec!["new_col".to_string()],
            unique: true, predicate: None,
        });
        users2.constraints.push(Constraint::Check {
            name: "chk_new".to_string(), expression: "2=2".to_string(),
        });
        curr.tables.insert("users".to_string(), users2);
        curr.tables.insert("to_create".to_string(), empty_table("to_create"));
        curr.functions.insert("new_fn".to_string(), basic_function("new_fn"));
        curr.views.insert("new_view".to_string(), ViewDef {
            name: "new_view".to_string(), schema: None,
            definition: "SELECT 2".to_string(),
        });

        let ops = engine().diff(&curr, &prev).unwrap();
        let mut replayed = prev.clone();
        for (i, op) in ops.iter().enumerate() {
            replayed.apply(op).unwrap_or_else(|e| panic!("op {} ({}) failed: {}", i, op.type_name(), e));
        }
        assert_eq!(replayed, curr, "replayed state must equal current state");
    }

    /// Replay test for FK cycle scenario: applying ops to empty should yield the cyclic schema.
    #[test]
    fn replay_fk_cycle() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let mut a = empty_table("a");
        a.foreign_keys.push(ForeignKey {
            name: "fk_a_b".to_string(), from_column: "b_id".to_string(),
            to_table: "b".to_string(), to_column: "id".to_string(),
        });
        let mut b = empty_table("b");
        b.foreign_keys.push(ForeignKey {
            name: "fk_b_a".to_string(), from_column: "a_id".to_string(),
            to_table: "a".to_string(), to_column: "id".to_string(),
        });
        curr.tables.insert("a".to_string(), a);
        curr.tables.insert("b".to_string(), b);

        let ops = engine().diff(&curr, &prev).unwrap();
        let mut replayed = prev.clone();
        for (i, op) in ops.iter().enumerate() {
            replayed.apply(op).unwrap_or_else(|e| panic!("op {} ({}) failed: {}", i, op.type_name(), e));
        }
        assert_eq!(replayed, curr);
    }

    /// Replay test for self-referential FK.
    #[test]
    fn replay_self_referential_fk() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        let mut emp = empty_table("employees");
        emp.foreign_keys.push(ForeignKey {
            name: "fk_manager".to_string(), from_column: "manager_id".to_string(),
            to_table: "employees".to_string(), to_column: "id".to_string(),
        });
        curr.tables.insert("employees".to_string(), emp);

        let ops = engine().diff(&curr, &prev).unwrap();
        let mut replayed = prev.clone();
        for (i, op) in ops.iter().enumerate() {
            replayed.apply(op).unwrap_or_else(|e| panic!("op {} ({}) failed: {}", i, op.type_name(), e));
        }
        assert_eq!(replayed, curr);
    }

    // --- Edge cases ---

    /// Both states empty produces zero operations.
    #[test]
    fn both_empty_no_ops() {
        let ops = engine().diff(&Schema::default(), &Schema::default()).unwrap();
        assert!(ops.is_empty());
    }

    /// A table with all entity types unchanged produces no operations.
    #[test]
    fn complex_table_unchanged() {
        let mut table = empty_table("users");
        table.columns.push(text_col("name"));
        table.foreign_keys.push(ForeignKey {
            name: "fk".to_string(), from_column: "a".to_string(),
            to_table: "b".to_string(), to_column: "id".to_string(),
        });
        table.indexes.push(Index {
            name: "idx".to_string(), columns: vec!["name".to_string()],
            unique: false, predicate: None,
        });
        table.constraints.push(Constraint::Check {
            name: "chk".to_string(), expression: "1=1".to_string(),
        });
        table.triggers.push(basic_trigger("trg"));
        let s = state_with_table(table);
        let ops = engine().diff(&s, &s).unwrap();
        assert!(ops.is_empty());
    }

    /// Dropping a self-referential table should not produce a separate DropForeignKey.
    #[test]
    fn drop_self_referential_table() {
        let mut prev = Schema::default();
        let mut emp = empty_table("employees");
        emp.foreign_keys.push(ForeignKey {
            name: "fk_manager".to_string(), from_column: "manager_id".to_string(),
            to_table: "employees".to_string(), to_column: "id".to_string(),
        });
        prev.tables.insert("employees".to_string(), emp);
        let curr = Schema::default();

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_fk_count = ops.iter().filter(|op| matches!(op, Operation::DropForeignKey { .. })).count();
        assert_eq!(drop_fk_count, 0,
            "self-referential FK should not need a separate DropForeignKey when table is dropped");
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropTable { .. }));
    }

    /// Adding a new table while modifying columns on an existing table:
    /// CreateTable must come after DropTable (for dropped tables) but changes
    /// to surviving tables can interleave.
    #[test]
    fn new_table_and_table_changes_together() {
        let mut prev = Schema::default();
        let mut users = empty_table("users");
        users.columns.push(text_col("old_field"));
        prev.tables.insert("users".to_string(), users);

        let mut curr = Schema::default();
        let mut users2 = empty_table("users");
        users2.columns.push(text_col("new_field"));
        curr.tables.insert("users".to_string(), users2);
        curr.tables.insert("products".to_string(), empty_table("products"));

        let ops = engine().diff(&curr, &prev).unwrap();
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operation::CreateTable { .. })),
            "should create the new table"
        );
        assert!(
            ops.iter()
                .any(|op| matches!(op, Operation::DropColumn { .. })),
            "should drop the removed column"
        );
        assert!(
            ops.iter().any(|op| matches!(op, Operation::AddColumn { .. })),
            "should add the new column"
        );

        // Replay: applying all ops to prev must yield curr
        let mut replayed = prev.clone();
        for (i, op) in ops.iter().enumerate() {
            replayed.apply(op).unwrap_or_else(|e| panic!("op {} ({}) failed: {}", i, op.type_name(), e));
        }
        assert_eq!(replayed, curr);
    }

    /// View depending on a modified table: ReplaceView must come after
    /// the table changes.
    #[test]
    fn replace_view_after_table_changes() {
        let mut prev = Schema::default();
        let mut users = empty_table("users");
        users.columns.push(text_col("name"));
        prev.tables.insert("users".to_string(), users);
        prev.views.insert("v_users".to_string(), ViewDef {
            name: "v_users".to_string(), schema: None,
            definition: "SELECT name FROM users".to_string(),
        });

        let mut curr = Schema::default();
        let mut users2 = empty_table("users");
        users2.columns.push(text_col("name"));
        users2.columns.push(text_col("email"));
        curr.tables.insert("users".to_string(), users2);
        curr.views.insert("v_users".to_string(), ViewDef {
            name: "v_users".to_string(), schema: None,
            definition: "SELECT name, email FROM users".to_string(),
        });

        let ops = engine().diff(&curr, &prev).unwrap();
        let add_col_pos = ops.iter().position(|op| matches!(op, Operation::AddColumn { .. }))
            .expect("should have AddColumn");
        let replace_view_pos = ops.iter().position(|op| matches!(op, Operation::ReplaceView { .. }))
            .expect("should have ReplaceView");
        assert!(add_col_pos < replace_view_pos,
            "AddColumn({add_col_pos}) must precede ReplaceView({replace_view_pos})");
    }

    /// FK from new table to another new table, plus an existing table change in the same diff.
    #[test]
    fn mixed_new_tables_with_fk_and_existing_changes() {
        let mut prev = Schema::default();
        let mut legacy = empty_table("legacy");
        legacy.columns.push(text_col("old_field"));
        prev.tables.insert("legacy".to_string(), legacy);

        let mut curr = Schema::default();
        let mut legacy2 = empty_table("legacy");
        legacy2.columns.push(text_col("new_field"));
        curr.tables.insert("legacy".to_string(), legacy2);
        let authors = empty_table("authors");
        let mut books = empty_table("books");
        books.foreign_keys.push(ForeignKey {
            name: "fk_books_authors".to_string(), from_column: "author_id".to_string(),
            to_table: "authors".to_string(), to_column: "id".to_string(),
        });
        curr.tables.insert("authors".to_string(), authors);
        curr.tables.insert("books".to_string(), books);

        let ops = engine().diff(&curr, &prev).unwrap();
        let create_names: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        let authors_idx = create_names.iter().position(|n| *n == "authors").unwrap();
        let books_idx = create_names.iter().position(|n| *n == "books").unwrap();
        assert!(authors_idx < books_idx, "authors must be created before books");

        let mut replayed = prev.clone();
        for (i, op) in ops.iter().enumerate() {
            replayed.apply(op).unwrap_or_else(|e| panic!("op {} ({}) failed: {}", i, op.type_name(), e));
        }
        assert_eq!(replayed, curr);
    }

    /// Dropping tables, functions, views all at once: verify the complete phase ordering.
    #[test]
    fn drop_everything_ordering() {
        let mut prev = Schema::default();
        prev.tables.insert("t1".to_string(), empty_table("t1"));
        prev.functions.insert("f1".to_string(), basic_function("f1"));
        prev.views.insert("v1".to_string(), ViewDef {
            name: "v1".to_string(), schema: None, definition: "SELECT 1".to_string(),
        });

        let curr = Schema::default();
        let ops = engine().diff(&curr, &prev).unwrap();

        let view_drop = ops.iter().position(|op| matches!(op, Operation::DropView { .. })).unwrap();
        let table_drop = ops.iter().position(|op| matches!(op, Operation::DropTable { .. })).unwrap();
        let fn_drop = ops.iter().position(|op| matches!(op, Operation::DropFunction { .. })).unwrap();

        assert!(view_drop < table_drop, "DropView before DropTable");
        assert!(table_drop < fn_drop, "DropTable before DropFunction");
    }

    /// Creating tables, functions, views all at once: verify the complete phase ordering.
    #[test]
    fn create_everything_ordering() {
        let prev = Schema::default();
        let mut curr = Schema::default();
        curr.tables.insert("t1".to_string(), empty_table("t1"));
        curr.functions.insert("f1".to_string(), basic_function("f1"));
        curr.views.insert("v1".to_string(), ViewDef {
            name: "v1".to_string(), schema: None, definition: "SELECT 1".to_string(),
        });

        let ops = engine().diff(&curr, &prev).unwrap();
        let fn_create = ops.iter().position(|op| matches!(op, Operation::CreateFunction { .. })).unwrap();
        let table_create = ops.iter().position(|op| matches!(op, Operation::CreateTable { .. })).unwrap();
        let view_create = ops.iter().position(|op| matches!(op, Operation::CreateView { .. })).unwrap();

        assert!(fn_create < table_create, "CreateFunction before CreateTable");
        assert!(table_create < view_create, "CreateTable before CreateView");
    }

    /// AlterFunction is emitted in the function-creates phase (before table creates),
    /// not in the drops phase.
    #[test]
    fn alter_function_before_table_creates() {
        let mut prev = Schema::default();
        prev.functions.insert("f1".to_string(), basic_function("f1"));

        let mut curr = Schema::default();
        let mut f1 = basic_function("f1");
        f1.body = "SELECT 99".to_string();
        curr.functions.insert("f1".to_string(), f1);
        curr.tables.insert("t1".to_string(), empty_table("t1"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let alter_fn_pos = ops.iter().position(|op| matches!(op, Operation::AlterFunction { .. })).unwrap();
        let create_table_pos = ops.iter().position(|op| matches!(op, Operation::CreateTable { .. })).unwrap();
        assert!(alter_fn_pos < create_table_pos,
            "AlterFunction must precede CreateTable");
    }

    /// Dropping one table that references another table also being dropped
    /// where the referenced table also references a third surviving table:
    /// ensures FK walks only consider the drop set.
    #[test]
    fn drop_set_fk_to_surviving_table_ignored() {
        let mut prev = Schema::default();
        prev.tables.insert("orgs".to_string(), empty_table("orgs"));
        let mut users = empty_table("users");
        users.foreign_keys.push(ForeignKey {
            name: "fk_users_orgs".to_string(), from_column: "org_id".to_string(),
            to_table: "orgs".to_string(), to_column: "id".to_string(),
        });
        let mut orders = empty_table("orders");
        orders.foreign_keys.push(ForeignKey {
            name: "fk_orders_users".to_string(), from_column: "user_id".to_string(),
            to_table: "users".to_string(), to_column: "id".to_string(),
        });
        prev.tables.insert("users".to_string(), users);
        prev.tables.insert("orders".to_string(), orders);

        // Drop users and orders, keep orgs
        let mut curr = Schema::default();
        curr.tables.insert("orgs".to_string(), empty_table("orgs"));

        let ops = engine().diff(&curr, &prev).unwrap();
        let drop_names: Vec<&str> = ops.iter().filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        }).collect();
        assert_eq!(drop_names.len(), 2);
        assert_eq!(drop_names[0], "orders", "orders (referencing) must drop before users");
        assert_eq!(drop_names[1], "users", "users (referenced) must drop after orders");
    }
}
