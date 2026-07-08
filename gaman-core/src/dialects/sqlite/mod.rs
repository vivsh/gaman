use crate::dialects::DialectError;
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{
    Column, Constraint, Index, Schema, SchemaValidationError, Table, TriggerDef, TriggerEvent,
    TriggerScope, TriggerTiming, ViewDef,
};

use super::DialectProcessor;

mod data_types;
mod extension_types;

pub(super) static SQLITE: SqliteProcessor = SqliteProcessor;

pub(super) struct SqliteProcessor;

impl DialectProcessor for SqliteProcessor {
    fn migration_to_sql(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        migration_to_sql(migration, start)
    }

    fn finalize_diff_operations(
        &self,
        ops: Vec<Operation>,
        _previous: &Schema,
        _current: &Schema,
    ) -> Vec<Operation> {
        ops
    }

    fn should_merge(&self, _table_name: &str, op: &Operation) -> bool {
        matches!(
            op,
            Operation::AddForeignKey { .. } | Operation::AddConstraint { .. }
        )
    }

    fn canonicalize_schema_name(
        &self,
        _object: EntityKind,
        schema: Option<&str>,
    ) -> Option<String> {
        schema.map(str::to_string)
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
        start: &Schema,
    ) -> Result<(), DialectError> {
        migration_to_sql(migration, start).map(|_| ())
    }
}

