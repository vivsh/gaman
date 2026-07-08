use std::collections::HashSet;

use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{
    Column, Constraint, ForeignKey, FunctionDef, Index, Schema, SchemaValidationError, Table,
    TriggerDef, TriggerScope, ViewDef, Volatility,
};

use super::{DialectError, DialectProcessor};

mod data_types;
mod extension_types;

pub(super) static POSTGRES: PostgresProcessor = PostgresProcessor;

pub(super) struct PostgresProcessor;

impl DialectProcessor for PostgresProcessor {
    fn migration_to_sql(
        &self,
        migration: &Migration,
        _start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        validate_migration(migration)?;
        migration
            .operations
            .iter()
            .map(operation_to_sql)
            .collect::<Result<Vec<_>, _>>()
            .map(|chunks| chunks.into_iter().flatten().collect())
    }

    fn finalize_diff_operations(
        &self,
        ops: Vec<Operation>,
        previous: &Schema,
        current: &Schema,
    ) -> Vec<Operation> {
        reorder_ops(ops, previous, current)
    }

    fn should_merge(&self, _table_name: &str, _op: &Operation) -> bool {
        true
    }

    fn canonicalize_schema_name(&self, object: EntityKind, schema: Option<&str>) -> Option<String> {
        match (object, schema) {
            (_, None) => None,
            (EntityKind::Extension, Some("public" | "pg_catalog")) => None,
            (_, Some("public")) => None,
            (_, Some(value)) => Some(value.to_string()),
        }
    }

    fn normalize_type<'a>(&self, t: &'a str) -> &'a str {
        normalize_type(t)
    }

    fn canonical_type(&self, t: &str) -> String {
        canonical_type(t)
    }

    fn is_catalog_type(&self, t: &str) -> bool {
        is_catalog_type(t)
    }

    fn type_suggestions(&self, t: &str) -> Vec<String> {
        type_suggestions(t)
    }

    fn validate_schema(&self, schema: &Schema) -> Result<(), SchemaValidationError> {
        validate_schema(schema)
    }

    fn validate_migration(&self, migration: &Migration) -> Result<(), DialectError> {
        validate_migration(migration)
    }

    fn validate_migration_with_state(
        &self,
        migration: &Migration,
        _start: &Schema,
    ) -> Result<(), DialectError> {
        validate_migration(migration)
    }
}

