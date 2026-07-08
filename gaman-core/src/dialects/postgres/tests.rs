use super::*;
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, Index, PrimaryKey, Schema, Table,
    ViewDef,
};

fn operation_to_sql(op: &Operation) -> Result<Vec<String>, DialectError> {
    let atomic = match op {
        Operation::AddIndex { concurrent, .. } | Operation::DropIndex { concurrent, .. } => {
            !concurrent
        }
        _ => true,
    };
    let migration = Migration {
        id: "test_render".to_string(),
        dependencies: vec![],
        operations: vec![op.clone()],
        atomic,
    };
    POSTGRES.migration_to_sql(&migration, &Schema::default())
}

fn col(name: &str, t: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: t.to_string(),
        nullable: false,
        primary_key: false,
        default: None,
        ..Default::default()
    }
}

fn nullable_col(name: &str, t: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: t.to_string(),
        nullable: true,
        primary_key: false,
        default: None,
        ..Default::default()
    }
}

fn empty_table(name: &str) -> Table {
    Table {
        name: name.to_string(),
        schema: None,
        primary_key: None,
        columns: vec![],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    }
}

fn enum_def(values: &[&str]) -> EnumDef {
    EnumDef {
        name: "status".to_string(),
        schema: None,
        values: values.iter().map(|value| value.to_string()).collect(),
    }
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

mod property_identifiers {
    use super::*;

    use proptest::prelude::*;

    fn arb_identifier() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-z][a-z0-9_]{0,10}".prop_map(|value| value),
            "[A-Z][A-Za-z0-9_]{0,10}".prop_map(|value| value),
            "[a-z]{1,8}_[0-9]{1,3}".prop_map(|value| value),
            (Just("quoted_"), "[a-z]{1,6}")
                .prop_map(|(prefix, suffix)| format!("{prefix}\"{suffix}")),
            Just("user".to_string()),
            Just("order".to_string()),
            Just("very_long_identifier_name_for_property_tests".to_string()),
        ]
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        #[doc = "PostgreSQL identifier quoting wraps generated identifiers and doubles embedded quotes."]
        fn quote_ident_escapes_generated_identifiers(identifier in arb_identifier()) {
            let escaped = identifier.replace('"', "\"\"");
            prop_assert_eq!(quote_ident(&identifier), format!("\"{escaped}\""));
        }

        #[test]
        #[doc = "Schema-qualified PostgreSQL create-table rendering quotes schema and table separately."]
        fn schema_qualified_create_table_quotes_parts_separately(
            schema in arb_identifier(),
            table_name in arb_identifier(),
        ) {
            let mut table = empty_table(&table_name);
            table.schema = Some(schema.clone());
            table.columns.push(col("id", "integer"));
            let sql = operation_to_sql(&Operation::CreateTable { table })
                .expect("create table should render")
                .join("\n");
            let qualified = format!(
                "{}.{}",
                quote_ident(&schema),
                quote_ident(&table_name)
            );
            let incorrectly_joined = quote_ident(&format!("{}.{}", schema, table_name));
            prop_assert!(
                sql.contains(&qualified),
                "SQL did not contain {}: {}",
                qualified,
                sql
            );
            prop_assert!(!sql.contains(&incorrectly_joined));
        }
    }
}

#[test]
fn postgres_type_catalog_canonicalizes_aliases_and_suggests_typos() {
    assert_eq!(canonical_type("int4"), "integer");
    assert_eq!(canonical_type("timestamptz"), "timestamp with time zone");
    assert!(is_catalog_type("citext"));
    assert!(is_catalog_type("vector(1536)"));
    assert_eq!(canonical_type("project_code"), "project_code");
    assert!(type_suggestions("intger").contains(&"integer".to_string()));
}

#[test]
fn create_table_basic() {
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col("id", "serial"), col("name", "text")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let sql = operation_to_sql(&Operation::CreateTable { table }).unwrap();
    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].starts_with("CREATE TABLE \"users\" ("),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("\"id\" serial"), "got: {}", sql[0]);
    assert!(sql[0].contains("\"name\" text"), "got: {}", sql[0]);
}