fn unsupported(op: &str, reason: impl Into<String>) -> DialectError {
    DialectError::Unsupported(op.to_string(), reason.into())
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn quote_table_name(name: &str) -> Result<String, DialectError> {
    if name.contains('.') {
        Err(unsupported(
            name,
            "SQLite dialect does not support schema-qualified table names",
        ))
    } else {
        Ok(quote_ident(name))
    }
}

fn qualified_table(table: &Table) -> Result<String, DialectError> {
    if table.schema.is_some() {
        return Err(unsupported(
            &table.name,
            "SQLite dialect does not support schemas",
        ));
    }
    quote_table_name(&table.name)
}

fn qualified_view(view: &ViewDef) -> Result<String, DialectError> {
    if view.schema.is_some() {
        return Err(unsupported(
            &view.name,
            "SQLite dialect does not support schemas",
        ));
    }
    quote_table_name(&view.name)
}

fn quoted_columns(columns: &[String]) -> String {
    columns
        .iter()
        .map(|column| quote_ident(column))
        .collect::<Vec<_>>()
        .join(", ")
}

fn inline_constraint_def(c: &Constraint) -> String {
    match c {
        Constraint::Unique { name, columns } => {
            format!(
                "CONSTRAINT {} UNIQUE ({})",
                quote_ident(name),
                quoted_columns(columns)
            )
        }
        Constraint::Check { name, expression } => {
            format!("CONSTRAINT {} CHECK ({})", quote_ident(name), expression)
        }
    }
}

fn foreign_key_clause(foreign_key: &crate::states::ForeignKey) -> Result<String, DialectError> {
    let mut clause = format!(
        "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        quote_ident(&foreign_key.name),
        quoted_columns(&foreign_key.columns),
        quote_table_name(&foreign_key.to_table)?,
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
        other => Err(unsupported(
            "foreign_key",
            &format!("unsupported on_delete action '{other}'"),
        )),
    }
}

fn col_def(c: &Column, for_add_column: bool) -> Result<String, DialectError> {
    col_def_with_primary_key(c, for_add_column, true)
}

fn col_def_with_primary_key(
    c: &Column,
    for_add_column: bool,
    render_primary_key: bool,
) -> Result<String, DialectError> {
    if for_add_column {
        if c.primary_key {
            return Err(unsupported(
                "add_column",
                "SQLite cannot add a PRIMARY KEY column with ALTER TABLE",
            ));
        }
        if c.references.is_some() || c.check.is_some() || c.generated.is_some() {
            return Err(unsupported(
                "add_column",
                "SQLite cannot add generated, checked, or inline foreign-key columns in the supported v1 subset",
            ));
        }
    }

    let mut s = format!("{} {}", quote_ident(&c.name), c.col_type);
    if let Some(ref expr) = c.generated {
        s.push_str(&format!(" GENERATED ALWAYS AS ({expr}) STORED"));
        return Ok(s);
    }
    if render_primary_key && c.primary_key {
        s.push_str(" PRIMARY KEY");
    } else if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(ref default) = c.default {
        s.push_str(&format!(" DEFAULT {default}"));
    }
    if let Some(ref expr) = c.check {
        s.push_str(&format!(" CHECK ({expr})"));
    }
    Ok(s)
}

fn create_table_sql(table: &Table, name: &str) -> Result<String, DialectError> {
    let inline_pk = match &table.primary_key {
        None => true,
        Some(pk) => {
            pk.columns.len() == 1
                && (pk.name == table.pk_constraint_name() || name.starts_with("__gaman_rebuild_"))
        }
    };
    let mut parts: Vec<String> = table
        .columns
        .iter()
        .map(|c| col_def_with_primary_key(c, false, inline_pk))
        .collect::<Result<_, _>>()?;
    if let Some(pk) = &table.primary_key
        && !inline_pk
    {
        parts.push(primary_key_def(&pk.name, &pk.columns));
    }
    for fk in &table.foreign_keys {
        parts.push(foreign_key_clause(fk)?);
    }
    for c in &table.constraints {
        parts.push(inline_constraint_def(c));
    }
    Ok(format!(
        "CREATE TABLE {} ({})",
        quote_table_name(name)?,
        parts.join(", ")
    ))
}

fn primary_key_def(name: &str, columns: &[String]) -> String {
    if name.is_empty() {
        format!("PRIMARY KEY ({})", quoted_columns(columns))
    } else {
        format!(
            "CONSTRAINT {} PRIMARY KEY ({})",
            quote_ident(name),
            quoted_columns(columns)
        )
    }
}

fn create_index_sql(
    index: &Index,
    table_name: &str,
    concurrent: bool,
) -> Result<String, DialectError> {
    if concurrent {
        return Err(unsupported(
            "add_index",
            "SQLite does not support concurrent index creation",
        ));
    }
    let unique = if index.unique { "UNIQUE " } else { "" };
    let predicate = index
        .predicate
        .as_ref()
        .map(|p| format!(" WHERE {p}"))
        .unwrap_or_default();
    Ok(format!(
        "CREATE {}INDEX {} ON {} ({}){}",
        unique,
        quote_ident(&index.name),
        quote_table_name(table_name)?,
        quoted_columns(&index.columns),
        predicate,
    ))
}

fn drop_index_sql(index: &Index, concurrent: bool) -> Result<String, DialectError> {
    if concurrent {
        return Err(unsupported(
            "drop_index",
            "SQLite does not support concurrent index drops",
        ));
    }
    Ok(format!("DROP INDEX {}", quote_ident(&index.name)))
}

fn rebuild_table_name(op: &Operation) -> Option<&str> {
    match op {
        Operation::DropColumn { table_name, .. }
        | Operation::AlterColumn { table_name, .. }
        | Operation::AddForeignKey { table_name, .. }
        | Operation::DropForeignKey { table_name, .. }
        | Operation::AddConstraint { table_name, .. }
        | Operation::DropConstraint { table_name, .. } => Some(table_name),
        Operation::AddColumn { table_name, column } if add_column_requires_rebuild(column) => {
            Some(table_name)
        }
        _ => None,
    }
}

fn add_column_requires_rebuild(column: &Column) -> bool {
    column.primary_key
        || column.references.is_some()
        || column.check.is_some()
        || column.generated.is_some()
        || (!column.nullable && column.default.is_none())
}

fn apply_ops(mut state: Schema, ops: &[Operation]) -> Result<Schema, DialectError> {
    for op in ops {
        state.apply(op).map_err(|error| {
            unsupported(
                op.type_name(),
                format!("SQLite table rebuild planning failed: {error}"),
            )
        })?;
    }
    Ok(state)
}

fn rebuild_table_sql(
    state: &Schema,
    before: &Table,
    after: &Table,
    ops: &[Operation],
) -> Result<Vec<String>, DialectError> {
    validate_rebuild(state, before, after)?;

    let temp_name = format!("__gaman_rebuild_{}", after.name);
    let fk_check_name = format!("__gaman_fk_check_{}", after.name);
    let mut temp_table = after.clone();
    temp_table.name = temp_name.clone();
    let copy = copy_mapping(before, after, ops)?;

    let mut stmts = vec![
        "PRAGMA defer_foreign_keys = ON".to_string(),
        create_table_sql(&temp_table, &temp_name)?,
    ];

    if !copy.columns.is_empty() {
        stmts.push(format!(
            "INSERT INTO {} ({}) SELECT {} FROM {}",
            quote_table_name(&temp_name)?,
            quoted_columns(&copy.columns),
            copy.expressions.join(", "),
            quote_table_name(&before.name)?,
        ));
    }

    stmts.push(format!("DROP TABLE {}", quote_table_name(&before.name)?));
    stmts.push(format!(
        "ALTER TABLE {} RENAME TO {}",
        quote_table_name(&temp_name)?,
        quote_ident(&after.name),
    ));
    for index in &after.indexes {
        stmts.push(create_index_sql(index, &after.name, false)?);
    }
    stmts.push(format!(
        "DROP TABLE IF EXISTS temp.{}",
        quote_ident(&fk_check_name)
    ));
    stmts.push(format!(
        r#"CREATE TEMP TABLE {} ("violation" integer CHECK ("violation" = 0))"#,
        quote_ident(&fk_check_name)
    ));
    stmts.push(format!(
        r#"INSERT INTO temp.{} ("violation") SELECT 1 FROM pragma_foreign_key_check"#,
        quote_ident(&fk_check_name)
    ));
    stmts.push(format!("DROP TABLE temp.{}", quote_ident(&fk_check_name)));
    stmts.push("PRAGMA foreign_key_check".to_string());
    Ok(stmts)
}

struct CopyMapping {
    columns: Vec<String>,
    expressions: Vec<String>,
}

fn copy_mapping(
    before: &Table,
    after: &Table,
    ops: &[Operation],
) -> Result<CopyMapping, DialectError> {
    let mut columns = Vec::new();
    let mut expressions = Vec::new();

    for target in after
        .columns
        .iter()
        .filter(|column| column.generated.is_none())
    {
        columns.push(target.name.clone());
        match before
            .columns
            .iter()
            .find(|column| column.name == target.name)
        {
            Some(source) => expressions.push(copy_expr(source, target, ops)?),
            None => expressions.push(new_column_expr(target)?),
        }
    }

    Ok(CopyMapping {
        columns,
        expressions,
    })
}

fn copy_expr(source: &Column, target: &Column, ops: &[Operation]) -> Result<String, DialectError> {
    let alter = ops.iter().find_map(|op| match op {
        Operation::AlterColumn {
            old,
            new,
            cast_expr,
            ..
        } if old.name == source.name && new.name == target.name => Some(cast_expr.as_deref()),
        _ => None,
    });

    let type_changed = normalize_type(&source.col_type) != normalize_type(&target.col_type);
    let mut expr = if let Some(Some(cast_expr)) = alter {
        cast_expr.to_string()
    } else if type_changed {
        validate_auto_cast_type(&target.col_type)?;
        format!("CAST({} AS {})", quote_ident(&source.name), target.col_type)
    } else {
        quote_ident(&source.name)
    };

    if source.nullable && !target.nullable && !target.primary_key {
        match (alter.flatten(), target.default.as_deref()) {
            (Some(_), _) => {}
            (None, Some(default)) => {
                expr = format!("COALESCE({expr}, {default})");
            }
            (None, None) => {
                return Err(unsupported(
                    "alter_column",
                    format!(
                        "SQLite cannot rebuild nullable column '{}' as NOT NULL without a default or cast expression",
                        target.name
                    ),
                ));
            }
        }
    }

    Ok(expr)
}

fn validate_auto_cast_type(col_type: &str) -> Result<(), DialectError> {
    let normalized = normalize_type(col_type).to_ascii_lowercase();
    let base = normalized
        .split_once('(')
        .map_or(normalized.as_str(), |(head, _)| head)
        .trim();
    let supported = matches!(
        base,
        "integer"
            | "int"
            | "bigint"
            | "smallint"
            | "tinyint"
            | "real"
            | "double"
            | "float"
            | "text"
            | "varchar"
            | "char"
            | "clob"
            | "blob"
            | "numeric"
            | "decimal"
            | "boolean"
    );
    if supported {
        Ok(())
    } else {
        Err(unsupported(
            "alter_column",
            format!(
                "SQLite cannot automatically cast to ambiguous type '{col_type}'; provide cast_expr"
            ),
        ))
    }
}

fn new_column_expr(column: &Column) -> Result<String, DialectError> {
    if let Some(default) = &column.default {
        Ok(default.clone())
    } else if column.nullable {
        Ok("NULL".to_string())
    } else {
        Err(unsupported(
            "add_column",
            format!(
                "SQLite cannot rebuild new NOT NULL column '{}' without a default",
                column.name
            ),
        ))
    }
}

fn validate_rebuild(state: &Schema, before: &Table, after: &Table) -> Result<(), DialectError> {
    qualified_table(before)?;
    qualified_table(after)?;

    let temp_name = format!("__gaman_rebuild_{}", after.name);
    if state.tables.contains_key(&temp_name) {
        return Err(unsupported(
            "table_rebuild",
            format!("SQLite rebuild temp table '{temp_name}' already exists"),
        ));
    }
    let fk_check_name = format!("__gaman_fk_check_{}", after.name);
    if state.tables.contains_key(&fk_check_name) {
        return Err(unsupported(
            "table_rebuild",
            format!("SQLite rebuild FK-check temp table '{fk_check_name}' already exists"),
        ));
    }

    if !before.triggers.is_empty() || !after.triggers.is_empty() {
        return Err(unsupported(
            "table_rebuild",
            format!(
                "SQLite rebuild for table '{}' with modeled triggers is not supported yet",
                after.name
            ),
        ));
    }

    if let Some(view_name) = dependent_view(state, &after.name) {
        return Err(unsupported(
            "table_rebuild",
            format!(
                "SQLite rebuild for table '{}' is blocked by dependent view '{}'",
                after.name, view_name
            ),
        ));
    }

    if let Some((child_table, fk_name)) = inbound_foreign_key(state, &after.name) {
        return Err(unsupported(
            "table_rebuild",
            format!(
                "SQLite rebuild for table '{}' is blocked by inbound foreign key '{}' on table '{}'",
                after.name, fk_name, child_table
            ),
        ));
    }

    if pk_signature(before) != pk_signature(after) {
        return Err(unsupported(
            "table_rebuild",
            format!(
                "SQLite rebuild for table '{}' does not support primary-key changes",
                after.name
            ),
        ));
    }

    validate_target_references(state, after)
}

fn dependent_view<'a>(state: &'a Schema, table_name: &str) -> Option<&'a str> {
    state
        .views
        .values()
        .find(|view| view_references_table(&view.definition, table_name))
        .map(|view| view.name.as_str())
}