// PostgreSQL-specific constraint: DROP FUNCTION fails when dependent triggers still exist.
// This arises when a function's argument signature changes, because the old signature cannot
// be replaced in-place — it must be dropped and recreated. Any trigger referencing that
// function must be bounced (dropped before, recreated after) regardless of whether the
// trigger itself changed. This is transparent to the generic diff algorithm.
pub fn reorder_ops(ops: Vec<Operation>, previous: &Schema, current: &Schema) -> Vec<Operation> {
    let sig_changed_fns: HashSet<&str> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::AlterFunction { old, new } if old.arguments != new.arguments => {
                Some(new.name.as_str())
            }
            _ => None,
        })
        .collect();

    if sig_changed_fns.is_empty() {
        return ops;
    }

    let dropped_tables: HashSet<&str> = ops
        .iter()
        .filter_map(|op| match op {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        })
        .collect();

    let mut pre_fn_drops: Vec<Operation> = Vec::new();
    let mut post_fn_creates: Vec<Operation> = Vec::new();
    let mut bounced_keys: HashSet<String> = HashSet::new();

    for (table_name, table) in &previous.tables {
        if dropped_tables.contains(table_name.as_str()) {
            continue;
        }
        for trigger in &table.triggers {
            if trigger
                .function_name
                .as_deref()
                .is_some_and(|f| sig_changed_fns.contains(f))
            {
                if let Some(n) = &trigger.name {
                    bounced_keys.insert(format!("{}:{}", table_name, n));
                }
                pre_fn_drops.push(Operation::DropTrigger {
                    table_name: table_name.clone(),
                    trigger: trigger.clone(),
                });
            }
        }
    }
    for (table_name, table) in &current.tables {
        for trigger in &table.triggers {
            if trigger
                .function_name
                .as_deref()
                .is_some_and(|f| sig_changed_fns.contains(f))
            {
                if let Some(n) = &trigger.name {
                    bounced_keys.insert(format!("{}:{}", table_name, n));
                }
                post_fn_creates.push(Operation::CreateTrigger {
                    table_name: table_name.clone(),
                    trigger: trigger.clone(),
                });
            }
        }
    }

    let trigger_key = |op: &Operation| -> Option<String> {
        match op {
            Operation::DropTrigger {
                table_name,
                trigger,
            } => trigger
                .name
                .as_deref()
                .map(|n| format!("{}:{}", table_name, n)),
            Operation::CreateTrigger {
                table_name,
                trigger,
            } => trigger
                .name
                .as_deref()
                .map(|n| format!("{}:{}", table_name, n)),
            Operation::AlterTrigger {
                table_name, old, ..
            } => old.name.as_deref().map(|n| format!("{}:{}", table_name, n)),
            _ => None,
        }
    };

    // Find where the first AlterFunction (sig change) sits — pre_fn_drops go just before it,
    // post_fn_creates go just after the last one.
    let first_alter = ops.iter().position(
        |op| matches!(op, Operation::AlterFunction { old, new } if old.arguments != new.arguments),
    );
    let last_alter = ops.iter().rposition(
        |op| matches!(op, Operation::AlterFunction { old, new } if old.arguments != new.arguments),
    );

    let (first_alter, last_alter) = match (first_alter, last_alter) {
        (Some(f), Some(l)) => (f, l),
        _ => return ops,
    };

    let mut result = Vec::with_capacity(ops.len() + pre_fn_drops.len() + post_fn_creates.len());
    for (i, op) in ops.into_iter().enumerate() {
        if i == first_alter {
            result.append(&mut pre_fn_drops);
        }
        if let Some(key) = trigger_key(&op)
            && bounced_keys.contains(&key)
        {
            continue; // already handled via pre/post
        }
        result.push(op);
        if i == last_alter {
            result.append(&mut post_fn_creates);
        }
    }
    result
}

pub fn normalize_type(t: &str) -> &str {
    data_types::normalize_type(t)
}

pub fn canonical_type(t: &str) -> String {
    if extension_types::is_extension_type(t) {
        t.to_string()
    } else {
        data_types::canonical_type(t)
    }
}

pub fn validate_schema(_schema: &Schema) -> Result<(), SchemaValidationError> {
    Ok(())
}

pub fn is_catalog_type(t: &str) -> bool {
    data_types::canonical_known_type(t).is_some() || extension_types::is_extension_type(t)
}

pub fn type_suggestions(t: &str) -> Vec<String> {
    type_suggestions_from_catalogs(
        t,
        data_types::known_type_names().chain(extension_types::extension_type_names()),
    )
}

fn type_suggestions_from_catalogs(
    t: &str,
    names: impl Iterator<Item = &'static str>,
) -> Vec<String> {
    let needle = data_types::normalize_type_text(t);
    let max_distance = if needle.len() <= 5 { 1 } else { 2 };
    let mut suggestions = names
        .filter(|name| edit_distance(&needle, name) <= max_distance)
        .map(str::to_string)
        .collect::<Vec<_>>();
    suggestions.sort();
    suggestions.dedup();
    suggestions
}

