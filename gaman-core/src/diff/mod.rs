use std::collections::{BTreeSet, HashMap, HashSet};

use thiserror::Error;

use crate::dialects::Dialect;
use crate::opaque::{
    opaque_option_sources_equal, opaque_sources_equal, table_option_sources_equal,
};
use crate::operations::Operation;
use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, FunctionDef, Schema, Table, TriggerDef, ViewDef,
};

#[derive(Debug, Error)]
pub enum DiffError {
    #[error("dependency cycle detected among operations — this is a bug in the diff engine")]
    DependencyCycle,
    #[error(
        "primary key changes on existing table '{0}' are not generated automatically; use an explicit SQL statement migration"
    )]
    PrimaryKeyMutation(String),
    #[error(
        "PostgreSQL partition metadata on existing table '{0}' cannot be changed automatically; use an explicit raw SQL migration"
    )]
    PostgresPartitionMutation(String),
}

// Walk the schema maps directly. The BTreeMap keys are the canonical identity
// for each entity, so a plain keyed comparison is enough here.

pub fn generate_diff(current: &Schema, previous: &Schema) -> Vec<Operation> {
    let mut ops: Vec<Operation> = Vec::new();

    diff_map_with_eq(
        &current.extensions,
        &previous.extensions,
        &mut ops,
        extension_equal,
        diff_extension,
    );
    diff_map(&current.enums, &previous.enums, &mut ops, diff_enum);
    diff_map_with_eq(
        &current.functions,
        &previous.functions,
        &mut ops,
        function_equal,
        diff_function,
    );
    diff_map_with_eq(
        &current.views,
        &previous.views,
        &mut ops,
        view_equal,
        diff_view,
    );
    diff_map(
        &current.tables,
        &previous.tables,
        &mut ops,
        |curr, prev, ops| diff_table(curr, prev, ops, None),
    );

    ops
}

fn diff_map<T: PartialEq>(
    current: &std::collections::BTreeMap<String, T>,
    previous: &std::collections::BTreeMap<String, T>,
    ops: &mut Vec<Operation>,
    handler: fn(Option<&T>, Option<&T>, &mut Vec<Operation>),
) {
    for (key, curr_val) in current {
        match previous.get(key) {
            None => handler(Some(curr_val), None, ops),
            Some(prev_val) if prev_val != curr_val => handler(Some(curr_val), Some(prev_val), ops),
            _ => {}
        }
    }
    for (key, prev_val) in previous {
        if !current.contains_key(key) {
            handler(None, Some(prev_val), ops);
        }
    }
}

fn diff_map_with_eq<T>(
    current: &std::collections::BTreeMap<String, T>,
    previous: &std::collections::BTreeMap<String, T>,
    ops: &mut Vec<Operation>,
    equal: fn(&T, &T) -> bool,
    handler: fn(Option<&T>, Option<&T>, &mut Vec<Operation>),
) {
    for (key, curr_val) in current {
        match previous.get(key) {
            None => handler(Some(curr_val), None, ops),
            Some(prev_val) if !equal(curr_val, prev_val) => {
                handler(Some(curr_val), Some(prev_val), ops)
            }
            _ => {}
        }
    }
    for (key, prev_val) in previous {
        if !current.contains_key(key) {
            handler(None, Some(prev_val), ops);
        }
    }
}

fn diff_extension(
    curr: Option<&ExtensionDef>,
    prev: Option<&ExtensionDef>,
    ops: &mut Vec<Operation>,
) {
    match (curr, prev) {
        (Some(c), None) => ops.push(Operation::CreateExtension {
            extension: c.clone(),
        }),
        (None, Some(p)) => ops.push(Operation::DropExtension {
            extension: p.clone(),
        }),
        (Some(c), Some(p)) => {
            ops.push(Operation::DropExtension {
                extension: p.clone(),
            });
            ops.push(Operation::CreateExtension {
                extension: c.clone(),
            });
        }
        _ => {}
    }
}

fn diff_enum(curr: Option<&EnumDef>, prev: Option<&EnumDef>, ops: &mut Vec<Operation>) {
    match (curr, prev) {
        (Some(c), None) => ops.push(Operation::CreateEnum {
            enum_def: c.clone(),
        }),
        (None, Some(p)) => ops.push(Operation::DropEnum {
            enum_def: p.clone(),
        }),
        (Some(c), Some(p)) => {
            let old_set: HashSet<&str> = p.values.iter().map(|v| v.as_str()).collect();
            let new_set: HashSet<&str> = c.values.iter().map(|v| v.as_str()).collect();
            if old_set.is_subset(&new_set) && values_are_subsequence(&p.values, &c.values) {
                ops.push(Operation::AlterEnum {
                    old: p.clone(),
                    new: c.clone(),
                });
            } else {
                ops.push(Operation::DropEnum {
                    enum_def: p.clone(),
                });
                ops.push(Operation::CreateEnum {
                    enum_def: c.clone(),
                });
            }
        }
        _ => {}
    }
}