#[test]
fn create_table_composite_primary_key() {
    let table = Table {
        name: "order_lines".to_string(),
        schema: None,
        primary_key: Some(PrimaryKey {
            name: "order_lines_identity".to_string(),
            columns: vec!["tenant_id".to_string(), "order_id".to_string()],
        }),
        columns: vec![col("order_id", "bigint"), col("tenant_id", "bigint")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };

    let sql = operation_to_sql(&Operation::CreateTable { table }).unwrap();

    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains(
            "CONSTRAINT \"order_lines_identity\" PRIMARY KEY (\"tenant_id\", \"order_id\")"
        )
    );
    assert!(!sql[0].contains("\"tenant_id\" bigint PRIMARY KEY"));
}

#[test]
fn create_table_reserved_word_name() {
    let table = Table {
        name: "order".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col("id", "serial")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let sql = operation_to_sql(&Operation::CreateTable { table }).unwrap();
    assert!(
        sql[0].starts_with("CREATE TABLE \"order\" ("),
        "got: {}",
        sql[0]
    );
}

#[test]
fn drop_table_sql() {
    let sql = operation_to_sql(&Operation::DropTable {
        table: empty_table("users"),
    })
    .unwrap();
    assert_eq!(sql, vec!["DROP TABLE \"users\""]);
}

#[test]
fn rename_table_sql() {
    let sql = operation_to_sql(&Operation::RenameTable {
        old_name: "users".to_string(),
        new_name: "accounts".to_string(),
    })
    .unwrap();
    assert_eq!(sql, vec!["ALTER TABLE \"users\" RENAME TO \"accounts\""]);
}

#[test]
fn add_column_sql() {
    let sql = operation_to_sql(&Operation::AddColumn {
        table_name: "users".to_string(),
        column: col("email", "text"),
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ADD COLUMN \"email\" text NOT NULL"]
    );
}

#[test]
fn drop_column_no_cascade() {
    let sql = operation_to_sql(&Operation::DropColumn {
        table_name: "users".to_string(),
        column: col("email", "text"),
        cascade: false,
    })
    .unwrap();
    assert_eq!(sql, vec!["ALTER TABLE \"users\" DROP COLUMN \"email\""]);
}

#[test]
fn drop_column_with_cascade() {
    let sql = operation_to_sql(&Operation::DropColumn {
        table_name: "users".to_string(),
        column: col("email", "text"),
        cascade: true,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" DROP COLUMN \"email\" CASCADE"]
    );
}

#[test]
fn alter_column_type_no_cast() {
    let old = col("status", "varchar(50)");
    let new = col("status", "text");
    let sql = operation_to_sql(&Operation::AlterColumn {
        table_name: "users".to_string(),
        old,
        new,
        cast_expr: None,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ALTER COLUMN \"status\" TYPE text"]
    );
}

#[test]
fn alter_column_type_with_cast() {
    let old = col("age", "text");
    let new = col("age", "integer");
    let sql = operation_to_sql(&Operation::AlterColumn {
        table_name: "users".to_string(),
        old,
        new,
        cast_expr: Some("age::integer".to_string()),
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ALTER COLUMN \"age\" TYPE integer USING age::integer"]
    );
}

#[test]
fn alter_column_nullable_change() {
    let old = col("email", "text");
    let new = nullable_col("email", "text");
    let sql = operation_to_sql(&Operation::AlterColumn {
        table_name: "users".to_string(),
        old,
        new,
        cast_expr: None,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ALTER COLUMN \"email\" DROP NOT NULL"]
    );
}

#[test]
fn alter_column_drops_schema_qualified_primary_key_by_bare_table_name() {
    let mut old = col("id", "integer");
    old.primary_key = true;
    let new = col("id", "integer");
    let sql = operation_to_sql(&Operation::AlterColumn {
        table_name: "app.users".to_string(),
        old,
        new,
        cast_expr: None,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"app\".\"users\" DROP CONSTRAINT \"users_pkey\""]
    );
}

#[test]
fn add_foreign_key_sql() {
    let fk = ForeignKey::single("posts_user_id_fkey", "user_id", "users", "id");
    let sql = operation_to_sql(&Operation::AddForeignKey {
        table_name: "posts".to_string(),
        foreign_key: fk,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"posts\" ADD CONSTRAINT \"posts_user_id_fkey\" FOREIGN KEY (\"user_id\") REFERENCES \"users\" (\"id\")"
        ]
    );
}

#[test]
fn add_composite_foreign_key_sql() {
    let fk = ForeignKey::new(
        "orders_user_fkey",
        ["tenant_id", "user_id"],
        "users",
        ["tenant_id", "id"],
    );
    let sql = operation_to_sql(&Operation::AddForeignKey {
        table_name: "orders".to_string(),
        foreign_key: fk,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec![
            "ALTER TABLE \"orders\" ADD CONSTRAINT \"orders_user_fkey\" FOREIGN KEY (\"tenant_id\", \"user_id\") REFERENCES \"users\" (\"tenant_id\", \"id\")"
        ]
    );
}

#[test]
fn drop_foreign_key_no_cascade() {
    let fk = ForeignKey::single("posts_user_id_fkey", "user_id", "users", "id");
    let sql = operation_to_sql(&Operation::DropForeignKey {
        table_name: "posts".to_string(),
        foreign_key: fk,
        cascade: false,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"posts\" DROP CONSTRAINT \"posts_user_id_fkey\""]
    );
}

#[test]
fn drop_foreign_key_with_cascade() {
    let fk = ForeignKey::single("posts_user_id_fkey", "user_id", "users", "id");
    let sql = operation_to_sql(&Operation::DropForeignKey {
        table_name: "posts".to_string(),
        foreign_key: fk,
        cascade: true,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TABLE \"posts\" DROP CONSTRAINT \"posts_user_id_fkey\" CASCADE"]
    );
}

#[test]
fn add_index_sql() {
    let index = Index {
        name: "users_email_idx".to_string(),
        columns: vec!["email".to_string()],
        unique: false,
        predicate: None,
    };
    let sql = operation_to_sql(&Operation::AddIndex {
        table_name: "users".to_string(),
        index,
        concurrent: false,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["CREATE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]
    );
}

#[test]
fn add_unique_index_sql() {
    let index = Index {
        name: "users_email_idx".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
        predicate: None,
    };
    let sql = operation_to_sql(&Operation::AddIndex {
        table_name: "users".to_string(),
        index,
        concurrent: false,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["CREATE UNIQUE INDEX \"users_email_idx\" ON \"users\" (\"email\")"]
    );
}

/// Renders partial and concurrent indexes as one exact PostgreSQL statement.
#[test]
fn add_partial_concurrent_index_sql() {
    let index = Index {
        name: "users_email_active_idx".to_string(),
        columns: vec!["email".to_string()],
        unique: false,
        predicate: Some("deleted_at IS NULL".to_string()),
    };

    let sql = operation_to_sql(&Operation::AddIndex {
        table_name: "app.users".to_string(),
        index,
        concurrent: true,
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE INDEX CONCURRENTLY \"users_email_active_idx\" ON \"app\".\"users\" (\"email\") WHERE deleted_at IS NULL"
        ]
    );
}

/// Renders table-level unique constraints exactly.
#[test]
fn add_unique_constraint_sql() {
    let sql = operation_to_sql(&Operation::AddConstraint {
        table_name: "users".to_string(),
        constraint: Constraint::Unique {
            name: "users_email_key".to_string(),
            columns: vec!["email".to_string()],
        },
    })
    .unwrap();

    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ADD CONSTRAINT \"users_email_key\" UNIQUE (\"email\")"]
    );
}

/// Renders table-level check constraints exactly.
#[test]
fn add_check_constraint_sql() {
    let sql = operation_to_sql(&Operation::AddConstraint {
        table_name: "users".to_string(),
        constraint: Constraint::Check {
            name: "users_age_check".to_string(),
            expression: "age >= 0".to_string(),
        },
    })
    .unwrap();

    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" ADD CONSTRAINT \"users_age_check\" CHECK (age >= 0)"]
    );
}

/// Drops named constraints exactly.
#[test]
fn drop_constraint_sql() {
    let sql = operation_to_sql(&Operation::DropConstraint {
        table_name: "users".to_string(),
        constraint: Constraint::Check {
            name: "users_age_check".to_string(),
            expression: "age >= 0".to_string(),
        },
    })
    .unwrap();

    assert_eq!(
        sql,
        vec!["ALTER TABLE \"users\" DROP CONSTRAINT \"users_age_check\""]
    );
}

#[test]
fn drop_index_sql() {
    let index = Index {
        name: "users_email_idx".to_string(),
        columns: vec!["email".to_string()],
        unique: false,
        predicate: None,
    };
    let sql = operation_to_sql(&Operation::DropIndex {
        table_name: "users".to_string(),
        index,
        concurrent: false,
    })
    .unwrap();
    assert_eq!(sql, vec!["DROP INDEX \"users_email_idx\""]);
}

#[test]
fn drop_index_sql_schema_qualifies_index_from_table_schema() {
    let index = Index {
        name: "users_email_idx".to_string(),
        columns: vec!["email".to_string()],
        unique: false,
        predicate: None,
    };
    let sql = operation_to_sql(&Operation::DropIndex {
        table_name: "app.users".to_string(),
        index,
        concurrent: true,
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["DROP INDEX CONCURRENTLY \"app\".\"users_email_idx\""]
    );
}

/// Renders raw SQL statements exactly as authored.
#[test]
fn statement_sql_is_preserved() {
    let sql = operation_to_sql(&Operation::Statement {
        up: "UPDATE users SET active = true".to_string(),
        down: Some("UPDATE users SET active = false".to_string()),
    })
    .unwrap();

    assert_eq!(sql, vec!["UPDATE users SET active = true"]);
}

/// Renders extension creation with optional schema and version.
#[test]
fn create_extension_sql() {
    let sql = operation_to_sql(&Operation::CreateExtension {
        extension: ExtensionDef {
            name: "pgcrypto".to_string(),
            schema: Some("extensions".to_string()),
            version: Some("1.3".to_string()),
        },
    })
    .unwrap();

    assert_eq!(
        sql,
        vec!["CREATE EXTENSION IF NOT EXISTS \"pgcrypto\" SCHEMA \"extensions\" VERSION '1.3'"]
    );
}

/// Renders extension drops exactly.
#[test]
fn drop_extension_sql() {
    let sql = operation_to_sql(&Operation::DropExtension {
        extension: ExtensionDef {
            name: "pgcrypto".to_string(),
            schema: None,
            version: None,
        },
    })
    .unwrap();

    assert_eq!(sql, vec!["DROP EXTENSION \"pgcrypto\""]);
}

/// Renders enum creation with schema qualification and quoted labels.
#[test]
fn create_enum_sql() {
    let mut enum_def = enum_def(&["draft", "published"]);
    enum_def.schema = Some("app".to_string());

    let sql = operation_to_sql(&Operation::CreateEnum { enum_def }).unwrap();

    assert_eq!(
        sql,
        vec!["CREATE TYPE \"app\".\"status\" AS ENUM ('draft', 'published')"]
    );
}

/// Renders enum drops with schema qualification.
#[test]
fn drop_enum_sql() {
    let mut enum_def = enum_def(&["draft", "published"]);
    enum_def.schema = Some("app".to_string());

    let sql = operation_to_sql(&Operation::DropEnum { enum_def }).unwrap();

    assert_eq!(sql, vec!["DROP TYPE \"app\".\"status\""]);
}

#[test]
fn alter_enum_append_only_adds_values() {
    let sql = operation_to_sql(&Operation::AlterEnum {
        old: enum_def(&["draft", "published"]),
        new: enum_def(&["draft", "published", "archived"]),
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TYPE \"status\" ADD VALUE 'archived' AFTER 'published'"]
    );
}

#[test]
fn alter_enum_inserted_value_uses_before_or_after() {
    let sql = operation_to_sql(&Operation::AlterEnum {
        old: enum_def(&["draft", "published"]),
        new: enum_def(&["queued", "draft", "review", "published"]),
    })
    .unwrap();
    assert_eq!(
        sql,
        vec![
            "ALTER TYPE \"status\" ADD VALUE 'queued' BEFORE 'draft'",
            "ALTER TYPE \"status\" ADD VALUE 'review' AFTER 'draft'",
        ]
    );
}

#[test]
fn rename_enum_value_sql() {
    let sql = operation_to_sql(&Operation::RenameEnumValue {
        enum_name: "status".to_string(),
        schema: Some("app".to_string()),
        old_value: "live".to_string(),
        new_value: "published".to_string(),
    })
    .unwrap();
    assert_eq!(
        sql,
        vec!["ALTER TYPE \"app\".\"status\" RENAME VALUE 'live' TO 'published'"]
    );
}

#[test]
fn alter_enum_value_removal_is_unsupported() {
    let err = operation_to_sql(&Operation::AlterEnum {
        old: enum_def(&["draft", "published", "archived"]),
        new: enum_def(&["draft", "published"]),
    })
    .unwrap_err();
    assert!(err.to_string().contains("cannot remove enum values"));
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
        query: None,
        language: None,
    }
}

/// Volatile function omits the volatility keyword.
#[test]
fn create_function_volatile_no_keyword() {
    let sql = operation_to_sql(&Operation::CreateFunction {
        function: basic_function("notify"),
    })
    .unwrap();
    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains("CREATE OR REPLACE FUNCTION"),
        "got: {}",
        sql[0]
    );
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

/// Renders function creation exactly for non-default volatility and security settings.
#[test]
fn create_function_exact_sql() {
    let mut f = basic_function("get_config");
    f.schema = Some("app".to_string());
    f.arguments = "key text".to_string();
    f.returns = "text".to_string();
    f.body = "SELECT key".to_string();
    f.volatility = crate::states::Volatility::Stable;
    f.security_definer = true;

    let sql = operation_to_sql(&Operation::CreateFunction { function: f }).unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE OR REPLACE FUNCTION \"app\".\"get_config\"(key text)\nRETURNS text\nLANGUAGE sql\nSTABLE\nSECURITY DEFINER\nAS $func$\nSELECT key\n$func$"
        ]
    );
}

