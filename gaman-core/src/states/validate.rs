use std::collections::HashSet;

use crate::dialects::Dialect;

use super::*;

impl Schema {
    pub fn prepare(mut self, dialect: Dialect) -> Result<Self, SchemaValidationError> {
        self.prepare_mut(&dialect)?;
        Ok(self)
    }

    pub fn prepare_mut(&mut self, dialect: &Dialect) -> Result<(), SchemaValidationError> {
        self.normalize();
        self.canonicalize(dialect);
        self.validate_checked()?;
        dialect.validate_schema(self)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_checked().map_err(|err| err.to_string())
    }

    pub fn validate_checked(&self) -> Result<(), SchemaValidationError> {
        for (name, table) in &self.tables {
            if table.name.is_empty() {
                return Err(SchemaValidationError::Invalid(format!(
                    "table with key '{name}' has an empty name — omit `name:` to inherit the key, or set it explicitly"
                )));
            }
            validate_table_primary_key(name, table)?;
            let mut seen = HashSet::new();
            for col in &table.columns {
                if !seen.insert(col.name.as_str()) {
                    return Err(SchemaValidationError::Invalid(format!(
                        "table '{name}' has duplicate column '{}'",
                        col.name
                    )));
                }
            }
            validate_table_references(self, name, table)?;
            for trigger in &table.triggers {
                if trigger.is_opaque() {
                    continue;
                }
                if trigger.events.is_empty() {
                    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
                    return Err(SchemaValidationError::Invalid(format!(
                        "trigger '{tname}' on table '{name}' has no events"
                    )));
                }
                validate_trigger_source(name, trigger)?;
            }
        }
        Ok(())
    }
}

fn validate_trigger_source(
    table_name: &str,
    trigger: &TriggerDef,
) -> Result<(), SchemaValidationError> {
    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
    let has_function = trigger
        .function_name
        .as_deref()
        .is_some_and(|name| !name.trim().is_empty());
    let has_query = trigger
        .query
        .as_deref()
        .is_some_and(|query| !query.trim().is_empty());

    match (has_function, has_query) {
        (true, false) | (false, true) => Ok(()),
        (true, true) => Err(SchemaValidationError::Invalid(format!(
            "trigger '{tname}' on table '{table_name}' must set either `function_name` or `query`, not both"
        ))),
        (false, false) => Err(SchemaValidationError::Invalid(format!(
            "trigger '{tname}' on table '{table_name}' must set either `function_name` or `query`"
        ))),
    }
}