fn values_are_subsequence(old: &[String], new: &[String]) -> bool {
    let mut new_iter = new.iter();
    old.iter()
        .all(|old_value| new_iter.by_ref().any(|new_value| new_value == old_value))
}

fn diff_function(curr: Option<&FunctionDef>, prev: Option<&FunctionDef>, ops: &mut Vec<Operation>) {
    match (curr, prev) {
        (Some(c), None) => ops.push(Operation::CreateFunction {
            function: c.clone(),
        }),
        (None, Some(p)) => ops.push(Operation::DropFunction {
            function: p.clone(),
        }),
        (Some(c), Some(p)) => {
            ops.push(Operation::AlterFunction {
                old: p.clone(),
                new: c.clone(),
            });
        }
        _ => {}
    }
}

fn diff_view(curr: Option<&ViewDef>, prev: Option<&ViewDef>, ops: &mut Vec<Operation>) {
    match (curr, prev) {
        (Some(c), None) => ops.push(Operation::CreateView { view: c.clone() }),
        (None, Some(p)) => ops.push(Operation::DropView { view: p.clone() }),
        (Some(c), Some(p)) => {
            ops.push(Operation::ReplaceView {
                old: p.clone(),
                new: c.clone(),
            });
        }
        _ => {}
    }
}

fn diff_table(
    curr: Option<&Table>,
    prev: Option<&Table>,
    ops: &mut Vec<Operation>,
    dialect: Option<&Dialect>,
) {
    match (curr, prev) {
        (Some(c), None) => ops.push(Operation::CreateTable { table: c.clone() }),
        (None, Some(p)) => ops.push(Operation::DropTable { table: p.clone() }),
        (Some(c), Some(p)) => diff_table_children(p, c, ops, dialect),
        _ => {}
    }
}

fn diff_table_children(
    prev: &Table,
    curr: &Table,
    ops: &mut Vec<Operation>,
    dialect: Option<&Dialect>,
) {
    let table_name = curr.qualified_name();
    if !table_option_sources_equal(
        &prev.options.header_raw,
        &prev.options.tail_raw,
        &curr.options.header_raw,
        &curr.options.tail_raw,
    ) {
        ops.push(Operation::AcknowledgeTableOptions {
            table_name: table_name.clone(),
            old: prev.options.clone(),
            new: curr.options.clone(),
        });
    }

    for change in diff_by_name(
        &prev.columns,
        &curr.columns,
        |c| c.name.as_str(),
        |left, right| column_equal(left, right, dialect),
    ) {
        match change {
            SubChange::Removed(col) => ops.push(Operation::DropColumn {
                table_name: table_name.clone(),
                column: col.clone(),
                cascade: false,
            }),
            SubChange::Added(col) => ops.push(Operation::AddColumn {
                table_name: table_name.clone(),
                column: col.clone(),
            }),
            SubChange::Modified(old, new) => ops.push(Operation::AlterColumn {
                table_name: table_name.clone(),
                old: old.clone(),
                new: new.clone(),
                cast_expr: None,
            }),
        }
    }

    for change in diff_by_name(
        &prev.foreign_keys,
        &curr.foreign_keys,
        |fk| fk.name.as_str(),
        PartialEq::eq,
    ) {
        match change {
            SubChange::Removed(fk) => ops.push(Operation::DropForeignKey {
                table_name: table_name.clone(),
                foreign_key: fk.clone(),
                cascade: false,
            }),
            SubChange::Added(fk) => ops.push(Operation::AddForeignKey {
                table_name: table_name.clone(),
                foreign_key: fk.clone(),
            }),
            SubChange::Modified(old, new) => {
                ops.push(Operation::DropForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: old.clone(),
                    cascade: false,
                });
                ops.push(Operation::AddForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: new.clone(),
                });
            }
        }
    }

    for change in diff_by_name(
        &prev.indexes,
        &curr.indexes,
        |i| i.name.as_str(),
        index_equal,
    ) {
        match change {
            SubChange::Removed(idx) => ops.push(Operation::DropIndex {
                table_name: table_name.clone(),
                index: idx.clone(),
                concurrent: false,
            }),
            SubChange::Added(idx) => ops.push(Operation::AddIndex {
                table_name: table_name.clone(),
                index: idx.clone(),
                concurrent: false,
            }),
            SubChange::Modified(old, new) => {
                ops.push(Operation::DropIndex {
                    table_name: table_name.clone(),
                    index: old.clone(),
                    concurrent: false,
                });
                ops.push(Operation::AddIndex {
                    table_name: table_name.clone(),
                    index: new.clone(),
                    concurrent: false,
                });
            }
        }
    }

    for change in diff_by_name(
        &prev.constraints,
        &curr.constraints,
        |c| c.name(),
        constraint_equal,
    ) {
        match change {
            SubChange::Removed(con) => ops.push(Operation::DropConstraint {
                table_name: table_name.clone(),
                constraint: con.clone(),
            }),
            SubChange::Added(con) => ops.push(Operation::AddConstraint {
                table_name: table_name.clone(),
                constraint: con.clone(),
            }),
            SubChange::Modified(old, new) => {
                ops.push(Operation::DropConstraint {
                    table_name: table_name.clone(),
                    constraint: old.clone(),
                });
                ops.push(Operation::AddConstraint {
                    table_name: table_name.clone(),
                    constraint: new.clone(),
                });
            }
        }
    }

    for change in diff_by_name(
        &prev.triggers,
        &curr.triggers,
        |t| t.name.as_deref().unwrap_or(""),
        trigger_equal,
    ) {
        match change {
            SubChange::Removed(trg) => ops.push(Operation::DropTrigger {
                table_name: table_name.clone(),
                trigger: trg.clone(),
            }),
            SubChange::Added(trg) => ops.push(Operation::CreateTrigger {
                table_name: table_name.clone(),
                trigger: trg.clone(),
            }),
            SubChange::Modified(old, new) => ops.push(Operation::AlterTrigger {
                table_name: table_name.clone(),
                old: old.clone(),
                new: new.clone(),
            }),
        }
    }
}