fn edit_distance(left: &str, right: &str) -> usize {
    let mut previous = (0..=right.len()).collect::<Vec<_>>();
    let mut current = vec![0; right.len() + 1];
    for (i, left_byte) in left.bytes().enumerate() {
        current[0] = i + 1;
        for (j, right_byte) in right.bytes().enumerate() {
            let substitution = previous[j] + usize::from(left_byte != right_byte);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current[j + 1] = substitution.min(insertion).min(deletion);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[right.len()]
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn quote_literal(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn quote_table_name(name: &str) -> String {
    if let Some((schema, table)) = name.split_once('.') {
        format!("{}.{}", quote_ident(schema), quote_ident(table))
    } else {
        quote_ident(name)
    }
}

fn quote_maybe_qualified_name(name: &str) -> String {
    quote_table_name(name)
}

fn bare_name(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(_, bare)| bare)
}

fn qualified_name(name: &str, schema: Option<&str>) -> String {
    match schema {
        None | Some("public") => quote_ident(name),
        Some(schema) => format!("{}.{}", quote_ident(schema), quote_ident(name)),
    }
}

fn qualified_table(table: &Table) -> String {
    qualified_name(&table.name, table.schema.as_deref())
}

fn qualified_view(view: &ViewDef) -> String {
    qualified_name(&view.name, view.schema.as_deref())
}

fn qualified_function(function: &crate::states::FunctionDef) -> String {
    qualified_name(&function.name, function.schema.as_deref())
}

fn quoted_columns(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ")
}

fn values_are_subsequence(old: &[String], new: &[String]) -> bool {
    let mut new_iter = new.iter();
    old.iter()
        .all(|old_value| new_iter.by_ref().any(|new_value| new_value == old_value))
}

fn add_enum_value_statements(old: &[String], new: &[String], type_name: &str) -> Vec<String> {
    let mut current = old.to_vec();
    let mut statements = Vec::new();

    for (target_index, value) in new.iter().enumerate() {
        if current.contains(value) {
            continue;
        }

        let previous = new[..target_index]
            .iter()
            .rev()
            .find(|candidate| current.contains(candidate));
        let clause = if let Some(previous) = previous {
            let current_index = current
                .iter()
                .position(|candidate| candidate == previous)
                .expect("previous enum value exists");
            current.insert(current_index + 1, value.clone());
            format!(" AFTER {}", quote_literal(previous))
        } else if let Some(next) = new[target_index + 1..]
            .iter()
            .find(|candidate| current.contains(candidate))
        {
            let current_index = current
                .iter()
                .position(|candidate| candidate == next)
                .expect("next enum value exists");
            current.insert(current_index, value.clone());
            format!(" BEFORE {}", quote_literal(next))
        } else {
            current.push(value.clone());
            String::new()
        };

        statements.push(format!(
            "ALTER TYPE {type_name} ADD VALUE {}{}",
            quote_literal(value),
            clause,
        ));
    }

    statements
}

fn foreign_key_clause(foreign_key: &ForeignKey) -> Result<String, DialectError> {
    let mut clause = format!(
        "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        quote_ident(&foreign_key.name),
        quoted_columns(&foreign_key.columns),
        quote_table_name(&foreign_key.to_table),
        quoted_columns(&foreign_key.to_columns),
    );
    if let Some(action) = foreign_key.on_delete.as_deref() {
        clause.push_str(" ON DELETE ");
        clause.push_str(delete_action_sql(action)?);
    }
    Ok(clause)
}

fn delete_action_sql(action: &str) -> Result<&'static str, DialectError> {
    match action {
        "cascade" => Ok("CASCADE"),
        "restrict" => Ok("RESTRICT"),
        "set_null" => Ok("SET NULL"),
        "set_default" => Ok("SET DEFAULT"),
        other => Err(DialectError::Unsupported(
            "foreign_key".to_string(),
            format!("unsupported on_delete action '{other}'"),
        )),
    }
}

fn create_index_sql(index: &Index, table_name: &str, concurrent: bool) -> String {
    let unique = if index.unique { "UNIQUE " } else { "" };
    let concurrent = if concurrent { "CONCURRENTLY " } else { "" };
    let predicate = index
        .predicate
        .as_ref()
        .map(|p| format!(" WHERE {p}"))
        .unwrap_or_default();
    format!(
        "CREATE {}INDEX {}{} ON {} ({}){}",
        unique,
        concurrent,
        quote_ident(&index.name),
        quote_table_name(table_name),
        quoted_columns(&index.columns),
        predicate,
    )
}

fn drop_index_sql(table_name: &str, index_name: &str, concurrent: bool) -> String {
    let concurrent = if concurrent { " CONCURRENTLY" } else { "" };
    let index = if let Some((schema, _)) = table_name.split_once('.') {
        qualified_name(index_name, Some(schema))
    } else {
        quote_ident(index_name)
    };
    format!("DROP INDEX{} {}", concurrent, index)
}

fn operation_to_sql(op: &Operation) -> Result<Vec<String>, DialectError> {
    let stmts = match op {
        Operation::CreateTable { table } => {
            let render_inline_pk = table.primary_key.is_none();
            let mut parts: Vec<String> = table
                .columns
                .iter()
                .map(|column| col_def_with_primary_key(column, render_inline_pk))
                .collect();
            if let Some(pk) = &table.primary_key {
                parts.push(primary_key_def(&pk.name, &pk.columns));
            }
            for fk in &table.foreign_keys {
                parts.push(foreign_key_clause(fk)?);
            }
            for c in &table.constraints {
                parts.push(format!("CONSTRAINT {}", inline_constraint_def(c)));
            }
            let mut stmts = vec![format!(
                "CREATE TABLE {} ({})",
                qualified_table(table),
                parts.join(", ")
            )];
            let table_name = table.qualified_name();
            for index in &table.indexes {
                stmts.push(create_index_sql(index, &table_name, false));
            }
            stmts
        }
        Operation::DropTable { table } => {
            vec![format!("DROP TABLE {}", qualified_table(table))]
        }
        Operation::RenameTable { old_name, new_name } => {
            vec![format!(
                "ALTER TABLE {} RENAME TO {}",
                quote_table_name(old_name),
                quote_ident(new_name)
            )]
        }
        Operation::AddColumn { table_name, column } => {
            vec![format!(
                "ALTER TABLE {} ADD COLUMN {}",
                quote_table_name(table_name),
                col_def(column)
            )]
        }
        Operation::DropColumn {
            table_name,
            column,
            cascade,
        } => {
            let suffix = if *cascade { " CASCADE" } else { "" };
            vec![format!(
                "ALTER TABLE {} DROP COLUMN {}{}",
                quote_table_name(table_name),
                quote_ident(&column.name),
                suffix
            )]
        }
        Operation::RenameColumn {
            table_name,
            old_name,
            new_name,
        } => {
            vec![format!(
                "ALTER TABLE {} RENAME COLUMN {} TO {}",
                quote_table_name(table_name),
                quote_ident(old_name),
                quote_ident(new_name)
            )]
        }
        Operation::AlterColumn {
            table_name,
            old,
            new,
            cast_expr,
        } => alter_column_statements(table_name, old, new, cast_expr.as_deref()),
        Operation::AddForeignKey {
            table_name,
            foreign_key,
        } => {
            vec![format!(
                "ALTER TABLE {} ADD {}",
                quote_table_name(table_name),
                foreign_key_clause(foreign_key)?
            )]
        }
        Operation::DropForeignKey {
            table_name,
            foreign_key,
            cascade,
        } => {
            let suffix = if *cascade { " CASCADE" } else { "" };
            vec![format!(
                "ALTER TABLE {} DROP CONSTRAINT {}{}",
                quote_table_name(table_name),
                quote_ident(&foreign_key.name),
                suffix
            )]
        }
        Operation::AddIndex {
            table_name,
            index,
            concurrent,
        } => {
            vec![create_index_sql(index, table_name, *concurrent)]
        }
        Operation::DropIndex {
            table_name,
            index,
            concurrent,
        } => {
            vec![drop_index_sql(table_name, &index.name, *concurrent)]
        }
        Operation::AddConstraint {
            table_name,
            constraint,
        } => {
            vec![format!(
                "ALTER TABLE {} ADD CONSTRAINT {}",
                quote_table_name(table_name),
                inline_constraint_def(constraint)
            )]
        }
        Operation::DropConstraint {
            table_name,
            constraint,
        } => {
            vec![format!(
                "ALTER TABLE {} DROP CONSTRAINT {}",
                quote_table_name(table_name),
                quote_ident(constraint.name())
            )]
        }
        Operation::Statement { up, .. } => {
            vec![up.clone()]
        }
        Operation::CreateFunction { function } => {
            vec![create_function_sql(function)?]
        }
        Operation::DropFunction { function } => {
            vec![format!(
                "DROP FUNCTION {}({})",
                qualified_function(function),
                function.arguments
            )]
        }
        Operation::AlterFunction { old, new } => {
            if old.arguments == new.arguments {
                vec![create_function_sql(new)?]
            } else {
                vec![
                    format!(
                        "DROP FUNCTION {}({})",
                        qualified_function(old),
                        old.arguments
                    ),
                    create_function_sql(new)?,
                ]
            }
        }
        Operation::CreateTrigger {
            table_name,
            trigger,
        } => create_trigger_statements(table_name, trigger)?,
        Operation::AlterTrigger {
            table_name,
            old,
            new,
            ..
        } => {
            let mut statements = create_trigger_statements(table_name, new)?;
            if old.query.is_some() && new.query.is_none() {
                statements.push(drop_generated_trigger_function_sql(old));
            }
            statements
        }
        Operation::DropTrigger {
            table_name,
            trigger,
        } => {
            let tname = trigger.name.as_deref().unwrap_or("");
            let mut statements = vec![format!(
                "DROP TRIGGER {} ON {}",
                quote_ident(tname),
                quote_table_name(table_name)
            )];
            if trigger.query.is_some() {
                statements.push(drop_generated_trigger_function_sql(trigger));
            }
            statements
        }
        Operation::CreateView { view } => {
            vec![format!(
                "CREATE OR REPLACE VIEW {} AS {}",
                qualified_view(view),
                view.definition
            )]
        }
        Operation::DropView { view } => {
            vec![format!("DROP VIEW {}", qualified_view(view))]
        }
        Operation::ReplaceView { new, .. } => {
            vec![format!(
                "CREATE OR REPLACE VIEW {} AS {}",
                qualified_view(new),
                new.definition
            )]
        }
        Operation::CreateExtension { extension } => {
            let mut sql = format!(
                "CREATE EXTENSION IF NOT EXISTS {}",
                quote_ident(&extension.name)
            );
            if let Some(schema) = &extension.schema {
                sql.push_str(&format!(" SCHEMA {}", quote_ident(schema)));
            }
            if let Some(version) = &extension.version {
                let v = version.replace('\'', "''");
                sql.push_str(&format!(" VERSION '{v}'"));
            }
            vec![sql]
        }
        Operation::DropExtension { extension } => {
            vec![format!("DROP EXTENSION {}", quote_ident(&extension.name))]
        }
        Operation::CreateEnum { enum_def } => {
            let values: Vec<String> = enum_def.values.iter().map(|v| quote_literal(v)).collect();
            let name = qualified_name(&enum_def.name, enum_def.schema.as_deref());
            vec![format!(
                "CREATE TYPE {name} AS ENUM ({})",
                values.join(", ")
            )]
        }
        Operation::DropEnum { enum_def } => {
            let name = qualified_name(&enum_def.name, enum_def.schema.as_deref());
            vec![format!("DROP TYPE {name}")]
        }
        Operation::RenameEnumValue {
            enum_name,
            schema,
            old_value,
            new_value,
        } => {
            let name = qualified_name(enum_name, schema.as_deref());
            vec![format!(
                "ALTER TYPE {name} RENAME VALUE {} TO {}",
                quote_literal(old_value),
                quote_literal(new_value),
            )]
        }
        Operation::AlterEnum { old, new } => {
            if old.name != new.name || old.schema != new.schema {
                return Err(super::DialectError::Unsupported(
                    "alter_enum".into(),
                    "PostgreSQL enum type renames are not modeled; use raw SQL".into(),
                ));
            }
            if old.values.len() <= new.values.len()
                && values_are_subsequence(&old.values, &new.values)
            {
                let name = qualified_name(&new.name, new.schema.as_deref());
                add_enum_value_statements(&old.values, &new.values, &name)
            } else if old.values.len() == new.values.len() {
                let removed: Vec<&String> = old
                    .values
                    .iter()
                    .filter(|v| !new.values.contains(v))
                    .collect();
                let added: Vec<&String> = new
                    .values
                    .iter()
                    .filter(|v| !old.values.contains(v))
                    .collect();
                if removed.len() == 1 && added.len() == 1 {
                    let name = qualified_name(&new.name, new.schema.as_deref());
                    vec![format!(
                        "ALTER TYPE {name} RENAME VALUE {} TO {}",
                        quote_literal(removed[0]),
                        quote_literal(added[0]),
                    )]
                } else {
                    return Err(super::DialectError::Unsupported(
                        "alter_enum".into(),
                        "PostgreSQL enum alterations support append-only additions or one-for-one value renames".into(),
                    ));
                }
            } else {
                return Err(super::DialectError::Unsupported(
                    "alter_enum".into(),
                    "PostgreSQL cannot remove enum values; use a rename decision or raw SQL".into(),
                ));
            }
        }
    };
    Ok(stmts)
}

pub fn validate_migration(m: &Migration) -> Result<(), super::DialectError> {
    if m.atomic {
        for op in &m.operations {
            match op {
                Operation::AddIndex {
                    concurrent: true, ..
                } => {
                    return Err(super::DialectError::Unsupported(
                        "add_index".into(),
                        "CONCURRENTLY requires atomic = false on the migration".into(),
                    ));
                }
                Operation::DropIndex {
                    concurrent: true, ..
                } => {
                    return Err(super::DialectError::Unsupported(
                        "drop_index".into(),
                        "CONCURRENTLY requires atomic = false on the migration".into(),
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

pub fn col_def(c: &Column) -> String {
    col_def_with_primary_key(c, true)
}

fn col_def_with_primary_key(c: &Column, render_primary_key: bool) -> String {
    let mut s = format!("{} {}", quote_ident(&c.name), c.col_type);
    if let Some(ref expr) = c.generated {
        s.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
        return s;
    }
    if render_primary_key && c.primary_key {
        s.push_str(" PRIMARY KEY");
    } else if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(ref default) = c.default {
        s.push_str(&format!(" DEFAULT {default}"));
    }
    s
}

fn primary_key_def(name: &str, columns: &[String]) -> String {
    let cols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
    format!(
        "CONSTRAINT {} PRIMARY KEY ({})",
        quote_ident(name),
        cols.join(", ")
    )
}

fn alter_column_statements(
    table: &str,
    old: &Column,
    new: &Column,
    cast_expr: Option<&str>,
) -> Vec<String> {
    let mut stmts = Vec::new();

    if old.col_type != new.col_type {
        let using = cast_expr.map(|e| format!(" USING {e}")).unwrap_or_default();
        stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {}{}",
            quote_table_name(table),
            quote_ident(&new.name),
            new.col_type,
            using,
        ));
    }

    match (old.nullable, new.nullable) {
        (false, true) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL",
            quote_table_name(table),
            quote_ident(&new.name)
        )),
        (true, false) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL",
            quote_table_name(table),
            quote_ident(&new.name)
        )),
        _ => {}
    }

    match (&old.default, &new.default) {
        (_, Some(d)) if old.default.as_deref() != Some(d.as_str()) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {d}",
            quote_table_name(table),
            quote_ident(&new.name)
        )),
        (Some(_), None) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT",
            quote_table_name(table),
            quote_ident(&new.name)
        )),
        _ => {}
    }

    match (old.primary_key, new.primary_key) {
        (false, true) => stmts.push(format!(
            "ALTER TABLE {} ADD PRIMARY KEY ({})",
            quote_table_name(table),
            quote_ident(&new.name)
        )),
        (true, false) => stmts.push(format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            quote_table_name(table),
            quote_ident(&Table::pk_constraint_name_for(bare_name(table)))
        )),
        _ => {}
    }

    stmts
}