#[test]
fn custom_schema_function_sql_is_qualified() {
    let mut f = basic_function("process");
    f.schema = Some("app".to_string());
    f.arguments = "user_id integer".to_string();

    let create_sql = operation_to_sql(&Operation::CreateFunction {
        function: f.clone(),
    })
    .unwrap();
    assert!(create_sql[0].starts_with("CREATE OR REPLACE FUNCTION \"app\".\"process\"("));

    let drop_sql = operation_to_sql(&Operation::DropFunction { function: f }).unwrap();
    assert_eq!(
        drop_sql,
        vec!["DROP FUNCTION \"app\".\"process\"(user_id integer)"]
    );
}

/// AlterFunction with same arguments produces a single CREATE OR REPLACE.
#[test]
fn alter_function_same_args_produces_replace() {
    let old = basic_function("notify");
    let mut new = basic_function("notify");
    new.body = "SELECT 2".to_string();
    let sql = operation_to_sql(&Operation::AlterFunction { old, new }).unwrap();
    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].starts_with("CREATE OR REPLACE FUNCTION"),
        "got: {}",
        sql[0]
    );
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
    assert!(
        sql[1].starts_with("CREATE OR REPLACE FUNCTION"),
        "got: {}",
        sql[1]
    );
}

#[test]
fn alter_function_different_args_preserves_schema() {
    let mut old = basic_function("process");
    old.schema = Some("app".to_string());
    let mut new = old.clone();
    new.arguments = "user_id integer".to_string();

    let sql = operation_to_sql(&Operation::AlterFunction { old, new }).unwrap();

    assert_eq!(sql[0], "DROP FUNCTION \"app\".\"process\"()");
    assert!(sql[1].starts_with("CREATE OR REPLACE FUNCTION \"app\".\"process\"("));
}