fn column_equal(left: &Column, right: &Column, dialect: Option<&Dialect>) -> bool {
    let type_equal = dialect.map_or_else(
        || left.col_type == right.col_type,
        |dialect| {
            dialect.type_comparison_key(&left.col_type)
                == dialect.type_comparison_key(&right.col_type)
        },
    );
    type_equal
        && left.name == right.name
        && left.nullable == right.nullable
        && dialect.map_or_else(
            || left.default == right.default,
            |dialect| match (&left.default, &right.default) {
                (Some(left), Some(right)) => dialect.default_expressions_equal(left, right),
                (None, None) => true,
                _ => false,
            },
        )
        && left.primary_key == right.primary_key
        && left.references == right.references
        && left.check == right.check
        && left.generated == right.generated
}

fn function_equal(left: &FunctionDef, right: &FunctionDef) -> bool {
    if left.is_opaque() || right.is_opaque() {
        return left.qualified_name() == right.qualified_name()
            && opaque_option_sources_equal(&left.opaque.raw, &right.opaque.raw);
    }
    left.name == right.name
        && left.schema == right.schema
        && left.parameters_sql() == right.parameters_sql()
        && left.depends_on == right.depends_on
        && left.returns == right.returns
        && left.language == right.language
        && left.volatility == right.volatility
        && left.security_definer == right.security_definer
        && opaque_sources_equal(&left.body, &right.body)
}

fn view_equal(left: &ViewDef, right: &ViewDef) -> bool {
    if left.is_opaque() || right.is_opaque() {
        return left.qualified_name() == right.qualified_name()
            && opaque_option_sources_equal(&left.opaque.raw, &right.opaque.raw);
    }
    left.name == right.name
        && left.schema == right.schema
        && opaque_sources_equal(&left.definition, &right.definition)
}

fn trigger_equal(left: &TriggerDef, right: &TriggerDef) -> bool {
    if left.is_opaque() || right.is_opaque() {
        return left.name == right.name
            && opaque_option_sources_equal(&left.opaque.raw, &right.opaque.raw);
    }
    left.name == right.name
        && left.timing == right.timing
        && left.events == right.events
        && left.scope == right.scope
        && left.function_name == right.function_name
        && opaque_option_sources_equal(&left.when, &right.when)
        && opaque_option_sources_equal(&left.query, &right.query)
        && left.language == right.language
}

