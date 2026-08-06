use std::collections::BTreeSet;

use crate::states::{Constraint, Schema, SchemaValidationError, Table};

use super::{ManagedRow, ManagedRows};

/// Canonicalizes declaration row order for stable replay fingerprints.
pub(crate) fn canonicalize_schema(schema: &mut Schema) {
    for declaration in schema.managed_rows.values_mut() {
        declaration.rows.sort_by_cached_key(|row| {
            serde_json::to_string(row).unwrap_or_else(|error| format!("invalid:{error}"))
        });
    }
}

/// Merges compatible declarations while deferring identity checks until the table is available.
pub fn merge_declaration(
    table: &str,
    existing: &mut ManagedRows,
    incoming: ManagedRows,
) -> Result<(), String> {
    let expected = shape(existing.rows.first());
    let observed = shape(incoming.rows.first());
    if expected.is_some() && observed.is_some() && expected != observed {
        return Err(format!(
            "managed rows for '{table}' use different row columns"
        ));
    }
    existing.rows.extend(incoming.rows);
    Ok(())
}

/// Validates all managed declarations against the final composed schema.
pub(crate) fn validate_schema(schema: &Schema) -> Result<(), SchemaValidationError> {
    for (table_name, managed) in &schema.managed_rows {
        let table = schema.tables.get(table_name).ok_or_else(|| {
            SchemaValidationError::Invalid(format!(
                "managed rows reference unknown table '{table_name}'"
            ))
        })?;
        validate_declaration(table_name, table, managed).map_err(SchemaValidationError::Invalid)?;
    }
    Ok(())
}

/// Resolves the table-owned identity represented in the managed row shape.
pub(crate) fn resolve_key(table: &Table, managed: &ManagedRows) -> Option<Vec<String>> {
    let shape = shape(managed.rows.first())?;
    if let Some(primary) = &table.primary_key
        && key_is_eligible(table, &primary.columns, &shape)
    {
        return Some(primary.columns.clone());
    }
    let mut candidates = table
        .constraints
        .iter()
        .filter_map(|constraint| match constraint {
            Constraint::Unique { columns, .. } if key_is_eligible(table, columns, &shape) => {
                Some(columns.clone())
            }
            _ => None,
        })
        .chain(
            table
                .indexes
                .iter()
                .filter(|index| {
                    index.unique
                        && index.predicate.is_none()
                        && key_is_eligible(table, &index.columns, &shape)
                })
                .map(|index| index.columns.clone()),
        )
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| left.len().cmp(&right.len()).then_with(|| left.cmp(right)));
    candidates.dedup();
    candidates.into_iter().next()
}

fn validate_declaration(
    table_name: &str,
    table: &Table,
    managed: &ManagedRows,
) -> Result<(), String> {
    if managed.rows.is_empty() {
        return Err(format!("managed rows for '{table_name}' must not be empty"));
    }
    let key = resolve_key(table, managed).ok_or_else(|| {
        format!("managed rows for '{table_name}' must include a non-null primary or unique key")
    })?;
    let expected_shape = shape(managed.rows.first()).unwrap_or_default();
    let mut identities = BTreeSet::new();
    for row in &managed.rows {
        if shape(Some(row)).unwrap_or_default() != expected_shape {
            return Err(format!(
                "managed rows for '{table_name}' must use one column shape"
            ));
        }
        validate_row(table_name, table, &key, row)?;
        let identity = row.identity(&key)?;
        if !identities.insert(identity.clone()) {
            return Err(format!("duplicate managed row '{table_name}[{identity}]"));
        }
    }
    Ok(())
}

fn validate_row(
    table_name: &str,
    table: &Table,
    key: &[String],
    row: &ManagedRow,
) -> Result<(), String> {
    for key_column in key {
        let value = row.values.get(key_column).ok_or_else(|| {
            format!("managed row on '{table_name}' is missing key column '{key_column}'")
        })?;
        if value.is_null() {
            return Err(format!(
                "managed row key '{table_name}.{key_column}' cannot be null"
            ));
        }
    }
    for name in row.values.keys() {
        let column = table
            .columns
            .iter()
            .find(|column| &column.name == name)
            .ok_or_else(|| {
                format!("managed row on '{table_name}' references unknown column '{name}'")
            })?;
        if column.generated.is_some() {
            return Err(format!(
                "managed row cannot own generated column '{table_name}.{name}'"
            ));
        }
    }
    for column in &table.columns {
        if !column.nullable
            && column.default.is_none()
            && column.generated.is_none()
            && !row.values.contains_key(&column.name)
        {
            return Err(format!(
                "managed row on '{table_name}' must provide required column '{}'; omitted columns need a default or must be nullable",
                column.name
            ));
        }
    }
    Ok(())
}

fn key_is_eligible(table: &Table, key: &[String], shape: &BTreeSet<&str>) -> bool {
    key.iter().all(|name| {
        shape.contains(name.as_str())
            && table
                .columns
                .iter()
                .find(|column| &column.name == name)
                .is_some_and(|column| !column.nullable)
    })
}

fn shape(row: Option<&ManagedRow>) -> Option<BTreeSet<&str>> {
    row.map(|row| row.values.keys().map(String::as_str).collect())
}