/// Renders function alteration with changed signature as drop plus create.
#[test]
fn alter_function_different_args_exact_sql() {
    let old = basic_function("process");
    let mut new = basic_function("process");
    new.arguments = "user_id integer".to_string();
    new.body = "SELECT user_id".to_string();

    let sql = operation_to_sql(&Operation::AlterFunction { old, new }).unwrap();

    assert_eq!(
        sql,
        vec![
            "DROP FUNCTION \"process\"()",
            "CREATE OR REPLACE FUNCTION \"process\"(user_id integer)\nRETURNS void\nLANGUAGE sql\nAS $func$\nSELECT user_id\n$func$",
        ]
    );
}

/// CreateTrigger SQL has correct BEFORE/AFTER and FOR EACH ROW.
#[test]
fn create_trigger_sql() {
    let sql = operation_to_sql(&Operation::CreateTrigger {
        table_name: "users".to_string(),
        trigger: basic_trigger("audit_trg"),
    })
    .unwrap();
    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains("CREATE OR REPLACE TRIGGER"),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("AFTER"), "got: {}", sql[0]);
    assert!(sql[0].contains("INSERT"), "got: {}", sql[0]);
    assert!(sql[0].contains("FOR EACH ROW"), "got: {}", sql[0]);
    assert!(sql[0].contains("EXECUTE FUNCTION"), "got: {}", sql[0]);
}

