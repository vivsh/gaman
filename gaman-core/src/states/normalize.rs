use std::collections::HashSet;

use super::names;
use super::*;

impl Schema {
    /// Folds inline `references` and `check` fields declared on columns into the
    /// table-level `foreign_keys` and `constraints` vecs. Call this once after
    /// deserializing a user-authored schema file before passing it to diff or validate.
    pub fn normalize(&mut self) {
        for (table_name, table) in self.tables.iter_mut() {
            let table_name = table_name.clone();
            if table.name.is_empty() {
                table.name = table_name.clone();
            }
            normalize_table_primary_key(table);
            for col in table.columns.iter_mut() {
                if let Some(r) = col.references.take() {
                    let fk_name = r
                        .name
                        .unwrap_or_else(|| names::foreign_key(&table_name, &[col.name.as_str()]));
                    let mut foreign_key =
                        ForeignKey::single(fk_name, col.name.clone(), r.table, r.column);
                    foreign_key.on_delete = r
                        .on_delete
                        .as_deref()
                        .and_then(canonical_foreign_key_action)
                        .map(str::to_string);
                    table.foreign_keys.push(foreign_key);
                }
                if let Some(expr) = col.check.take() {
                    table.constraints.push(Constraint::Check {
                        name: names::column_check(&table_name, &col.name),
                        expression: expr,
                    });
                }
            }
            fill_derived_names(table);
            for trigger in table.triggers.iter_mut() {
                if trigger.name.is_none() {
                    let mut event_parts: Vec<&str> = trigger
                        .events
                        .iter()
                        .map(|e| match e {
                            TriggerEvent::Insert => "insert",
                            TriggerEvent::Update => "update",
                            TriggerEvent::Delete => "delete",
                            TriggerEvent::Truncate => "truncate",
                        })
                        .collect();
                    event_parts.sort_unstable();
                    let timing_part = match trigger.timing {
                        TriggerTiming::Before => "before",
                        TriggerTiming::After => "after",
                        TriggerTiming::InsteadOf => "instead_of",
                    };
                    trigger.name = Some(format!(
                        "{}_{}_{}_trg",
                        table_name,
                        event_parts.join("_"),
                        timing_part
                    ));
                }
            }
        }
        for (key, func) in self.functions.iter_mut() {
            if func.name.is_empty() {
                func.name = key.clone();
            }
        }
    }
}

pub(crate) fn normalize_table_primary_key(table: &mut Table) {
    let flagged: Vec<String> = table
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect();

    if table.primary_key.is_none() && !flagged.is_empty() {
        table.primary_key = Some(PrimaryKey {
            name: names::primary_key(&table.name),
            columns: flagged.clone(),
        });
    }

    if let Some(pk) = &mut table.primary_key
        && pk.name.is_empty()
    {
        pk.name = names::primary_key(&table.name);
    }

    let Some(pk) = &table.primary_key else {
        return;
    };

    if !flagged.is_empty() && !same_string_set(&flagged, &pk.columns) {
        return;
    }

    let pk_columns = pk.columns.clone();
    for column in &mut table.columns {
        column.primary_key = pk_columns.iter().any(|name| name == &column.name);
        if column.primary_key {
            column.nullable = false;
        }
    }
}

fn fill_derived_names(table: &mut Table) {
    let table_name = table.name.as_str();
    for fk in &mut table.foreign_keys {
        if fk.name.is_empty() {
            fk.name = names::foreign_key(table_name, &fk.columns);
        }
    }
    for index in &mut table.indexes {
        if index.name.is_empty() {
            index.name = names::index(table_name, &index.columns);
        }
    }
    for constraint in &mut table.constraints {
        match constraint {
            Constraint::Unique { name, columns } if name.is_empty() => {
                *name = names::unique(table_name, columns);
            }
            Constraint::Check { name, .. } if name.is_empty() => {
                *name = names::table_check(table_name);
            }
            _ => {}
        }
    }
}

fn same_string_set(left: &[String], right: &[String]) -> bool {
    let left: HashSet<&str> = left.iter().map(String::as_str).collect();
    let right: HashSet<&str> = right.iter().map(String::as_str).collect();
    left == right
}