fn validate_table_references(
    schema: &Schema,
    table_name: &str,
    table: &Table,
) -> Result<(), SchemaValidationError> {
    let column_names: HashSet<&str> = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();

    let mut index_names = HashSet::new();
    for index in &table.indexes {
        if !index_names.insert(index.name.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate index '{}'",
                index.name
            )));
        }
        if index.is_opaque() {
            continue;
        }
        for column in &index.columns {
            if !column_names.contains(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} index {}: unknown column '{column}'",
                    index.name
                )));
            }
        }
    }

    let mut constraint_names = HashSet::new();
    for constraint in &table.constraints {
        if !constraint_names.insert(constraint.name()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate constraint '{}'",
                constraint.name()
            )));
        }
        if let Constraint::Unique { name, columns } = constraint {
            for column in columns {
                if !column_names.contains(column.as_str()) {
                    return Err(SchemaValidationError::Invalid(format!(
                        "table {table_name} constraint {name}: unknown column '{column}'"
                    )));
                }
            }
        } else if let Constraint::Opaque { name, .. } = constraint
            && name.trim().is_empty()
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has opaque constraint with empty name"
            )));
        }
    }

    let mut fk_names = HashSet::new();
    for fk in &table.foreign_keys {
        if !fk_names.insert(fk.name.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate foreign key '{}'",
                fk.name
            )));
        }
        if fk.columns.is_empty() {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: source columns must not be empty",
                fk.name
            )));
        }
        if fk.to_columns.is_empty() {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: target columns must not be empty",
                fk.name
            )));
        }
        if let Some(action) = &fk.on_delete
            && canonical_foreign_key_action(action).is_none()
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: unsupported on_delete action '{}'",
                fk.name, action
            )));
        }
        if let Some(action) = &fk.on_update
            && canonical_foreign_key_action(action).is_none()
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: unsupported on_update action '{}'",
                fk.name, action
            )));
        }
        if fk.columns.len() != fk.to_columns.len() {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: source and target column counts differ",
                fk.name
            )));
        }
        let mut fk_source_seen = HashSet::new();
        for column in &fk.columns {
            if !fk_source_seen.insert(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} foreign key {} repeats source column '{}'",
                    fk.name, column
                )));
            }
            if !column_names.contains(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} foreign key {}: unknown source column '{}'",
                    fk.name, column
                )));
            }
        }
        let Some((_, target)) = table_by_reference(schema, &fk.to_table) else {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: referenced table {} not found",
                fk.name, fk.to_table
            )));
        };
        let target_column_names: HashSet<&str> = target
            .columns
            .iter()
            .map(|column| column.name.as_str())
            .collect();
        let mut fk_target_seen = HashSet::new();
        for column in &fk.to_columns {
            if !fk_target_seen.insert(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} foreign key {} repeats target column '{}'",
                    fk.name, column
                )));
            }
            if !target_column_names.contains(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} foreign key {}: referenced column '{}.{}' not found",
                    fk.name, fk.to_table, column
                )));
            }
        }
        if !target_has_unique_key_for_columns(target, &fk.to_columns) {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: referenced columns '{}({})' are not a primary key, unique constraint, or unique index",
                fk.name,
                fk.to_table,
                fk.to_columns.join(", ")
            )));
        }
    }

    let mut trigger_names = HashSet::new();
    for trigger in &table.triggers {
        if let Some(name) = &trigger.name
            && !trigger_names.insert(name.as_str())
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate trigger '{name}'"
            )));
        }
    }

    Ok(())
}

fn table_by_reference<'a>(schema: &'a Schema, reference: &str) -> Option<(&'a String, &'a Table)> {
    if let Some(table) = schema.tables.get_key_value(reference) {
        return Some(table);
    }

    let mut matches = schema
        .tables
        .iter()
        .filter(|(_, table)| table.name == reference);
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

fn target_has_unique_key_for_columns(target: &Table, columns: &[String]) -> bool {
    if target
        .primary_key
        .as_ref()
        .is_some_and(|pk| pk.columns == columns)
    {
        return true;
    }
    if target.constraints.iter().any(|constraint| {
        matches!(constraint, Constraint::Unique { columns: unique_columns, .. } if unique_columns == columns)
    }) {
        return true;
    }
    target
        .indexes
        .iter()
        .any(|index| index.unique && index.predicate.is_none() && index.columns == columns)
}

fn validate_table_primary_key(
    table_name: &str,
    table: &Table,
) -> Result<(), SchemaValidationError> {
    let flagged: Vec<&str> = table
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.as_str())
        .collect();

    let Some(pk) = &table.primary_key else {
        return Ok(());
    };

    if pk.name.is_empty() {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' has a primary key with an empty name"
        )));
    }
    if pk.columns.is_empty() {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' has a primary key with no columns"
        )));
    }

    let mut pk_seen = HashSet::new();
    for column in &pk.columns {
        if !pk_seen.insert(column.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key '{}' repeats column '{column}'",
                pk.name
            )));
        }
        if !table
            .columns
            .iter()
            .any(|candidate| candidate.name == *column)
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key '{}' references unknown column '{column}'",
                pk.name
            )));
        }
    }

    if !flagged.is_empty()
        && !same_str_set(
            &flagged,
            &pk.columns.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' primary key column flags conflict with explicit primary_key '{}'",
            pk.name
        )));
    }

    for column in table.primary_key_columns() {
        if column.nullable {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key column '{}' must be non-null",
                column.name
            )));
        }
    }

    Ok(())
}

fn same_str_set(left: &[&str], right: &[&str]) -> bool {
    let left: HashSet<&str> = left.iter().copied().collect();
    let right: HashSet<&str> = right.iter().copied().collect();
    left == right
}