fn inbound_foreign_key<'a>(state: &'a Schema, table_name: &str) -> Option<(&'a str, &'a str)> {
    state.tables.values().find_map(|table| {
        if table.name == table_name {
            return None;
        }
        table
            .foreign_keys
            .iter()
            .find(|fk| fk.to_table == table_name)
            .map(|fk| (table.name.as_str(), fk.name.as_str()))
    })
}

fn view_references_table(definition: &str, table_name: &str) -> bool {
    identifier_tokens(definition)
        .iter()
        .any(|token| token.eq_ignore_ascii_case(table_name))
}

fn identifier_tokens(sql: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut chars = sql.char_indices().peekable();
    while let Some((_, ch)) = chars.next() {
        if ch == '"' || ch == '`' || ch == '[' {
            let close = if ch == '[' { ']' } else { ch };
            let mut ident = String::new();
            while let Some((_, next)) = chars.next() {
                if next == close {
                    if close != ']'
                        && matches!(chars.peek(), Some((_, escaped)) if *escaped == close)
                    {
                        ident.push(next);
                        chars.next();
                        continue;
                    }
                    break;
                }
                ident.push(next);
            }
            if !ident.is_empty() {
                tokens.push(ident);
            }
        } else if ch == '\'' {
            while let Some((_, next)) = chars.next() {
                if next == '\'' {
                    if matches!(chars.peek(), Some((_, '\''))) {
                        chars.next();
                        continue;
                    }
                    break;
                }
            }
        } else if is_ident_start(ch) {
            let mut ident = String::new();
            ident.push(ch);
            while let Some((_, next)) = chars.peek().copied() {
                if is_ident_continue(next) {
                    ident.push(next);
                    chars.next();
                } else {
                    break;
                }
            }
            tokens.push(ident);
        }
    }
    tokens
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn pk_signature(table: &Table) -> Vec<(&str, &str)> {
    table
        .primary_key_column_names()
        .into_iter()
        .filter_map(|name| {
            table
                .columns
                .iter()
                .find(|column| column.name == name)
                .map(|column| (column.name.as_str(), normalize_type(&column.col_type)))
        })
        .collect()
}

fn validate_target_references(state: &Schema, table: &Table) -> Result<(), DialectError> {
    let column_exists = |name: &str| table.columns.iter().any(|column| column.name == name);
    for index in &table.indexes {
        for column in &index.columns {
            if !column_exists(column) {
                return Err(unsupported(
                    "table_rebuild",
                    format!(
                        "SQLite rebuild target index '{}' references missing column '{}'",
                        index.name, column
                    ),
                ));
            }
        }
    }
    for constraint in &table.constraints {
        if let Constraint::Unique { name, columns } = constraint {
            for column in columns {
                if !column_exists(column) {
                    return Err(unsupported(
                        "table_rebuild",
                        format!(
                            "SQLite rebuild target constraint '{}' references missing column '{}'",
                            name, column
                        ),
                    ));
                }
            }
        }
    }
    for fk in &table.foreign_keys {
        for column in &fk.columns {
            if !column_exists(column) {
                return Err(unsupported(
                    "table_rebuild",
                    format!(
                        "SQLite rebuild target foreign key '{}' references missing column '{}'",
                        fk.name, column
                    ),
                ));
            }
        }
        if fk.to_table != table.name && !state.tables.contains_key(&fk.to_table) {
            return Err(unsupported(
                "table_rebuild",
                format!(
                    "SQLite rebuild target foreign key '{}' references unknown table '{}'",
                    fk.name, fk.to_table
                ),
            ));
        }
    }
    Ok(())
}

fn operation_to_sql(op: &Operation) -> Result<Vec<String>, DialectError> {
    match op {
        Operation::CreateTable { table } => {
            qualified_table(table)?;
            let mut stmts = vec![create_table_sql(table, &table.name)?];
            for index in &table.indexes {
                stmts.push(create_index_sql(index, &table.name, false)?);
            }
            Ok(stmts)
        }
        Operation::DropTable { table } => {
            Ok(vec![format!("DROP TABLE {}", qualified_table(table)?)])
        }
        Operation::RenameTable { old_name, new_name } => Ok(vec![format!(
            "ALTER TABLE {} RENAME TO {}",
            quote_table_name(old_name)?,
            quote_ident(new_name)
        )]),
        Operation::AddColumn { table_name, column } => Ok(vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            quote_table_name(table_name)?,
            col_def(column, true)?
        )]),
        Operation::RenameColumn {
            table_name,
            old_name,
            new_name,
        } => Ok(vec![format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            quote_table_name(table_name)?,
            quote_ident(old_name),
            quote_ident(new_name)
        )]),
        Operation::AddIndex {
            table_name,
            index,
            concurrent,
        } => Ok(vec![create_index_sql(index, table_name, *concurrent)?]),
        Operation::DropIndex {
            index, concurrent, ..
        } => Ok(vec![drop_index_sql(index, *concurrent)?]),
        Operation::Statement { up, .. } => Ok(vec![up.clone()]),
        Operation::CreateView { view } => Ok(vec![format!(
            "CREATE VIEW {} AS {}",
            qualified_view(view)?,
            view.definition
        )]),
        Operation::DropView { view } => Ok(vec![format!("DROP VIEW {}", qualified_view(view)?)]),
        Operation::ReplaceView { old, new } => Ok(vec![
            format!("DROP VIEW {}", qualified_view(old)?),
            format!("CREATE VIEW {} AS {}", qualified_view(new)?, new.definition),
        ]),
        Operation::CreateTrigger {
            table_name,
            trigger,
        }
        | Operation::AlterTrigger {
            table_name,
            new: trigger,
            ..
        } => Ok(vec![create_trigger_sql(table_name, trigger)?]),
        Operation::DropTrigger {
            table_name,
            trigger,
        } => {
            let name = trigger.name.as_deref().unwrap_or("");
            let _ = table_name;
            Ok(vec![format!("DROP TRIGGER {}", quote_ident(name))])
        }
        Operation::DropColumn { .. }
        | Operation::AlterColumn { .. }
        | Operation::AddForeignKey { .. }
        | Operation::DropForeignKey { .. }
        | Operation::AddConstraint { .. }
        | Operation::DropConstraint { .. } => Err(unsupported(
            op.type_name(),
            "SQLite requires migration context for table rebuilds; render through Migrator",
        )),
        Operation::CreateFunction { .. }
        | Operation::AlterFunction { .. }
        | Operation::DropFunction { .. }
        | Operation::CreateExtension { .. }
        | Operation::DropExtension { .. }
        | Operation::CreateEnum { .. }
        | Operation::DropEnum { .. }
        | Operation::RenameEnumValue { .. }
        | Operation::AlterEnum { .. } => Err(unsupported(
            op.type_name(),
            "operation is not supported by the SQLite dialect",
        )),
    }
}

