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
        validate_function_identities(self)?;
        self.canonicalize(dialect);
        resolve_function_dependencies(self)?;
        self.validate_checked()?;
        dialect.validate_schema(self)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_checked().map_err(|err| err.to_string())
    }

    pub fn validate_checked(&self) -> Result<(), SchemaValidationError> {
        crate::managed_rows::validate_schema(self)?;
        validate_function_parameters(self)?;
        for table in self.tables.values() {
            for column in &table.columns {
                if column.dialect_options.mysql.is_some()
                    && column.dialect_options.mariadb.is_some()
                {
                    return Err(SchemaValidationError::Invalid(format!(
                        "column '{}.{}' cannot define both mysql and mariadb options",
                        table.name, column.name
                    )));
                }
            }
        }
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

fn resolve_function_dependencies(schema: &mut Schema) -> Result<(), SchemaValidationError> {
    let functions = schema
        .functions
        .iter()
        .map(|(key, function)| (key.clone(), function.qualified_name()))
        .collect::<Vec<_>>();
    let tables = schema.tables.keys().cloned().collect::<Vec<_>>();
    let views = schema.views.keys().cloned().collect::<Vec<_>>();
    let enums = schema.enums.keys().cloned().collect::<Vec<_>>();
    let extensions = schema.extensions.keys().cloned().collect::<Vec<_>>();
    let sequences = schema.sequences.keys().cloned().collect::<Vec<_>>();
    for function in schema.functions.values_mut() {
        for dependency in &mut function.depends_on {
            let target = match dependency.kind {
                EntityKind::Function => resolve_function_dependency(&dependency.target, &functions)?,
                EntityKind::Table => resolve_root_dependency(&dependency.target, &tables)?,
                EntityKind::View => resolve_root_dependency(&dependency.target, &views)?,
                EntityKind::Enum => resolve_root_dependency(&dependency.target, &enums)?,
                EntityKind::Extension => resolve_root_dependency(&dependency.target, &extensions)?,
                EntityKind::Sequence => resolve_root_dependency(&dependency.target, &sequences)?,
                _ => return Err(SchemaValidationError::Invalid("function dependencies must target root entities".to_string())),
            };
            dependency.target = target;
        }
        function.depends_on.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| left.target.cmp(&right.target))
        });
        function.depends_on.dedup();
    }
    validate_function_dependency_cycles(schema)?;
    Ok(())
}

fn validate_function_identities(schema: &Schema) -> Result<(), SchemaValidationError> {
    let mut identities = HashSet::new();
    for function in schema.functions.values() {
        let identity = function.identity_key();
        if !identities.insert(identity.clone()) {
            return Err(SchemaValidationError::Invalid(format!(
                "duplicate function identity '{identity}'"
            )));
        }
    }
    Ok(())
}

fn validate_function_dependency_cycles(schema: &Schema) -> Result<(), SchemaValidationError> {
    let mut visiting = HashSet::new();
    let mut visited = HashSet::new();
    let mut path = Vec::new();
    for key in schema.functions.keys() {
        visit_function_dependencies(schema, key, &mut visiting, &mut visited, &mut path)?;
    }
    Ok(())
}

fn visit_function_dependencies(
    schema: &Schema,
    key: &str,
    visiting: &mut HashSet<String>,
    visited: &mut HashSet<String>,
    path: &mut Vec<String>,
) -> Result<(), SchemaValidationError> {
    if visited.contains(key) {
        return Ok(());
    }
    if !visiting.insert(key.to_string()) {
        let start = path.iter().position(|entry| entry == key).unwrap_or(0);
        let mut cycle = path[start..].to_vec();
        cycle.push(key.to_string());
        return Err(SchemaValidationError::Invalid(format!(
            "function dependency cycle: {}",
            cycle.join(" -> ")
        )));
    }
    path.push(key.to_string());
    let function = schema.functions.get(key).ok_or_else(|| {
        SchemaValidationError::Invalid(format!("function dependency target '{key}' disappeared"))
    })?;
    for dependency in &function.depends_on {
        if dependency.kind == EntityKind::Function {
            visit_function_dependencies(schema, &dependency.target, visiting, visited, path)?;
        }
    }
    path.pop();
    visiting.remove(key);
    visited.insert(key.to_string());
    Ok(())
}

fn resolve_function_dependency(target: &str, functions: &[(String, String)]) -> Result<String, SchemaValidationError> {
    let matches = if target.contains('(') {
        functions.iter().filter(|(key, _)| key == target || (target.ends_with("()") && key == target.trim_end_matches("()"))).map(|(key, _)| key.clone()).collect::<Vec<_>>()
    } else {
        functions.iter().filter(|(_, name)| name == target || (!name.contains('.') && target == format!("public.{name}"))).map(|(key, _)| key.clone()).collect::<Vec<_>>()
    };
    match matches.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(SchemaValidationError::Invalid(format!("function dependency '{target}' does not resolve"))),
        _ => Err(SchemaValidationError::Invalid(format!("function dependency '{target}' is ambiguous; use one of: {}", matches.join(", ")))),
    }
}

fn resolve_root_dependency(target: &str, keys: &[String]) -> Result<String, SchemaValidationError> {
    let matches = keys.iter().filter(|key| key.as_str() == target || (!key.contains('.') && target == format!("public.{key}"))).cloned().collect::<Vec<_>>();
    match matches.as_slice() {
        [only] => Ok(only.clone()),
        [] => Err(SchemaValidationError::Invalid(format!("dependency '{target}' does not resolve"))),
        _ => Err(SchemaValidationError::Invalid(format!("dependency '{target}' is ambiguous"))),
    }
}

fn validate_function_parameters(schema: &Schema) -> Result<(), SchemaValidationError> {
    for function in schema.functions.values() {
        if !function.arguments.trim().is_empty() && !function.parameters.is_empty() {
            return Err(SchemaValidationError::Invalid(format!(
                "function '{}' cannot specify both legacy arguments and typed parameters",
                function.qualified_name()
            )));
        }
        let mut default_seen = false;
        let mut names = HashSet::new();
        for parameter in &function.parameters {
            if parameter.type_name.trim().is_empty() {
                return Err(SchemaValidationError::Invalid(format!("function '{}' has a parameter without a type", function.qualified_name())));
            }
            if !parameter.name.is_empty() && !names.insert(parameter.name.as_str()) {
                return Err(SchemaValidationError::Invalid(format!("function '{}' repeats parameter '{}'", function.qualified_name(), parameter.name)));
            }
            if parameter.default.is_some() {
                default_seen = true;
            } else if default_seen {
                return Err(SchemaValidationError::Invalid(format!("function '{}' has a non-default parameter after a default parameter", function.qualified_name())));
            }
        }
    }
    Ok(())
}

/// Rejects vendor column blocks for dialects that do not own them.
pub(crate) fn reject_family_column_options(
    schema: &Schema,
    dialect: &str,
) -> Result<(), SchemaValidationError> {
    for table in schema.tables.values() {
        if let Some(column) = table.columns.iter().find(|column| {
            column.dialect_options.mysql.is_some() || column.dialect_options.mariadb.is_some()
        }) {
            return Err(SchemaValidationError::Invalid(format!(
                "{dialect} column '{}.{}' cannot use mysql or mariadb options",
                table.name, column.name
            )));
        }
    }
    Ok(())
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
