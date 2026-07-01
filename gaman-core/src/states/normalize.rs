use std::collections::HashSet;

use super::*;

impl Schema {
    /// Folds inline `references` and `check` fields declared on columns into the
    /// table-level `foreign_keys` and `constraints` vecs. Call this once after
    /// deserializing a user-authored schema file before passing it to diff or validate.
    pub fn normalize(&mut self) {
        let mut new_functions: Vec<(String, FunctionDef)> = Vec::new();
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
                        .unwrap_or_else(|| format!("{}_{}_fkey", table_name, col.name));
                    table.foreign_keys.push(ForeignKey::single(
                        fk_name,
                        col.name.clone(),
                        r.table,
                        r.column,
                    ));
                }
                if let Some(expr) = col.check.take() {
                    table.constraints.push(Constraint::Check {
                        name: format!("{}_{}_check", table_name, col.name),
                        expression: expr,
                    });
                }
            }
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
                if let Some(body) = trigger.body.take() {
                    let trigger_name = trigger.name.as_deref().unwrap();
                    let fn_name = format!("{}_fn", trigger_name);
                    let lang = trigger
                        .language
                        .take()
                        .unwrap_or_else(|| "plpgsql".to_string());
                    trigger.function_name = Some(fn_name.clone());
                    new_functions.push((
                        fn_name.clone(),
                        FunctionDef {
                            name: fn_name,
                            schema: None,
                            arguments: String::new(),
                            returns: "trigger".to_string(),
                            language: lang,
                            body,
                            volatility: Volatility::Volatile,
                            security_definer: false,
                        },
                    ));
                }
            }
        }
        for (key, func) in self.functions.iter_mut() {
            if func.name.is_empty() {
                func.name = key.clone();
            }
        }
        for (key, func) in new_functions {
            self.functions.insert(key, func);
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
            name: table.pk_constraint_name(),
            columns: flagged.clone(),
        });
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

fn same_string_set(left: &[String], right: &[String]) -> bool {
    let left: HashSet<&str> = left.iter().map(String::as_str).collect();
    let right: HashSet<&str> = right.iter().map(String::as_str).collect();
    left == right
}