fn create_trigger_sql(table_name: &str, trigger: &TriggerDef) -> Result<String, DialectError> {
    if trigger.function_name.is_some() {
        return Err(unsupported(
            "create_trigger",
            "SQLite trigger rendering requires `query`; `function_name` is PostgreSQL-only",
        ));
    }
    if trigger.language.is_some() {
        return Err(unsupported(
            "create_trigger",
            "SQLite trigger rendering does not support `language`",
        ));
    }
    let Some(query) = trigger.query.as_deref() else {
        return Err(unsupported(
            "create_trigger",
            "SQLite trigger rendering requires `query`",
        ));
    };
    let name = trigger.name.as_deref().unwrap_or("");
    let timing = match trigger.timing {
        TriggerTiming::Before => "BEFORE",
        TriggerTiming::After => "AFTER",
        TriggerTiming::InsteadOf => "INSTEAD OF",
    };
    let mut events = Vec::new();
    for event in &trigger.events {
        events.push(match event {
            TriggerEvent::Insert => "INSERT",
            TriggerEvent::Update => "UPDATE",
            TriggerEvent::Delete => "DELETE",
            TriggerEvent::Truncate => {
                return Err(unsupported(
                    "create_trigger",
                    "SQLite triggers do not support TRUNCATE events",
                ));
            }
        });
    }
    events.sort_unstable();
    let scope = match trigger.scope {
        TriggerScope::Row => "FOR EACH ROW",
        TriggerScope::Statement => {
            return Err(unsupported(
                "create_trigger",
                "SQLite only supports row-level triggers",
            ));
        }
    };
    let when_clause = trigger
        .when
        .as_deref()
        .map(|condition| format!("\nWHEN {condition}"))
        .unwrap_or_default();
    Ok(format!(
        "CREATE TRIGGER {}\n{} {} ON {}\n{}{}\nBEGIN\n{}\nEND",
        quote_ident(name),
        timing,
        events.join(" OR "),
        quote_table_name(table_name)?,
        scope,
        when_clause,
        query.trim()
    ))
}