fn constraint_equal(left: &Constraint, right: &Constraint) -> bool {
    if left.is_opaque() || right.is_opaque() {
        let left_raw = left.raw_sql().map(ToOwned::to_owned);
        let right_raw = right.raw_sql().map(ToOwned::to_owned);
        return left.name() == right.name() && opaque_option_sources_equal(&left_raw, &right_raw);
    }
    left == right
}

fn extension_equal(left: &ExtensionDef, right: &ExtensionDef) -> bool {
    if left.is_opaque() || right.is_opaque() {
        return left.qualified_name() == right.qualified_name()
            && opaque_option_sources_equal(&left.opaque.raw, &right.opaque.raw);
    }
    left == right
}

fn index_equal(left: &crate::states::Index, right: &crate::states::Index) -> bool {
    if left.is_opaque() || right.is_opaque() {
        return left.name == right.name
            && opaque_option_sources_equal(&left.opaque.raw, &right.opaque.raw);
    }
    left == right
}

enum SubChange<'a, T> {
    Added(&'a T),
    Removed(&'a T),
    Modified(&'a T, &'a T),
}

fn diff_by_name<'a, T>(
    prev: &'a [T],
    curr: &'a [T],
    name_fn: impl Fn(&T) -> &str,
    equal: impl Fn(&T, &T) -> bool,
) -> Vec<SubChange<'a, T>> {
    let prev_map: HashMap<&str, &T> = prev.iter().map(|v| (name_fn(v), v)).collect();
    let curr_map: HashMap<&str, &T> = curr.iter().map(|v| (name_fn(v), v)).collect();
    let mut changes = Vec::new();

    for (&name, &prev_val) in &prev_map {
        if !curr_map.contains_key(name) {
            changes.push(SubChange::Removed(prev_val));
        }
    }
    for (&name, &curr_val) in &curr_map {
        match prev_map.get(name) {
            None => changes.push(SubChange::Added(curr_val)),
            Some(&prev_val) if !equal(prev_val, curr_val) => {
                changes.push(SubChange::Modified(prev_val, curr_val))
            }
            _ => {}
        }
    }
    changes
}

// When a function is dropped but a surviving table still carries a trigger
// referencing it, that trigger is not detected by the detection pass (the
// trigger node is identical in both states). We inject the DropTrigger here.
fn inject_orphan_triggers(ops: Vec<Operation>, previous: &Schema) -> Vec<Operation> {
    let dropped_fns: HashSet<String> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropFunction { function } => Some(function.qualified_name()),
            _ => None,
        })
        .collect();

    if dropped_fns.is_empty() {
        return ops;
    }

    let dropped_tables: HashSet<String> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.qualified_name()),
            _ => None,
        })
        .collect();

    let existing_drop_keys: HashSet<String> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropTrigger {
                table_name,
                trigger,
            } => trigger
                .name
                .as_deref()
                .map(|n| format!("{}:{}", table_name, n)),
            _ => None,
        })
        .collect();

    let mut injected: Vec<Operation> = Vec::new();
    for (table_name, table) in &previous.tables {
        if dropped_tables.contains(table_name) {
            continue;
        }
        for trg in &table.triggers {
            let references_dropped = trg
                .function_name
                .as_deref()
                .is_some_and(|f| dropped_fns.contains(f));
            if !references_dropped {
                continue;
            }
            let key = trg
                .name
                .as_deref()
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

// Incompatible enum changes (drop+recreate) require surviving columns that use
// the enum to be temporarily cast to text and back. Without this, DropEnum fails
// because the type is still in use.
// Limitation: enum references inside SQL (defaults, check constraints, function
// bodies, view definitions) cannot be tracked without parsing SQL.
fn inject_enum_column_casts(ops: Vec<Operation>, previous: &Schema) -> Vec<Operation> {
    let dropped_enums: HashSet<&str> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropEnum { enum_def } => Some(enum_def.name.as_str()),
            _ => None,
        })
        .collect();
    let created_enums: HashSet<&str> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::CreateEnum { enum_def } => Some(enum_def.name.as_str()),
            _ => None,
        })
        .collect();

    let recreated: HashSet<&str> = dropped_enums
        .intersection(&created_enums)
        .copied()
        .collect();
    if recreated.is_empty() {
        return ops;
    }

    let dropped_tables: HashSet<String> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.qualified_name()),
            _ => None,
        })
        .collect();

    let touched_columns: HashSet<(&str, &str)> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropColumn {
                table_name, column, ..
            } => Some((table_name.as_str(), column.name.as_str())),
            Operation::AlterColumn {
                table_name, old, ..
            } => Some((table_name.as_str(), old.name.as_str())),
            _ => None,
        })
        .collect();

    let mut casts: Vec<Operation> = Vec::new();

    for (table_name, table) in &previous.tables {
        if dropped_tables.contains(table_name) {
            continue;
        }
        for col in &table.columns {
            if !recreated.contains(col.col_type.as_str()) {
                continue;
            }
            if touched_columns.contains(&(table_name.as_str(), col.name.as_str())) {
                continue;
            }
            let text_col = Column {
                col_type: "text".to_string(),
                ..col.clone()
            };
            casts.push(Operation::AlterColumn {
                table_name: table_name.clone(),
                old: col.clone(),
                new: text_col.clone(),
                cast_expr: Some(format!("{}::text", col.name)),
            });
            casts.push(Operation::AlterColumn {
                table_name: table_name.clone(),
                old: text_col,
                new: col.clone(),
                cast_expr: Some(format!("{}::{}", col.name, col.col_type)),
            });
        }
    }

    if casts.is_empty() {
        return ops;
    }

    let mut result = Vec::with_capacity(ops.len() + casts.len());
    result.extend(ops);
    result.extend(casts);
    result
}

