use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::operations::Operation;
use crate::states::{Column, Constraint, ForeignKey, Index, SchemaState, Table, TriggerDef};

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

    pub fn diff(&self, current: &SchemaState, previous: &SchemaState) -> Result<Vec<Operation>, DiffError> {
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

        let mut result = Vec::with_capacity(drops.len() + creates.len() + changes.len());
        result.extend(view_drops);
        result.extend(drop_ordered(drops));
        result.extend(fn_creates);
        result.extend(creates);
        result.extend(deferred_fk_adds);
        result.extend(changes);
        result.extend(orphan_trigger_drops);
        result.extend(fn_drops);
        result.extend(view_creates);
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

    // --- removals first ---

    for c in &prev.constraints {
        if curr_cons.get(c.name()) != Some(&c) {
            ops.push(Operation::DropConstraint { table_name: name.to_string(), constraint: c.clone() });
        }
    }
    for i in &prev.indexes {
        if curr_idxs.get(i.name.as_str()) != Some(&i) {
            ops.push(Operation::DropIndex { table_name: name.to_string(), index: i.clone() });
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

    // --- additions / modifications ---

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
            ops.push(Operation::AddIndex { table_name: name.to_string(), index: i.clone() });
        }
    }
    for c in &curr.constraints {
        if prev_cons.get(c.name()) != Some(&c) {
            ops.push(Operation::AddConstraint { table_name: name.to_string(), constraint: c.clone() });
        }
    }

    let prev_trgs: HashMap<&str, &TriggerDef> = prev.triggers.iter()
        .filter_map(|t| t.name.as_deref().map(|n| (n, t))).collect();
    let curr_trgs: HashMap<&str, &TriggerDef> = curr.triggers.iter()
        .filter_map(|t| t.name.as_deref().map(|n| (n, t))).collect();
    for t in &prev.triggers {
        let tname = t.name.as_deref().unwrap_or("");
        if !curr_trgs.contains_key(tname) {
            ops.push(Operation::DropTrigger { table_name: name.to_string(), trigger: t.clone() });
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

fn diff_functions(current: &SchemaState, previous: &SchemaState) -> (Vec<Operation>, Vec<Operation>) {
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

fn diff_views(current: &SchemaState, previous: &SchemaState) -> (Vec<Operation>, Vec<Operation>) {
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
            if cyclic.contains(&(table.name.clone(), fk.name.clone())) {
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

    fn state_with_table(t: Table) -> SchemaState {
        let mut s = SchemaState::default();
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
        let prev = SchemaState::default();
        let curr = state_with_table(empty_table("users"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "users"));
    }

    /// A table present in previous but not current generates DropTable.
    #[test]
    fn removed_table_generates_drop() {
        let prev = state_with_table(empty_table("users"));
        let curr = SchemaState::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropTable { table } if table.name == "users"));
    }

    /// Multiple new tables are emitted in sorted order.
    #[test]
    fn multiple_new_tables_sorted() {
        let prev = SchemaState::default();
        let mut curr = SchemaState::default();
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
        let mut prev = SchemaState::default();
        prev.tables.insert("zebra".to_string(), empty_table("zebra"));
        prev.tables.insert("apple".to_string(), empty_table("apple"));
        let curr = SchemaState::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::DropTable { table } if table.name == "zebra"));
        assert!(matches!(&ops[1], Operation::DropTable { table } if table.name == "apple"));
    }

    /// When dropping tables with FK dependencies between them, the referencing table
    /// (the one with the FK) is dropped before the referenced table.
    #[test]
    fn dropped_tables_fk_dep_order() {
        let mut products = empty_table("products");
        let mut inventory = empty_table("inventory");
        inventory.foreign_keys.push(ForeignKey {
            name: "inventory_product_id_fkey".to_string(),
            from_column: "product_id".to_string(),
            to_table: "products".to_string(),
            to_column: "id".to_string(),
        });
        let mut prev = SchemaState::default();
        prev.tables.insert("products".to_string(), products);
        prev.tables.insert("inventory".to_string(), inventory);
        let curr = SchemaState::default();
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
        let mut prev = SchemaState::default();
        prev.tables.insert("old".to_string(), empty_table("old"));
        let mut curr = SchemaState::default();
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
        let mut prev = SchemaState::default();
        prev.tables.insert("users".to_string(), prev_table);
        prev.tables.insert("to_drop".to_string(), empty_table("to_drop"));

        let mut curr_table = empty_table("users");
        curr_table.columns.push(text_col("name"));
        curr_table.columns.push(Column { name: "old_field".to_string(), col_type: "int".to_string(), nullable: true, default: None, primary_key: false, ..Default::default() });
        curr_table.columns.push(text_col("new_field"));
        let mut curr = SchemaState::default();
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
        let prev = SchemaState::default();
        let mut curr = SchemaState::default();
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
        let mut prev = SchemaState::default();
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
        let mut curr = SchemaState::default();
        curr.functions.insert("notify".to_string(), basic_function("notify"));
        let prev = SchemaState::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::CreateFunction { function } if function.name == "notify"));
    }

    /// A function present in previous but not current generates DropFunction.
    #[test]
    fn removed_function_generates_drop() {
        let curr = SchemaState::default();
        let mut prev = SchemaState::default();
        prev.functions.insert("notify".to_string(), basic_function("notify"));
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 1);
        assert!(matches!(&ops[0], Operation::DropFunction { function } if function.name == "notify"));
    }

    /// A changed function body generates AlterFunction with correct old and new.
    #[test]
    fn modified_function_generates_alter() {
        let mut curr = SchemaState::default();
        let mut updated = basic_function("notify");
        updated.body = "SELECT 2".to_string();
        curr.functions.insert("notify".to_string(), updated.clone());
        let mut prev = SchemaState::default();
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
        let mut curr = SchemaState::default();
        curr.tables.insert("users".to_string(), empty_table("users"));
        curr.functions.insert("notify".to_string(), basic_function("notify"));
        let prev = SchemaState::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(&ops[0], Operation::CreateFunction { .. }), "first op should be CreateFunction");
        assert!(matches!(&ops[1], Operation::CreateTable { .. }), "second op should be CreateTable");
    }

    /// DropView must precede DropTable so a view over a dropped table doesn't block the op.
    #[test]
    fn view_drops_before_table_drops() {
        let mut prev = SchemaState::default();
        prev.tables.insert("users".to_string(), empty_table("users"));
        prev.views.insert("v_users".to_string(), ViewDef {
            name: "v_users".to_string(),
            schema: None,
            definition: "SELECT * FROM users".to_string(),
        });
        let curr = SchemaState::default();
        let ops = engine().diff(&curr, &prev).unwrap();
        assert_eq!(ops.len(), 2);
        let drop_view_pos = ops.iter().position(|op| matches!(op, Operation::DropView { .. })).unwrap();
        let drop_table_pos = ops.iter().position(|op| matches!(op, Operation::DropTable { .. })).unwrap();
        assert!(drop_view_pos < drop_table_pos, "DropView must precede DropTable");
    }

    /// CreateView must follow CreateTable so the table already exists when the view is created.
    #[test]
    fn view_creates_after_table_creates() {
        let prev = SchemaState::default();
        let mut curr = SchemaState::default();
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
        let prev = SchemaState::default();
        let mut curr = SchemaState::default();
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
}