#[test]
fn create_trigger_sql_qualifies_function_name() {
    let mut trigger = basic_trigger("audit_trg");
    trigger.function_name = Some("audit.audit_fn".to_string());

    let sql = operation_to_sql(&Operation::CreateTrigger {
        table_name: "app.users".to_string(),
        trigger,
    })
    .unwrap();

    assert!(sql[0].contains("ON \"app\".\"users\""), "got: {}", sql[0]);
    assert!(
        sql[0].contains("EXECUTE FUNCTION \"audit\".\"audit_fn\"()"),
        "got: {}",
        sql[0]
    );
}

/// Query triggers render a generated trigger function followed by the trigger.
#[test]
fn create_query_trigger_sql_generates_row_function() {
    let mut trigger = basic_trigger("audit_trg");
    trigger.function_name = None;
    trigger.query = Some("INSERT INTO audit_log(user_id) VALUES (NEW.id);".to_string());

    let sql = operation_to_sql(&Operation::CreateTrigger {
        table_name: "users".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(sql.len(), 2);
    assert!(
        sql[0].starts_with("CREATE OR REPLACE FUNCTION \"audit_trg_fn\"()"),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("RETURNS trigger"), "got: {}", sql[0]);
    assert!(sql[0].contains("LANGUAGE plpgsql"), "got: {}", sql[0]);
    assert!(
        sql[0].contains("INSERT INTO audit_log(user_id) VALUES (NEW.id);"),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("RETURN NEW;"), "got: {}", sql[0]);
    assert!(
        sql[1].contains("EXECUTE FUNCTION \"audit_trg_fn\"()"),
        "got: {}",
        sql[1]
    );
}

/// Renders query triggers as a generated function plus trigger SQL.
#[test]
fn create_query_trigger_exact_sql() {
    let mut trigger = basic_trigger("audit_trg");
    trigger.function_name = None;
    trigger.query = Some("INSERT INTO audit_log(user_id) VALUES (NEW.id);".to_string());

    let sql = operation_to_sql(&Operation::CreateTrigger {
        table_name: "users".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE OR REPLACE FUNCTION \"audit_trg_fn\"()\nRETURNS trigger\nLANGUAGE plpgsql\nAS $func$\nBEGIN\nINSERT INTO audit_log(user_id) VALUES (NEW.id);\nRETURN NEW;\nEND;\n$func$",
            "CREATE OR REPLACE TRIGGER \"audit_trg\"\nAFTER INSERT\nON \"users\"\nFOR EACH ROW\nEXECUTE FUNCTION \"audit_trg_fn\"()",
        ]
    );
}

/// Statement-level query triggers use RETURN NULL in generated PostgreSQL functions.
#[test]
fn create_query_trigger_sql_generates_statement_function() {
    let mut trigger = basic_trigger("audit_trg");
    trigger.function_name = None;
    trigger.query = Some("INSERT INTO audit_log(action) VALUES ('bulk');".to_string());
    trigger.scope = crate::states::TriggerScope::Statement;
    trigger.language = Some("plpgsql".to_string());

    let sql = operation_to_sql(&Operation::CreateTrigger {
        table_name: "users".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(sql.len(), 2);
    assert!(sql[0].contains("RETURN NULL;"), "got: {}", sql[0]);
    assert!(sql[1].contains("FOR EACH STATEMENT"), "got: {}", sql[1]);
}

/// Dropping query triggers also drops their generated PostgreSQL trigger function.
#[test]
fn drop_query_trigger_sql_drops_generated_function() {
    let mut trigger = basic_trigger("audit_trg");
    trigger.function_name = None;
    trigger.query = Some("INSERT INTO audit_log(user_id) VALUES (OLD.id);".to_string());

    let sql = operation_to_sql(&Operation::DropTrigger {
        table_name: "users".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            "DROP TRIGGER \"audit_trg\" ON \"users\"",
            "DROP FUNCTION \"audit_trg_fn\"()"
        ]
    );
}

/// Switching from query to function trigger drops the old generated function.
#[test]
fn alter_query_trigger_to_function_drops_generated_function() {
    let mut old = basic_trigger("audit_trg");
    old.function_name = None;
    old.query = Some("INSERT INTO audit_log(user_id) VALUES (NEW.id);".to_string());
    let new = basic_trigger("audit_trg");

    let sql = operation_to_sql(&Operation::AlterTrigger {
        table_name: "users".to_string(),
        old,
        new,
    })
    .unwrap();

    assert_eq!(sql.len(), 2);
    assert!(sql[0].contains("EXECUTE FUNCTION \"audit_fn\"()"));
    assert_eq!(sql[1], "DROP FUNCTION \"audit_trg_fn\"()");
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
    })
    .unwrap();
    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].starts_with("CREATE OR REPLACE TRIGGER"),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("\"new_fn\""), "got: {}", sql[0]);
}