// Splits compound CreateTable/DropTable into atomic operations so
// the topological sort can order sub-entities independently.
// CreateTable → CreateTable(columns only) + AddFK* + AddIndex* + AddConstraint* + CreateTrigger*
// DropTable → DropTrigger* + DropConstraint* + DropIndex* + DropFK* + DropTable(columns only)
fn decompose(ops: Vec<Operation>) -> Vec<Operation> {
    let mut result = Vec::with_capacity(ops.len() * 2);
    for op in ops {
        match op {
            Operation::CreateTable { mut table } => {
                let fks = std::mem::take(&mut table.foreign_keys);
                let indexes = std::mem::take(&mut table.indexes);
                let constraints = std::mem::take(&mut table.constraints);
                let triggers = std::mem::take(&mut table.triggers);
                let tname = table.name.clone();
                result.push(Operation::CreateTable { table });
                for fk in fks {
                    result.push(Operation::AddForeignKey {
                        table_name: tname.clone(),
                        foreign_key: fk,
                    });
                }
                for idx in indexes {
                    result.push(Operation::AddIndex {
                        table_name: tname.clone(),
                        index: idx,
                        concurrent: false,
                    });
                }
                for con in constraints {
                    result.push(Operation::AddConstraint {
                        table_name: tname.clone(),
                        constraint: con,
                    });
                }
                for trg in triggers {
                    result.push(Operation::CreateTrigger {
                        table_name: tname.clone(),
                        trigger: trg,
                    });
                }
            }
            Operation::DropTable { mut table } => {
                let fks = std::mem::take(&mut table.foreign_keys);
                let indexes = std::mem::take(&mut table.indexes);
                let constraints = std::mem::take(&mut table.constraints);
                let triggers = std::mem::take(&mut table.triggers);
                let tname = table.name.clone();
                for trg in triggers {
                    result.push(Operation::DropTrigger {
                        table_name: tname.clone(),
                        trigger: trg,
                    });
                }
                for con in constraints {
                    result.push(Operation::DropConstraint {
                        table_name: tname.clone(),
                        constraint: con,
                    });
                }
                for idx in indexes {
                    result.push(Operation::DropIndex {
                        table_name: tname.clone(),
                        index: idx,
                        concurrent: false,
                    });
                }
                for fk in fks {
                    result.push(Operation::DropForeignKey {
                        table_name: tname.clone(),
                        foreign_key: fk,
                        cascade: false,
                    });
                }
                result.push(Operation::DropTable { table });
            }
            other => result.push(other),
        }
    }
    result
}

// After sorting, folds decomposed sub-entity ops back into their parent
// CreateTable when the dialect supports inline definitions (e.g. Postgres
// can inline FKs, indexes, constraints). This produces cleaner migrations.
// FKs are only inlined when the reference is self-referential or points to a
// table that already existed before this migration — cross-table FKs within
// the same migration stay as standalone ops (already sorted after all creates).
fn fk_can_be_inlined(
    to_table: &str,
    owner_table: &str,
    tables_being_created: &HashSet<String>,
) -> bool {
    to_table == owner_table || !tables_being_created.contains(to_table)
}