fn inline_constraint_def(c: &Constraint) -> String {
    match c {
        Constraint::Unique { name, columns } => {
            let cols: Vec<String> = columns.iter().map(|c| quote_ident(c)).collect();
            format!("{} UNIQUE ({})", quote_ident(name), cols.join(", "))
        }
        Constraint::Check { name, expression } => {
            format!("{} CHECK ({})", quote_ident(name), expression)
        }
    }
}

fn create_function_sql(f: &crate::states::FunctionDef) -> Result<String, DialectError> {
    // PostgreSQL requires trigger functions to be VOLATILE; STABLE/IMMUTABLE are rejected.
    // Trigger functions also cannot have declared arguments (use TG_ARGV instead).
    let is_trigger = f.returns.eq_ignore_ascii_case("trigger");
    if is_trigger && !f.arguments.trim().is_empty() {
        return Err(DialectError::Unsupported(
            f.name.clone(),
            "trigger functions cannot have declared arguments (use TG_NARGS/TG_ARGV instead)"
                .into(),
        ));
    }
    let vol = if is_trigger {
        ""
    } else {
        match f.volatility {
            Volatility::Volatile => "",
            Volatility::Stable => "\nSTABLE",
            Volatility::Immutable => "\nIMMUTABLE",
        }
    };
    let sec = if f.security_definer {
        "\nSECURITY DEFINER"
    } else {
        ""
    };
    Ok(format!(
        "CREATE OR REPLACE FUNCTION {}({})\nRETURNS {}\nLANGUAGE {}{}{}
AS $func$\n{}\n$func$",
        qualified_function(f),
        f.arguments,
        f.returns,
        f.language,
        vol,
        sec,
        f.body
    ))
}