pub(crate) fn migration_to_sql(
    migration: &Migration,
    start: &Schema,
) -> Result<Vec<String>, DialectError> {
    let mut state = start.clone();
    let mut stmts = Vec::new();
    let mut index = 0;

    while index < migration.operations.len() {
        let op = &migration.operations[index];
        if let Some(table_name) = rebuild_table_name(op) {
            if !migration.atomic {
                return Err(unsupported(
                    op.type_name(),
                    "SQLite table rebuilds require atomic migrations",
                ));
            }
            let mut end = index + 1;
            while end < migration.operations.len() {
                match rebuild_table_name(&migration.operations[end]) {
                    Some(next_table) if next_table == table_name => end += 1,
                    _ => break,
                }
            }
            let group = &migration.operations[index..end];
            let next_state = apply_ops(state.clone(), group)?;
            let before = state.tables.get(table_name).ok_or_else(|| {
                unsupported(
                    table_name,
                    format!("SQLite table rebuild target '{table_name}' does not exist"),
                )
            })?;
            let after = next_state.tables.get(table_name).ok_or_else(|| {
                unsupported(
                    table_name,
                    format!("SQLite table rebuild target '{table_name}' is missing after replay"),
                )
            })?;
            stmts.extend(rebuild_table_sql(&state, before, after, group)?);
            state = next_state;
            index = end;
        } else {
            stmts.extend(operation_to_sql(op)?);
            state = apply_ops(state, std::slice::from_ref(op))?;
            index += 1;
        }
    }

    Ok(stmts)
}