fn merge_operations(ops: Vec<Operation>, dialect: &Dialect) -> Vec<Operation> {
    let tables_being_created: HashSet<String> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::CreateTable { table } => Some(table.qualified_name()),
            _ => None,
        })
        .collect();

    let mut result: Vec<Operation> = Vec::with_capacity(ops.len());
    let mut created_tables: HashMap<String, usize> = HashMap::new();
    for op in ops {
        let merged = match &op {
            Operation::AddForeignKey {
                table_name,
                foreign_key,
            } if created_tables.contains_key(table_name)
                && dialect.should_merge(table_name, &op)
                && fk_can_be_inlined(&foreign_key.to_table, table_name, &tables_being_created) =>
            {
                let idx = created_tables[table_name];
                if let Operation::CreateTable { ref mut table } = result[idx] {
                    table.foreign_keys.push(foreign_key.clone());
                }
                true
            }
            Operation::AddIndex {
                table_name, index, ..
            } if created_tables.contains_key(table_name)
                && dialect.should_merge(table_name, &op) =>
            {
                let idx = created_tables[table_name];
                if let Operation::CreateTable { ref mut table } = result[idx] {
                    table.indexes.push(index.clone());
                }
                true
            }
            Operation::AddConstraint {
                table_name,
                constraint,
            } if created_tables.contains_key(table_name)
                && dialect.should_merge(table_name, &op) =>
            {
                let idx = created_tables[table_name];
                if let Operation::CreateTable { ref mut table } = result[idx] {
                    table.constraints.push(constraint.clone());
                }
                true
            }
            Operation::CreateTrigger {
                table_name,
                trigger,
            } if created_tables.contains_key(table_name)
                && dialect.should_merge(table_name, &op) =>
            {
                let idx = created_tables[table_name];
                if let Operation::CreateTable { ref mut table } = result[idx] {
                    table.triggers.push(trigger.clone());
                }
                true
            }
            _ => false,
        };
        if !merged {
            if let Operation::CreateTable { ref table } = op {
                created_tables.insert(table.qualified_name(), result.len());
            }
            result.push(op);
        }
    }
    result
}

// Topologically sort operations with Kahn's algorithm. When multiple nodes are
// otherwise equal, break ties deterministically so output stays reproducible.

pub fn sort_operations(ops: Vec<Operation>) -> Result<Vec<Operation>, DiffError> {
    let n = ops.len();
    if n == 0 {
        return Ok(ops);
    }

    let mut adj: Vec<Vec<usize>> = vec![vec![]; n];
    let mut in_deg: Vec<usize> = vec![0; n];

    build_dependency_edges(&ops, &mut adj, &mut in_deg);

    // Kahn's BFS with deterministic tie-breaking via BTreeSet.
    let mut queue: BTreeSet<(u8, u8, String, usize)> = BTreeSet::new();
    for i in 0..n {
        if in_deg[i] == 0 {
            let (tp, sp) = tiebreak_priority(&ops[i]);
            queue.insert((tp, sp, ops[i].entity_name().to_string(), i));
        }
    }

    let mut sorted_indices: Vec<usize> = Vec::with_capacity(n);
    while let Some(entry) = queue.iter().next().cloned() {
        queue.remove(&entry);
        let idx = entry.3;
        sorted_indices.push(idx);
        for &dep in &adj[idx] {
            in_deg[dep] -= 1;
            if in_deg[dep] == 0 {
                let (tp, sp) = tiebreak_priority(&ops[dep]);
                queue.insert((tp, sp, ops[dep].entity_name().to_string(), dep));
            }
        }
    }

    if sorted_indices.len() != n {
        return Err(DiffError::DependencyCycle);
    }

    let mut slots: Vec<Option<Operation>> = ops.into_iter().map(Some).collect();
    let result = sorted_indices
        .into_iter()
        .map(|i| slots[i].take().expect("index used twice"))
        .collect();
    Ok(result)
}

