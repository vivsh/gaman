use std::collections::HashSet;

use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::{Column, Constraint, ForeignKey, Index, Schema, Table, ViewDef, Volatility};

use super::DialectError;

// PostgreSQL-specific constraint: DROP FUNCTION fails when dependent triggers still exist.
// This arises when a function's argument signature changes, because the old signature cannot
// be replaced in-place — it must be dropped and recreated. Any trigger referencing that
// function must be bounced (dropped before, recreated after) regardless of whether the
// trigger itself changed. This is transparent to the generic diff algorithm.
pub fn reorder_ops(ops: Vec<Operation>, previous: &Schema, current: &Schema) -> Vec<Operation> {
    let sig_changed_fns: HashSet<&str> = ops.iter()
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

    let dropped_tables: HashSet<&str> = ops.iter()
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
            if trigger.function_name.as_deref().map_or(false, |f| sig_changed_fns.contains(f)) {
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
            if trigger.function_name.as_deref().map_or(false, |f| sig_changed_fns.contains(f)) {
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
            Operation::DropTrigger { table_name, trigger } => {
                trigger.name.as_deref().map(|n| format!("{}:{}", table_name, n))
            }
            Operation::CreateTrigger { table_name, trigger } => {
                trigger.name.as_deref().map(|n| format!("{}:{}", table_name, n))
            }
            Operation::AlterTrigger { table_name, old, .. } => {
                old.name.as_deref().map(|n| format!("{}:{}", table_name, n))
            }
            _ => None,
        }
    };

    // Find where the first AlterFunction (sig change) sits — pre_fn_drops go just before it,
    // post_fn_creates go just after the last one.
    let first_alter = ops.iter().position(|op| matches!(op, Operation::AlterFunction { old, new } if old.arguments != new.arguments));
    let last_alter = ops.iter().rposition(|op| matches!(op, Operation::AlterFunction { old, new } if old.arguments != new.arguments));

    let (first_alter, last_alter) = match (first_alter, last_alter) {
        (Some(f), Some(l)) => (f, l),
        _ => return ops,
    };

    let mut result = Vec::with_capacity(ops.len() + pre_fn_drops.len() + post_fn_creates.len());
    for (i, op) in ops.into_iter().enumerate() {
        if i == first_alter {
            result.extend(pre_fn_drops.drain(..));
        }
        if let Some(key) = trigger_key(&op) {
            if bounced_keys.contains(&key) {
                continue; // already handled via pre/post
            }
        }
        result.push(op);
        if i == last_alter {
            result.extend(post_fn_creates.drain(..));
        }
    }
    result
}