pub fn validate_migration(m: &Migration) -> Result<(), DialectError> {
    for op in &m.operations {
        if rebuild_table_name(op).is_some() {
            continue;
        }
        operation_to_sql(op)?;
    }
    Ok(())
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

pub fn validate_schema(schema: &Schema) -> Result<(), SchemaValidationError> {
    if !schema.extensions.is_empty() {
        let names = schema
            .extensions
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite does not support extensions: {names}"
        )));
    }
    if !schema.enums.is_empty() {
        let names = schema.enums.keys().cloned().collect::<Vec<_>>().join(", ");
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite does not support enums: {names}"
        )));
    }
    if !schema.functions.is_empty() {
        let names = schema
            .functions
            .keys()
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite does not support stored functions: {names}"
        )));
    }

    for (table_name, table) in &schema.tables {
        if table.schema.is_some() {
            return Err(SchemaValidationError::Invalid(format!(
                "SQLite does not support schema-qualified tables: {table_name}"
            )));
        }
        for trigger in &table.triggers {
            validate_sqlite_trigger(table_name, trigger)?;
        }
    }

    Ok(())
}

fn validate_sqlite_trigger(
    table_name: &str,
    trigger: &TriggerDef,
) -> Result<(), SchemaValidationError> {
    let name = trigger.name.as_deref().unwrap_or("<unnamed>");
    if trigger.function_name.is_some() {
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite trigger {table_name}.{name} must use `query`; `function_name` is PostgreSQL-only"
        )));
    }
    if trigger.language.is_some() {
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite trigger {table_name}.{name} must not set `language`"
        )));
    }
    if trigger.events.contains(&TriggerEvent::Truncate) {
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite trigger {table_name}.{name} does not support truncate events"
        )));
    }
    if trigger.scope == TriggerScope::Statement {
        return Err(SchemaValidationError::Invalid(format!(
            "SQLite trigger {table_name}.{name} must be row-level"
        )));
    }
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

#[cfg(test)]
mod tests;
