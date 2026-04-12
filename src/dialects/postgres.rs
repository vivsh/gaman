use crate::operations::Operation;
use crate::states::{Column, Constraint, Table, ViewDef, Volatility};

use super::DialectError;

pub fn normalize_type(t: &str) -> &str {
    match t {
        "int2"              => "smallint",
        "int4"              => "integer",
        "int8"              => "bigint",
        "bool"              => "boolean",
        "float4"            => "real",
        "float8"            => "double precision",
        "bpchar"            => "char",
        "character varying" => "varchar",
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

fn qualified_table(table: &Table) -> String {
    match table.schema.as_deref() {
        None | Some("public") => quote_ident(&table.name),
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(&table.name)),
    }
}

fn qualified_view(view: &ViewDef) -> String {
    match view.schema.as_deref() {
        None | Some("public") => quote_ident(&view.name),
        Some(s) => format!("{}.{}", quote_ident(s), quote_ident(&view.name)),
    }
}

pub fn operation_to_sql(op: &Operation) -> Result<Vec<String>, DialectError> {
    let stmts = match op {
        Operation::CreateTable { table } => {
            let mut parts: Vec<String> = table.columns.iter().map(col_def).collect();
            for fk in &table.foreign_keys {
                parts.push(format!(
                    "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                    quote_ident(&fk.name),
                    quote_ident(&fk.from_column),
                    quote_table_name(&fk.to_table),
                    quote_ident(&fk.to_column),
                ));
            }
            for c in &table.constraints {
                parts.push(format!("CONSTRAINT {}", inline_constraint_def(c)));
            }
            let mut stmts = vec![format!("CREATE TABLE {} ({})", qualified_table(table), parts.join(", "))];
            for index in &table.indexes {
                let unique = if index.unique { "UNIQUE " } else { "" };
                let cols: Vec<String> = index.columns.iter().map(|c| quote_ident(c)).collect();
                stmts.push(format!("CREATE {}INDEX {} ON {} ({})", unique, quote_ident(&index.name), qualified_table(table), cols.join(", ")));
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
            vec![format!(
                "ALTER TABLE {} ADD CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
                quote_table_name(table_name),
                quote_ident(&foreign_key.name),
                quote_ident(&foreign_key.from_column),
                quote_table_name(&foreign_key.to_table),
                quote_ident(&foreign_key.to_column),
            )]
        }
        Operation::DropForeignKey { table_name, foreign_key, cascade } => {
            let suffix = if *cascade { " CASCADE" } else { "" };
            vec![format!("ALTER TABLE {} DROP CONSTRAINT {}{}", quote_table_name(table_name), quote_ident(&foreign_key.name), suffix)]
        }
        Operation::AddIndex { table_name, index } => {
            let unique = if index.unique { "UNIQUE " } else { "" };
            let cols: Vec<String> = index.columns.iter().map(|c| quote_ident(c)).collect();
            vec![format!("CREATE {}INDEX {} ON {} ({})", unique, quote_ident(&index.name), quote_table_name(table_name), cols.join(", "))]
        }
        Operation::DropIndex { index, .. } => {
            vec![format!("DROP INDEX {}", quote_ident(&index.name))]
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
            vec![create_function_sql(function)]
        }
        Operation::DropFunction { function } => {
            vec![format!("DROP FUNCTION {}({})", quote_ident(&function.name), function.arguments)]
        }
        Operation::AlterFunction { old, new } => {
            if old.arguments == new.arguments {
                vec![create_function_sql(new)]
            } else {
                eprintln!(
                    "warning: function '{}' argument signature changed — dropping old and creating new; \
                     existing callers referencing the old signature will break",
                    old.name
                );
                vec![
                    format!("DROP FUNCTION {}({})", quote_ident(&old.name), old.arguments),
                    create_function_sql(new),
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
    };
    Ok(stmts)
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

fn create_function_sql(f: &crate::states::FunctionDef) -> String {
    let vol = match f.volatility {
        Volatility::Volatile => "",
        Volatility::Stable => "\nSTABLE",
        Volatility::Immutable => "\nIMMUTABLE",
    };
    let sec = if f.security_definer { "\nSECURITY DEFINER" } else { "" };
    format!(
        "CREATE OR REPLACE FUNCTION {}({})\nRETURNS {}\nLANGUAGE {}{}{}
AS $func$\n{}\n$func$",
        quote_ident(&f.name), f.arguments, f.returns, f.language, vol, sec, f.body
    )
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
        let sql = operation_to_sql(&Operation::AddIndex { table_name: "users".to_string(), index }).unwrap();
        assert_eq!(sql, vec!["CREATE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]);
    }

    #[test]
    fn add_unique_index_sql() {
        let index = Index { name: "users_email_idx".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None };
        let sql = operation_to_sql(&Operation::AddIndex { table_name: "users".to_string(), index }).unwrap();
        assert_eq!(sql, vec!["CREATE UNIQUE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]);
    }

    #[test]
    fn drop_index_sql() {
        let index = Index { name: "users_email_idx".to_string(), columns: vec!["email".to_string()], unique: false, predicate: None };
        let sql = operation_to_sql(&Operation::DropIndex { table_name: "users".to_string(), index }).unwrap();
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