fn create_trigger_statements(
    table_name: &str,
    trigger: &TriggerDef,
) -> Result<Vec<String>, DialectError> {
    if let Some(query) = trigger.query.as_deref() {
        let function = generated_trigger_function(trigger, query)?;
        return Ok(vec![
            create_function_sql(&function)?,
            create_trigger_sql(table_name, trigger),
        ]);
    }
    Ok(vec![create_trigger_sql(table_name, trigger)])
}

fn generated_trigger_function(
    trigger: &TriggerDef,
    query: &str,
) -> Result<FunctionDef, DialectError> {
    let trigger_name = trigger.name.as_deref().ok_or_else(|| {
        DialectError::Unsupported(
            "create_trigger".to_string(),
            "trigger query rendering requires a trigger name; normalize schema before rendering"
                .to_string(),
        )
    })?;
    let return_stmt = match trigger.scope {
        TriggerScope::Row => "RETURN NEW;",
        TriggerScope::Statement => "RETURN NULL;",
    };
    let body = format!("BEGIN\n{}\n{}\nEND;", query.trim(), return_stmt);
    Ok(FunctionDef {
        name: format!("{trigger_name}_fn"),
        schema: None,
        arguments: String::new(),
        returns: "trigger".to_string(),
        language: trigger
            .language
            .clone()
            .unwrap_or_else(|| "plpgsql".to_string()),
        body,
        volatility: Volatility::Volatile,
        security_definer: false,
    })
}