/// DropTrigger SQL includes the table name.
#[test]
fn drop_trigger_sql() {
    let sql = operation_to_sql(&Operation::DropTrigger {
        table_name: "users".to_string(),
        trigger: basic_trigger("audit_trg"),
    })
    .unwrap();
    assert_eq!(sql, vec!["DROP TRIGGER \"audit_trg\" ON \"users\""]);
}

/// Renders view creation exactly.
#[test]
fn create_view_sql() {
    let sql = operation_to_sql(&Operation::CreateView {
        view: ViewDef {
            name: "active_users".to_string(),
            schema: Some("app".to_string()),
            definition: "SELECT id FROM app.users WHERE active".to_string(),
        },
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE OR REPLACE VIEW \"app\".\"active_users\" AS SELECT id FROM app.users WHERE active"
        ]
    );
}

/// Renders view drops exactly.
#[test]
fn drop_view_sql() {
    let sql = operation_to_sql(&Operation::DropView {
        view: ViewDef {
            name: "active_users".to_string(),
            schema: Some("app".to_string()),
            definition: String::new(),
        },
    })
    .unwrap();

    assert_eq!(sql, vec!["DROP VIEW \"app\".\"active_users\""]);
}

/// Renders view replacement as CREATE OR REPLACE of the target view.
#[test]
fn replace_view_sql() {
    let old = ViewDef {
        name: "active_users".to_string(),
        schema: Some("app".to_string()),
        definition: "SELECT id FROM app.users".to_string(),
    };
    let new = ViewDef {
        name: "active_users".to_string(),
        schema: Some("app".to_string()),
        definition: "SELECT id FROM app.users WHERE active".to_string(),
    };

    let sql = operation_to_sql(&Operation::ReplaceView { old, new }).unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE OR REPLACE VIEW \"app\".\"active_users\" AS SELECT id FROM app.users WHERE active"
        ]
    );
}