fn tiebreak_priority(op: &Operation) -> (u8, u8) {
    match op {
        Operation::CreateExtension { .. } => (0, 0),
        Operation::CreateEnum { .. }
        | Operation::RenameEnumValue { .. }
        | Operation::AlterEnum { .. } => (1, 0),
        Operation::DropView { .. } => (2, 0),
        Operation::ReplaceView { .. } => (11, 0),
        Operation::DropTable { .. } => (4, 0),
        Operation::CreateFunction { .. } | Operation::AlterFunction { .. } => (5, 0),
        Operation::CreateTable { .. } => (6, 0),
        Operation::DeleteRow { .. } => (7, 0),
        Operation::DropTrigger { .. } => (8, 0),
        Operation::DropConstraint { .. } => (8, 1),
        Operation::DropIndex { .. } => (8, 2),
        Operation::DropForeignKey { .. } => (8, 3),
        Operation::DropColumn { .. } => (8, 4),
        Operation::AlterColumn { .. } => (8, 5),
        Operation::AddColumn { .. } => (8, 6),
        Operation::AddForeignKey { .. } => (8, 7),
        Operation::AddIndex { .. } => (8, 8),
        Operation::AddConstraint { .. } => (8, 9),
        Operation::CreateTrigger { .. } | Operation::AlterTrigger { .. } => (8, 10),
        Operation::DropFunction { .. } => (10, 0),
        Operation::CreateView { .. } => (11, 0),
        Operation::DropEnum { .. } => (12, 0),
        Operation::DropExtension { .. } => (13, 0),
        Operation::Statement { .. }
        | Operation::AcknowledgeTableOptions { .. }
        | Operation::RenameTable { .. }
        | Operation::RenameColumn { .. } => (8, 5),
        Operation::InsertRow { .. } => (9, 0),
        Operation::UpdateRow { .. } => (9, 1),
    }
}

use crate::states::types::{Dep, EntityKind};

fn build_dependency_edges(ops: &[Operation], adj: &mut [Vec<usize>], in_deg: &mut [usize]) {
    let mut seen: HashSet<(usize, usize)> = HashSet::new();

    let entity_names: Vec<std::borrow::Cow<str>> = ops.iter().map(|op| op.entity_name()).collect();
    let mut create_idx: HashMap<(EntityKind, &str), Vec<usize>> = HashMap::new();
    let mut drop_idx: HashMap<(EntityKind, &str), Vec<usize>> = HashMap::new();

    for (i, op) in ops.iter().enumerate() {
        if let Some(kind) = op.entity_kind() {
            let name: &str = &entity_names[i];
            if op.is_drop() {
                drop_idx.entry((kind, name)).or_default().push(i);
            } else {
                create_idx.entry((kind, name)).or_default().push(i);
            }
        }
    }

    let mut add_edge = |from: usize, to: usize| {
        if from != to && seen.insert((from, to)) {
            adj[from].push(to);
            in_deg[to] += 1;
        }
    };

    let resolve_create = |dep: &Dep| -> Vec<usize> {
        match &dep.name {
            Some(name) => resolve_named_dependency(&create_idx, dep.kind, name),
            None => create_idx
                .iter()
                .filter(|((k, _), _)| *k == dep.kind)
                .flat_map(|(_, v)| v.iter().copied())
                .collect(),
        }
    };

    let resolve_drop = |dep: &Dep| -> Vec<usize> {
        match &dep.name {
            Some(name) => resolve_named_dependency(&drop_idx, dep.kind, name),
            None => drop_idx
                .iter()
                .filter(|((k, _), _)| *k == dep.kind)
                .flat_map(|(_, v)| v.iter().copied())
                .collect(),
        }
    };

    for (i, op) in ops.iter().enumerate() {
        if !op.is_drop() {
            for dep in op.forward_deps() {
                for j in resolve_create(&dep) {
                    add_edge(j, i);
                }
            }
        }
        if !op.is_create() {
            for dep in op.backward_deps() {
                for k in resolve_drop(&dep) {
                    add_edge(i, k);
                }
            }
        }
    }

    for (child_index, operation) in ops.iter().enumerate() {
        let Operation::DropTable { table } = operation else {
            continue;
        };
        for foreign_key in &table.foreign_keys {
            for parent_index in
                resolve_named_dependency(&drop_idx, EntityKind::Table, &foreign_key.to_table)
            {
                add_edge(child_index, parent_index);
            }
        }
    }

    let mut by_table: HashMap<&str, Vec<usize>> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        if let Some(tn) = op.table_name() {
            let (tp, _) = tiebreak_priority(op);
            if tp == 8 {
                by_table.entry(tn).or_default().push(i);
            }
        }
    }
    for indices in by_table.values() {
        for &a in indices {
            for &b in indices {
                if a == b {
                    continue;
                }
                let (_, spa) = tiebreak_priority(&ops[a]);
                let (_, spb) = tiebreak_priority(&ops[b]);
                if spa < spb {
                    add_edge(a, b);
                }
            }
        }
    }
}