fn drop_generated_trigger_function_sql(trigger: &TriggerDef) -> String {
    let name = trigger
        .name
        .as_ref()
        .map(|trigger_name| format!("{trigger_name}_fn"))
        .unwrap_or_default();
    format!("DROP FUNCTION {}()", quote_ident(&name))
}

fn trigger_function_name(t: &TriggerDef) -> String {
    t.function_name
        .clone()
        .or_else(|| t.name.as_ref().map(|name| format!("{name}_fn")))
        .unwrap_or_default()
}

fn create_trigger_sql(table_name: &str, t: &TriggerDef) -> String {
    use crate::states::{TriggerEvent, TriggerScope, TriggerTiming};
    let tname = t.name.as_deref().unwrap_or("");
    let timing = match t.timing {
        TriggerTiming::Before => "BEFORE",
        TriggerTiming::After => "AFTER",
        TriggerTiming::InsteadOf => "INSTEAD OF",
    };
    let mut events: Vec<&str> = t
        .events
        .iter()
        .map(|e| match e {
            TriggerEvent::Insert => "INSERT",
            TriggerEvent::Update => "UPDATE",
            TriggerEvent::Delete => "DELETE",
            TriggerEvent::Truncate => "TRUNCATE",
        })
        .collect();
    events.sort_unstable();
    let scope = match t.scope {
        TriggerScope::Row => "ROW",
        TriggerScope::Statement => "STATEMENT",
    };
    let when_clause = t
        .when
        .as_deref()
        .map(|w| format!("\nWHEN ({})", w))
        .unwrap_or_default();
    let fn_name = trigger_function_name(t);
    format!(
        "CREATE OR REPLACE TRIGGER {}\n{} {}\nON {}\nFOR EACH {}{}
EXECUTE FUNCTION {}()",
        quote_ident(tname),
        timing,
        events.join(" OR "),
        quote_table_name(table_name),
        scope,
        when_clause,
        quote_maybe_qualified_name(&fn_name)
    )
}

#[cfg(test)]
mod tests;