pub fn normalize_type(t: &str) -> &str {
    match t {
        "int"               => "integer",
        "int2"              => "smallint",
        "int4"              => "integer",
        "int8"              => "bigint",
        "bool"              => "boolean",
        "float4"            => "real",
        "float8"            => "double precision",
        "bpchar"            => "char",
        "character varying" => "varchar",
        "timestamp"         => "timestamp without time zone",
        "timestamptz"       => "timestamp with time zone",
        other               => other,
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn quote_table_name(name: &str) -> String {
    if let Some((schema, table)) = name.split_once('.') {
        format!("{}.{}", quote_ident(schema), quote_ident(table))
    } else {
        quote_ident(name)
    }
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

fn quoted_columns(columns: &[String]) -> String {
    columns.iter().map(|column| quote_ident(column)).collect::<Vec<_>>().join(", ")
}

fn foreign_key_clause(foreign_key: &ForeignKey) -> String {
    format!(
        "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        quote_ident(&foreign_key.name),
        quote_ident(&foreign_key.from_column),
        quote_table_name(&foreign_key.to_table),
        quote_ident(&foreign_key.to_column),
    )
}

fn create_index_sql(index: &Index, table_name: &str, concurrent: bool) -> String {
    let unique = if index.unique { "UNIQUE " } else { "" };
    let concurrent = if concurrent { "CONCURRENTLY " } else { "" };
    format!(
        "CREATE {}INDEX {}{} ON {} ({})",
        unique,
        concurrent,
        quote_ident(&index.name),
        quote_table_name(table_name),
        quoted_columns(&index.columns),
    )
}

fn drop_index_sql(index_name: &str, concurrent: bool) -> String {
    let concurrent = if concurrent { " CONCURRENTLY" } else { "" };
    format!("DROP INDEX{} {}", concurrent, quote_ident(index_name))
}

pub fn operation_to_sql(op: &Operation) -> Result<Vec<String>, DialectError> {
    let stmts = match op {
        Operation::CreateTable { table } => {
            let mut parts: Vec<String> = table.columns.iter().map(col_def).collect();
            for fk in &table.foreign_keys {
                parts.push(foreign_key_clause(fk));
            }
            for c in &table.constraints {
                parts.push(format!("CONSTRAINT {}", inline_constraint_def(c)));
            }
            let mut stmts = vec![format!("CREATE TABLE {} ({})", qualified_table(table), parts.join(", "))];
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
            vec![format!("ALTER TABLE {} RENAME TO {}", quote_table_name(old_name), quote_ident(new_name))]
        }
        Operation::AddColumn { table_name, column } => {
            vec![format!("ALTER TABLE {} ADD COLUMN {}", quote_table_name(table_name), col_def(column))]
        }
        Operation::DropColumn { table_name, column, cascade } => {
            let suffix = if *cascade { " CASCADE" } else { "" };
            vec![format!("ALTER TABLE {} DROP COLUMN {}{}", quote_table_name(table_name), quote_ident(&column.name), suffix)]
        }
        Operation::RenameColumn { table_name, old_name, new_name } => {
            vec![format!("ALTER TABLE {} RENAME COLUMN {} TO {}", quote_table_name(table_name), quote_ident(old_name), quote_ident(new_name))]
        }
        Operation::AlterColumn { table_name, old, new, cast_expr } => {
            alter_column_statements(table_name, old, new, cast_expr.as_deref())
        }
        Operation::AddForeignKey { table_name, foreign_key } => {
            vec![format!("ALTER TABLE {} ADD {}", quote_table_name(table_name), foreign_key_clause(foreign_key))]
        }
        Operation::DropForeignKey { table_name, foreign_key, cascade } => {
            let suffix = if *cascade { " CASCADE" } else { "" };
            vec![format!("ALTER TABLE {} DROP CONSTRAINT {}{}", quote_table_name(table_name), quote_ident(&foreign_key.name), suffix)]
        }
        Operation::AddIndex { table_name, index, concurrent } => {
            vec![create_index_sql(index, table_name, *concurrent)]
        }
        Operation::DropIndex { index, concurrent, .. } => {
            vec![drop_index_sql(&index.name, *concurrent)]
        }
        Operation::AddConstraint { table_name, constraint } => {
            vec![format!("ALTER TABLE {} ADD CONSTRAINT {}", quote_table_name(table_name), inline_constraint_def(constraint))]
        }
        Operation::DropConstraint { table_name, constraint } => {
            vec![format!("ALTER TABLE {} DROP CONSTRAINT {}", quote_table_name(table_name), quote_ident(constraint.name()))]
        }
        Operation::Statement { up, .. } => {
            vec![up.clone()]
        }
        Operation::Invoke { .. } => {
            vec![]
        }
        Operation::CreateFunction { function } => {
            vec![create_function_sql(function)?]
        }
        Operation::DropFunction { function } => {
            vec![format!("DROP FUNCTION {}({})", quote_ident(&function.name), function.arguments)]
        }
        Operation::AlterFunction { old, new } => {
            if old.arguments == new.arguments {
                vec![create_function_sql(new)?]
            } else {
                eprintln!(
                    "warning: function '{}' argument signature changed — dropping old and creating new; \
                     existing callers referencing the old signature will break",
                    old.name
                );
                vec![
                    format!("DROP FUNCTION {}({})", quote_ident(&old.name), old.arguments),
                    create_function_sql(new)?,
                ]
            }
        }
        Operation::CreateTrigger { table_name, trigger } => {
            vec![create_trigger_sql(table_name, trigger)]
        }
        Operation::AlterTrigger { table_name, new, .. } => {
            vec![create_trigger_sql(table_name, new)]
        }
        Operation::DropTrigger { table_name, trigger } => {
            let tname = trigger.name.as_deref().unwrap_or("");
            vec![format!("DROP TRIGGER {} ON {}", quote_ident(tname), quote_table_name(table_name))]
        }
        Operation::CreateView { view } => {
            vec![format!("CREATE OR REPLACE VIEW {} AS {}", qualified_view(view), view.definition)]
        }
        Operation::DropView { view } => {
            vec![format!("DROP VIEW {}", qualified_view(view))]
        }
        Operation::ReplaceView { new, .. } => {
            vec![format!("CREATE OR REPLACE VIEW {} AS {}", qualified_view(new), new.definition)]
        }
        Operation::CreateExtension { extension } => {
            let mut sql = format!("CREATE EXTENSION IF NOT EXISTS {}", quote_ident(&extension.name));
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
            let values: Vec<String> = enum_def.values.iter()
                .map(|v| format!("'{}'", v.replace('\'', "''")))
                .collect();
            let name = qualified_name(&enum_def.name, enum_def.schema.as_deref());
            vec![format!("CREATE TYPE {name} AS ENUM ({})", values.join(", "))]
        }
        Operation::DropEnum { enum_def } => {
            let name = qualified_name(&enum_def.name, enum_def.schema.as_deref());
            vec![format!("DROP TYPE {name}")]
        }
        Operation::AlterEnum { old, new } => {
            let name = qualified_name(&new.name, new.schema.as_deref());
            let old_set: std::collections::HashSet<&str> = old.values.iter().map(|v| v.as_str()).collect();
            new.values.iter()
                .filter(|v| !old_set.contains(v.as_str()))
                .map(|v| format!("ALTER TYPE {name} ADD VALUE '{}'", v.replace('\'', "''")))
                .collect()
        }
    };
    Ok(stmts)
}

pub fn validate_migration(m: &Migration) -> Result<(), super::DialectError> {
    if m.atomic {
        for op in &m.operations {
            match op {
                Operation::AddIndex { concurrent: true, .. } => {
                    return Err(super::DialectError::Unsupported(
                        "add_index".into(),
                        "CONCURRENTLY requires atomic = false on the migration".into(),
                    ));
                }
                Operation::DropIndex { concurrent: true, .. } => {
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

pub fn create_tracking_table_sql() -> Vec<String> {
    vec![
        r#"CREATE TABLE IF NOT EXISTS "gaman_migrations" ("id" text NOT NULL, "applied_at" timestamptz NOT NULL DEFAULT now(), CONSTRAINT "gaman_migrations_id_key" UNIQUE ("id"))"#.to_string(),
        r#"CREATE INDEX IF NOT EXISTS "gaman_migrations_id_idx" ON "gaman_migrations" ("id")"#.to_string(),
    ]
}

pub fn col_def(c: &Column) -> String {
    let mut s = format!("{} {}", quote_ident(&c.name), c.col_type);
    if c.primary_key {
        s.push_str(" PRIMARY KEY");
    } else if !c.nullable {
        s.push_str(" NOT NULL");
    }
    if let Some(ref default) = c.default {
        s.push_str(&format!(" DEFAULT {default}"));
    }
    s
}

fn alter_column_statements(table: &str, old: &Column, new: &Column, cast_expr: Option<&str>) -> Vec<String> {
    let mut stmts = Vec::new();

    if old.col_type != new.col_type {
        let using = cast_expr.map(|e| format!(" USING {e}")).unwrap_or_default();
        stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} TYPE {}{}",
            quote_table_name(table), quote_ident(&new.name), new.col_type, using,
        ));
    }

    match (old.nullable, new.nullable) {
        (false, true) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP NOT NULL",
            quote_table_name(table), quote_ident(&new.name)
        )),
        (true, false) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} SET NOT NULL",
            quote_table_name(table), quote_ident(&new.name)
        )),
        _ => {}
    }

    match (&old.default, &new.default) {
        (_, Some(d)) if old.default.as_deref() != Some(d.as_str()) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} SET DEFAULT {d}",
            quote_table_name(table), quote_ident(&new.name)
        )),
        (Some(_), None) => stmts.push(format!(
            "ALTER TABLE {} ALTER COLUMN {} DROP DEFAULT",
            quote_table_name(table), quote_ident(&new.name)
        )),
        _ => {}
    }

    match (old.primary_key, new.primary_key) {
        (false, true) => stmts.push(format!(
            "ALTER TABLE {} ADD PRIMARY KEY ({})",
            quote_table_name(table), quote_ident(&new.name)
        )),
        (true, false) => stmts.push(format!(
            "ALTER TABLE {} DROP CONSTRAINT {}",
            quote_table_name(table), quote_ident(&Table::pk_constraint_name_for(table))
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
            "trigger functions cannot have declared arguments (use TG_NARGS/TG_ARGV instead)".into(),
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
    let sec = if f.security_definer { "\nSECURITY DEFINER" } else { "" };
    Ok(format!(
        "CREATE OR REPLACE FUNCTION {}({})\nRETURNS {}\nLANGUAGE {}{}{}
AS $func$\n{}\n$func$",
        quote_ident(&f.name), f.arguments, f.returns, f.language, vol, sec, f.body
    ))
}

fn create_trigger_sql(table_name: &str, t: &crate::states::TriggerDef) -> String {
    use crate::states::{TriggerEvent, TriggerScope, TriggerTiming};
    let tname = t.name.as_deref().unwrap_or("");
    let timing = match t.timing {
        TriggerTiming::Before => "BEFORE",
        TriggerTiming::After => "AFTER",
        TriggerTiming::InsteadOf => "INSTEAD OF",
    };
    let mut events: Vec<&str> = t.events.iter().map(|e| match e {
        TriggerEvent::Insert => "INSERT",
        TriggerEvent::Update => "UPDATE",
        TriggerEvent::Delete => "DELETE",
        TriggerEvent::Truncate => "TRUNCATE",
    }).collect();
    events.sort_unstable();
    let scope = match t.scope {
        TriggerScope::Row => "ROW",
        TriggerScope::Statement => "STATEMENT",
    };
    let when_clause = t.when.as_deref()
        .map(|w| format!("\nWHEN ({})", w))
        .unwrap_or_default();
    let fn_name = t.function_name.as_deref().unwrap_or("");
    format!(
        "CREATE OR REPLACE TRIGGER {}\n{} {}\nON {}\nFOR EACH {}{}
EXECUTE FUNCTION {}()",
        quote_ident(tname), timing, events.join(" OR "),
        quote_table_name(table_name), scope, when_clause,
        quote_ident(fn_name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::Operation;
    use crate::states::{Column, ForeignKey, Index, Table};

    fn col(name: &str, t: &str) -> Column {
        Column { name: name.to_string(), col_type: t.to_string(), nullable: false, primary_key: false, default: None, ..Default::default() }
    }

    fn nullable_col(name: &str, t: &str) -> Column {
        Column { name: name.to_string(), col_type: t.to_string(), nullable: true, primary_key: false, default: None, ..Default::default() }
    }

    fn empty_table(name: &str) -> Table {
        Table { name: name.to_string(), schema: None, columns: vec![], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] }
    }

    #[test]
    fn quote_ident_plain() {
        assert_eq!(quote_ident("users"), "\"users\"");
    }

    #[test]
    fn quote_ident_reserved_words() {
        assert_eq!(quote_ident("order"), "\"order\"");
        assert_eq!(quote_ident("user"), "\"user\"");
        assert_eq!(quote_ident("table"), "\"table\"");
    }

    #[test]
    fn quote_ident_spaces_and_hyphens() {
        assert_eq!(quote_ident("my table"), "\"my table\"");
        assert_eq!(quote_ident("my-table"), "\"my-table\"");
    }

    #[test]
    fn quote_ident_embedded_double_quote() {
        let result = quote_ident("it\"s");
        assert_eq!(result, "\"it\"\"s\"");
    }

    #[test]
    fn create_table_basic() {
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![col("id", "serial"), col("name", "text")],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let sql = operation_to_sql(&Operation::CreateTable { table }).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].starts_with("CREATE TABLE \"users\" ("), "got: {}", sql[0]);
        assert!(sql[0].contains("\"id\" serial"), "got: {}", sql[0]);
        assert!(sql[0].contains("\"name\" text"), "got: {}", sql[0]);
    }

    #[test]
    fn create_table_reserved_word_name() {
        let table = Table {
            name: "order".to_string(),
            schema: None,
            columns: vec![col("id", "serial")],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let sql = operation_to_sql(&Operation::CreateTable { table }).unwrap();
        assert!(sql[0].starts_with("CREATE TABLE \"order\" ("), "got: {}", sql[0]);
    }

    #[test]
    fn drop_table_sql() {
        let sql = operation_to_sql(&Operation::DropTable { table: empty_table("users") }).unwrap();
        assert_eq!(sql, vec!["DROP TABLE \"users\""]);
    }

    #[test]
    fn rename_table_sql() {
        let sql = operation_to_sql(&Operation::RenameTable {
            old_name: "users".to_string(),
            new_name: "accounts".to_string(),
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" RENAME TO \"accounts\""]);
    }

    #[test]
    fn add_column_sql() {
        let sql = operation_to_sql(&Operation::AddColumn {
            table_name: "users".to_string(),
            column: col("email", "text"),
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" ADD COLUMN \"email\" text NOT NULL"]);
    }

    #[test]
    fn drop_column_no_cascade() {
        let sql = operation_to_sql(&Operation::DropColumn {
            table_name: "users".to_string(),
            column: col("email", "text"),
            cascade: false,
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" DROP COLUMN \"email\""]);
    }

    #[test]
    fn drop_column_with_cascade() {
        let sql = operation_to_sql(&Operation::DropColumn {
            table_name: "users".to_string(),
            column: col("email", "text"),
            cascade: true,
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" DROP COLUMN \"email\" CASCADE"]);
    }

    #[test]
    fn alter_column_type_no_cast() {
        let old = col("status", "varchar(50)");
        let new = col("status", "text");
        let sql = operation_to_sql(&Operation::AlterColumn {
            table_name: "users".to_string(),
            old, new,
            cast_expr: None,
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" ALTER COLUMN \"status\" TYPE text"]);
    }

    #[test]
    fn alter_column_type_with_cast() {
        let old = col("age", "text");
        let new = col("age", "integer");
        let sql = operation_to_sql(&Operation::AlterColumn {
            table_name: "users".to_string(),
            old, new,
            cast_expr: Some("age::integer".to_string()),
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" ALTER COLUMN \"age\" TYPE integer USING age::integer"]);
    }

    #[test]
    fn alter_column_nullable_change() {
        let old = col("email", "text");
        let new = nullable_col("email", "text");
        let sql = operation_to_sql(&Operation::AlterColumn {
            table_name: "users".to_string(),
            old, new,
            cast_expr: None,
        }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"users\" ALTER COLUMN \"email\" DROP NOT NULL"]);
    }

    #[test]
    fn add_foreign_key_sql() {
        let fk = ForeignKey { name: "posts_user_id_fkey".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        let sql = operation_to_sql(&Operation::AddForeignKey { table_name: "posts".to_string(), foreign_key: fk }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"posts\" ADD CONSTRAINT \"posts_user_id_fkey\" FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\")"]);
    }

    #[test]
    fn drop_foreign_key_no_cascade() {
        let fk = ForeignKey { name: "posts_user_id_fkey".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        let sql = operation_to_sql(&Operation::DropForeignKey { table_name: "posts".to_string(), foreign_key: fk, cascade: false }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"posts\" DROP CONSTRAINT \"posts_user_id_fkey\""]);
    }

    #[test]
    fn drop_foreign_key_with_cascade() {
        let fk = ForeignKey { name: "posts_user_id_fkey".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        let sql = operation_to_sql(&Operation::DropForeignKey { table_name: "posts".to_string(), foreign_key: fk, cascade: true }).unwrap();
        assert_eq!(sql, vec!["ALTER TABLE \"posts\" DROP CONSTRAINT \"posts_user_id_fkey\" CASCADE"]);
    }

    #[test]
    fn add_index_sql() {
        let index = Index { name: "users_email_idx".to_string(), columns: vec!["email".to_string()], unique: false, predicate: None };
        let sql = operation_to_sql(&Operation::AddIndex { table_name: "users".to_string(), index, concurrent: false }).unwrap();
        assert_eq!(sql, vec!["CREATE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]);
    }

    #[test]
    fn add_unique_index_sql() {
        let index = Index { name: "users_email_idx".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None };
        let sql = operation_to_sql(&Operation::AddIndex { table_name: "users".to_string(), index, concurrent: false }).unwrap();
        assert_eq!(sql, vec!["CREATE UNIQUE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]);
    }

    #[test]
    fn drop_index_sql() {
        let index = Index { name: "users_email_idx".to_string(), columns: vec!["email".to_string()], unique: false, predicate: None };
        let sql = operation_to_sql(&Operation::DropIndex { table_name: "users".to_string(), index, concurrent: false }).unwrap();
        assert_eq!(sql, vec!["DROP INDEX \"users_email_idx\""]);
    }

    #[test]
    fn tracking_table_has_two_statements() {
        let sqls = create_tracking_table_sql();
        assert_eq!(sqls.len(), 2);
        assert!(sqls[0].contains("CREATE TABLE IF NOT EXISTS"), "got: {}", sqls[0]);
        assert!(sqls[1].contains("CREATE INDEX IF NOT EXISTS"), "got: {}", sqls[1]);
        assert!(sqls[1].contains("gaman_migrations_id_idx"), "got: {}", sqls[1]);
    }

    fn basic_function(name: &str) -> crate::states::FunctionDef {
        crate::states::FunctionDef {
            name: name.to_string(),
            schema: None,
            arguments: String::new(),
            returns: "void".to_string(),
            language: "sql".to_string(),
            body: "SELECT 1".to_string(),
            volatility: crate::states::Volatility::Volatile,
            security_definer: false,
        }
    }

    fn basic_trigger(name: &str) -> crate::states::TriggerDef {
        crate::states::TriggerDef {
            name: Some(name.to_string()),
            timing: crate::states::TriggerTiming::After,
            events: vec![crate::states::TriggerEvent::Insert],
            scope: crate::states::TriggerScope::Row,
            function_name: Some("audit_fn".to_string()),
            when: None,
            body: None,
            language: None,
        }
    }

    /// Volatile function omits the volatility keyword.
    #[test]
    fn create_function_volatile_no_keyword() {
        let sql = operation_to_sql(&Operation::CreateFunction { function: basic_function("notify") }).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("CREATE OR REPLACE FUNCTION"), "got: {}", sql[0]);
        assert!(!sql[0].contains("VOLATILE"), "should not contain VOLATILE");
        assert!(sql[0].contains("SELECT 1"), "should contain body");
    }

    /// Stable function includes the STABLE keyword.
    #[test]
    fn create_function_stable_keyword() {
        let mut f = basic_function("get_config");
        f.volatility = crate::states::Volatility::Stable;
        let sql = operation_to_sql(&Operation::CreateFunction { function: f }).unwrap();
        assert!(sql[0].contains("STABLE"), "got: {}", sql[0]);
    }

    /// security_definer function includes SECURITY DEFINER.
    #[test]
    fn create_function_security_definer() {
        let mut f = basic_function("run_as_owner");
        f.security_definer = true;
        let sql = operation_to_sql(&Operation::CreateFunction { function: f }).unwrap();
        assert!(sql[0].contains("SECURITY DEFINER"), "got: {}", sql[0]);
    }

    /// DropFunction SQL includes parenthesized arguments.
    #[test]
    fn drop_function_includes_args() {
        let mut f = basic_function("process");
        f.arguments = "user_id integer".to_string();
        let sql = operation_to_sql(&Operation::DropFunction { function: f }).unwrap();
        assert_eq!(sql, vec!["DROP FUNCTION \"process\"(user_id integer)"]);
    }

    /// AlterFunction with same arguments produces a single CREATE OR REPLACE.
    #[test]
    fn alter_function_same_args_produces_replace() {
        let old = basic_function("notify");
        let mut new = basic_function("notify");
        new.body = "SELECT 2".to_string();
        let sql = operation_to_sql(&Operation::AlterFunction { old, new }).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].starts_with("CREATE OR REPLACE FUNCTION"), "got: {}", sql[0]);
    }

    /// AlterFunction with different arguments produces DROP + CREATE (two statements).
    #[test]
    fn alter_function_different_args_produces_drop_and_create() {
        let old = basic_function("process");
        let mut new = basic_function("process");
        new.arguments = "user_id integer".to_string();
        let sql = operation_to_sql(&Operation::AlterFunction { old, new }).unwrap();
        assert_eq!(sql.len(), 2);
        assert!(sql[0].starts_with("DROP FUNCTION"), "got: {}", sql[0]);
        assert!(sql[1].starts_with("CREATE OR REPLACE FUNCTION"), "got: {}", sql[1]);
    }

    /// CreateTrigger SQL has correct BEFORE/AFTER and FOR EACH ROW.
    #[test]
    fn create_trigger_sql() {
        let sql = operation_to_sql(&Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        }).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].contains("CREATE OR REPLACE TRIGGER"), "got: {}", sql[0]);
        assert!(sql[0].contains("AFTER"), "got: {}", sql[0]);
        assert!(sql[0].contains("INSERT"), "got: {}", sql[0]);
        assert!(sql[0].contains("FOR EACH ROW"), "got: {}", sql[0]);
        assert!(sql[0].contains("EXECUTE FUNCTION"), "got: {}", sql[0]);
    }

    /// AlterTrigger SQL is CREATE OR REPLACE TRIGGER (PG14+).
    #[test]
    fn alter_trigger_sql_is_create_or_replace() {
        let old = basic_trigger("audit_trg");
        let mut new = basic_trigger("audit_trg");
        new.function_name = Some("new_fn".to_string());
        let sql = operation_to_sql(&Operation::AlterTrigger {
            table_name: "users".to_string(),
            old,
            new,
        }).unwrap();
        assert_eq!(sql.len(), 1);
        assert!(sql[0].starts_with("CREATE OR REPLACE TRIGGER"), "got: {}", sql[0]);
        assert!(sql[0].contains("\"new_fn\""), "got: {}", sql[0]);
    }

    /// DropTrigger SQL includes the table name.
    #[test]
    fn drop_trigger_sql() {
        let sql = operation_to_sql(&Operation::DropTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        }).unwrap();
        assert_eq!(sql, vec!["DROP TRIGGER \"audit_trg\" ON \"users\""]);
    }
}