/// Resolves exact entity identities and the canonical zero-argument function
/// signature used by function operations.
fn resolve_named_dependency(
    index: &HashMap<(EntityKind, &str), Vec<usize>>,
    kind: EntityKind,
    name: &str,
) -> Vec<usize> {
    if let Some(matches) = index.get(&(kind, name)) {
        return matches.clone();
    }
    if !matches!(kind, EntityKind::Function | EntityKind::Table) {
        return Vec::new();
    }
    let normalized = normalize_dependency_identity(kind, name);
    index
        .iter()
        .filter(|((candidate_kind, candidate), _)| {
            *candidate_kind == kind && normalize_dependency_identity(kind, candidate) == normalized
        })
        .flat_map(|(_, matches)| matches.iter().copied())
        .collect()
}

fn normalize_dependency_identity(kind: EntityKind, identity: &str) -> &str {
    match kind {
        EntityKind::Function => identity.strip_suffix("()").unwrap_or(identity),
        EntityKind::Table => identity.strip_prefix("public.").unwrap_or(identity),
        _ => identity,
    }
}

pub(crate) fn dependency_closure(
    operations: &[Operation],
    seeds: &HashSet<usize>,
) -> HashSet<usize> {
    let mut adjacency = vec![Vec::new(); operations.len()];
    let mut incoming = vec![0; operations.len()];
    build_dependency_edges(operations, &mut adjacency, &mut incoming);
    let mut reverse = vec![Vec::new(); operations.len()];
    for (prerequisite, dependents) in adjacency.iter().enumerate() {
        for dependent in dependents {
            reverse[*dependent].push(prerequisite);
        }
    }
    let mut selected = seeds.clone();
    let mut pending = seeds.iter().copied().collect::<Vec<_>>();
    while let Some(index) = pending.pop() {
        for prerequisite in &reverse[index] {
            if selected.insert(*prerequisite) {
                pending.push(*prerequisite);
            }
        }
    }
    selected
}

// Public entry point for diff generation.

pub struct DiffEngine;

impl DiffEngine {
    pub fn new() -> Self {
        Self
    }

    pub fn diff(
        &self,
        current: &Schema,
        previous: &Schema,
        dialect: &Dialect,
    ) -> Result<Vec<Operation>, DiffError> {
        let mut current = current.clone();
        let mut previous = previous.clone();
        current.canonicalize(dialect);
        previous.canonicalize(dialect);
        reject_postgres_partition_mutations(&current, &previous)?;
        reject_primary_key_mutations(&current, &previous)?;

        let raw_ops = generate_diff_for_dialect(&current, &previous, dialect);
        let mut raw_ops = raw_ops;
        raw_ops.extend(crate::managed_rows::diff_schemas(&current, &previous));
        let ops = inject_orphan_triggers(raw_ops, &previous);
        let ops = inject_enum_column_casts(ops, &previous);
        let ops = decompose(ops);
        let ops = sort_operations(ops)?;
        let ops = crate::managed_rows::order_operations(ops, &current, &previous)?;
        Ok(merge_operations(ops, dialect))
    }
}

fn reject_postgres_partition_mutations(
    current: &Schema,
    previous: &Schema,
) -> Result<(), DiffError> {
    for (name, current_table) in &current.tables {
        let Some(previous_table) = previous.tables.get(name) else {
            continue;
        };
        if current_table.options.postgres_partition != previous_table.options.postgres_partition {
            return Err(DiffError::PostgresPartitionMutation(name.clone()));
        }
    }
    Ok(())
}

fn generate_diff_for_dialect(
    current: &Schema,
    previous: &Schema,
    dialect: &Dialect,
) -> Vec<Operation> {
    let mut current_without_tables = current.clone();
    current_without_tables.tables.clear();
    let mut previous_without_tables = previous.clone();
    previous_without_tables.tables.clear();
    let mut ops = generate_diff(&current_without_tables, &previous_without_tables);

    for (key, current_table) in &current.tables {
        match previous.tables.get(key) {
            None => diff_table(Some(current_table), None, &mut ops, Some(dialect)),
            Some(previous_table) => {
                diff_table(
                    Some(current_table),
                    Some(previous_table),
                    &mut ops,
                    Some(dialect),
                );
            }
        }
    }
    for (key, previous_table) in &previous.tables {
        if !current.tables.contains_key(key) {
            diff_table(None, Some(previous_table), &mut ops, Some(dialect));
        }
    }
    ops
}

fn reject_primary_key_mutations(current: &Schema, previous: &Schema) -> Result<(), DiffError> {
    for (name, current_table) in &current.tables {
        let Some(previous_table) = previous.tables.get(name) else {
            continue;
        };
        if current_table.primary_key != previous_table.primary_key {
            return Err(DiffError::PrimaryKeyMutation(name.clone()));
        }
    }
    Ok(())
}

impl Default for DiffEngine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod test_diff;
