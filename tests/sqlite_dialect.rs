#![cfg(feature = "sqlite")]

use std::sync::Arc;

use gaman::core::{
    BoxFuture, Dialect, DialectError, Environment, EnvironmentError, EnvironmentExecutor, Executor,
    Migrator, SqliteExecutor, TRACKING_TABLE, VecAdapter,
};
use gaman::schema::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, Operation,
    PrimaryKey, Schema, Table, TriggerDef, TriggerEvent, TriggerScope, TriggerTiming, ViewDef,
    Volatility,
};
use gaman::{Config, Migration};
use sqlx::ConnectOptions;

fn operation_to_sql(operation: Operation) -> Result<Vec<String>, DialectError> {
    let start = start_schema_for_operation(&operation);
    migration_operation_to_sql(operation, &start)
}

fn operation_to_sql_without_start(operation: Operation) -> Result<Vec<String>, DialectError> {
    migration_operation_to_sql(operation, &Schema::default())
}

fn migration_operation_to_sql(
    operation: Operation,
    start: &Schema,
) -> Result<Vec<String>, DialectError> {
    Dialect::Sqlite.migration_to_sql(
        &Migration {
            id: "test".to_string(),
            dependencies: vec![],
            operations: vec![operation],
            atomic: true,
        },
        start,
    )
}

fn start_schema_for_operation(operation: &Operation) -> Schema {
    let mut schema = Schema::default();
    match operation {
        Operation::DropTable { table } => {
            schema.tables.insert(table.qualified_name(), table.clone());
        }
        Operation::RenameTable { old_name, .. } => {
            let table = table(old_name);
            schema.tables.insert(table.qualified_name(), table);
        }
        Operation::AddColumn { table_name, .. }
        | Operation::AddIndex { table_name, .. }
        | Operation::CreateTrigger { table_name, .. } => {
            let table = table(table_name);
            schema.tables.insert(table.qualified_name(), table);
        }
        Operation::DropIndex {
            table_name, index, ..
        } => {
            let mut table = table(table_name);
            table.indexes.push(index.clone());
            schema.tables.insert(table.qualified_name(), table);
        }
        Operation::AlterTrigger {
            table_name, old, ..
        }
        | Operation::DropTrigger {
            table_name,
            trigger: old,
        } => {
            let mut table = table(table_name);
            table.triggers.push(old.clone());
            schema.tables.insert(table.qualified_name(), table);
        }
        Operation::RenameColumn {
            table_name,
            old_name,
            ..
        } => {
            let mut table = table(table_name);
            table.columns.push(col(old_name, "text", true));
            schema.tables.insert(table.qualified_name(), table);
        }
        Operation::ReplaceView { old, .. } | Operation::DropView { view: old } => {
            schema.views.insert(old.qualified_name(), old.clone());
        }
        _ => {}
    }
    schema
}

fn table(name: &str) -> Table {
    Table {
        name: name.to_string(),
        schema: None,
        primary_key: None,
        columns: vec![Column {
            name: "id".to_string(),
            col_type: "integer".to_string(),
            nullable: false,
            default: None,
            primary_key: true,
            ..Default::default()
        }],
        foreign_keys: vec![],
        indexes: vec![Index {
            name: format!("{name}_id_idx"),
            columns: vec!["id".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        }],
        constraints: vec![Constraint::Check {
            name: format!("{name}_id_check"),
            expression: "id > 0".to_string(),
        }],
        triggers: vec![],
        options: Default::default(),
    }
}

fn col(name: &str, col_type: &str, nullable: bool) -> Column {
    Column {
        name: name.to_string(),
        col_type: col_type.to_string(),
        nullable,
        ..Default::default()
    }
}

fn pk_col(name: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: "integer".to_string(),
        nullable: false,
        primary_key: true,
        ..Default::default()
    }
}

fn migration(id: &str, operations: Vec<Operation>) -> Migration {
    Migration {
        id: id.to_string(),
        dependencies: vec![],
        operations,
        atomic: true,
    }
}

fn migration_after(id: &str, dependency: &str, operations: Vec<Operation>) -> Migration {
    Migration {
        id: id.to_string(),
        dependencies: vec![dependency.to_string()],
        operations,
        atomic: true,
    }
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
        columns: vec![
            col("order_id", "integer", false),
            col("tenant_id", "integer", false),
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };

    let sql = operation_to_sql(Operation::CreateTable { table }).unwrap();

    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains(
            "CONSTRAINT \"order_lines_identity\" PRIMARY KEY (\"tenant_id\", \"order_id\")"
        )
    );
    assert!(!sql[0].contains("\"tenant_id\" integer PRIMARY KEY"));
    assert!(!sql[0].contains("\"order_id\" integer PRIMARY KEY"));
}

#[test]
fn create_table_composite_foreign_key() {
    let table = Table {
        name: "orders".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            col("tenant_id", "integer", false),
            col("user_id", "integer", false),
        ],
        foreign_keys: vec![ForeignKey::new(
            "orders_user_fkey",
            ["tenant_id", "user_id"],
            "users",
            ["tenant_id", "id"],
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };

    let sql = operation_to_sql(Operation::CreateTable { table }).unwrap();

    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains(
            "CONSTRAINT \"orders_user_fkey\" FOREIGN KEY (\"tenant_id\", \"user_id\") REFERENCES \"users\" (\"tenant_id\", \"id\")"
        ),
        "got: {}",
        sql[0]
    );
}

/// SQLite query triggers render direct CREATE TRIGGER SQL without RETURN NEW.
#[test]
fn create_query_trigger_sqlite() {
    let trigger = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);".to_string()),
        language: None,
        opaque: Default::default(),
    };

    let sql = operation_to_sql(Operation::CreateTrigger {
        table_name: "orders".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains("CREATE TRIGGER \"orders_insert_after_trg\""),
        "got: {}",
        sql[0]
    );
    assert!(sql[0].contains("AFTER INSERT ON \"orders\""));
    assert!(sql[0].contains("FOR EACH ROW"));
    assert!(sql[0].contains("INSERT INTO audit_log(order_id) VALUES (NEW.id);"));
    assert!(!sql[0].contains("RETURN NEW"));
}

/// SQLite rejects PostgreSQL trigger function wiring.
#[test]
fn sqlite_trigger_function_name_is_unsupported() {
    let trigger = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: Some("orders_audit_fn".to_string()),
        when: None,
        query: None,
        language: None,
        opaque: Default::default(),
    };

    let err = operation_to_sql(Operation::CreateTrigger {
        table_name: "orders".to_string(),
        trigger,
    })
    .unwrap_err();
    assert!(err.to_string().contains("function_name"));
}

/// SQLite rejects language on direct query triggers.
#[test]
fn sqlite_trigger_language_is_unsupported() {
    let trigger = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);".to_string()),
        language: Some("plpgsql".to_string()),
        opaque: Default::default(),
    };

    let err = operation_to_sql(Operation::CreateTrigger {
        table_name: "orders".to_string(),
        trigger,
    })
    .unwrap_err();
    assert!(err.to_string().contains("language"));
}

/// SQLite rejects truncate trigger events.
#[test]
fn sqlite_trigger_truncate_is_unsupported() {
    let trigger = TriggerDef {
        name: Some("orders_truncate_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Truncate],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(action) VALUES ('truncate');".to_string()),
        language: None,
        opaque: Default::default(),
    };

    let err = operation_to_sql(Operation::CreateTrigger {
        table_name: "orders".to_string(),
        trigger,
    })
    .unwrap_err();
    assert!(err.to_string().contains("TRUNCATE"));
}

/// SQLite renders trigger alteration as direct CREATE TRIGGER for the new trigger shape.
#[test]
fn sqlite_renders_alter_trigger_as_create_trigger() {
    let old = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(action) VALUES ('old');".to_string()),
        language: None,
        opaque: Default::default(),
    };
    let new = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(action) VALUES ('new');".to_string()),
        language: None,
        opaque: Default::default(),
    };

    let sql = operation_to_sql(Operation::AlterTrigger {
        table_name: "orders".to_string(),
        old,
        new,
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            "CREATE TRIGGER \"orders_insert_after_trg\"\nAFTER INSERT ON \"orders\"\nFOR EACH ROW\nBEGIN\nINSERT INTO audit_log(action) VALUES ('new');\nEND"
        ]
    );
}

/// SQLite renders trigger drops by name.
#[test]
fn sqlite_renders_drop_trigger() {
    let trigger = TriggerDef {
        name: Some("orders_insert_after_trg".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(order_id) VALUES (OLD.id);".to_string()),
        language: None,
        opaque: Default::default(),
    };

    let sql = operation_to_sql(Operation::DropTrigger {
        table_name: "orders".to_string(),
        trigger,
    })
    .unwrap();

    assert_eq!(sql, vec![r#"DROP TRIGGER "orders_insert_after_trg""#]);
}

/// SQLite direct operation rendering rejects every table rebuild operation with a context-required error.
#[test]
fn sqlite_rebuild_operations_require_migration_context() {
    let operations = vec![
        Operation::DropColumn {
            table_name: "users".to_string(),
            column: col("email", "text", true),
            cascade: false,
        },
        Operation::AlterColumn {
            table_name: "users".to_string(),
            old: col("age", "text", true),
            new: col("age", "integer", true),
            cast_expr: None,
        },
        Operation::AddForeignKey {
            table_name: "users".to_string(),
            foreign_key: ForeignKey::single(
                "users_account_id_fkey",
                "account_id",
                "accounts",
                "id",
            ),
        },
        Operation::DropForeignKey {
            table_name: "users".to_string(),
            foreign_key: ForeignKey::single(
                "users_account_id_fkey",
                "account_id",
                "accounts",
                "id",
            ),
            cascade: false,
        },
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: Constraint::Check {
                name: "users_age_check".to_string(),
                expression: "age >= 0".to_string(),
            },
        },
        Operation::DropConstraint {
            table_name: "users".to_string(),
            constraint: Constraint::Check {
                name: "users_age_check".to_string(),
                expression: "age >= 0".to_string(),
            },
        },
    ];

    for operation in operations {
        let err = operation_to_sql_without_start(operation.clone()).unwrap_err();
        assert!(
            err.to_string().contains("table rebuild planning failed"),
            "unexpected error for {operation:?}: {err}"
        );
    }
}

fn sqlite_unsupported_function(name: &str) -> FunctionDef {
    FunctionDef {
        name: name.to_string(),
        schema: None,
        arguments: String::new(),
        returns: "void".to_string(),
        language: "sql".to_string(),
        body: "SELECT 1".to_string(),
        volatility: Volatility::Volatile,
        security_definer: false,
        opaque: Default::default(),
    }
}

fn sqlite_unsupported_enum() -> EnumDef {
    EnumDef {
        name: "status".to_string(),
        schema: None,
        values: vec!["draft".to_string(), "published".to_string()],
        opaque: Default::default(),
    }
}

/// SQLite rejects every opaque object family that the dialect does not support.
#[test]
fn sqlite_rejects_unsupported_opaque_operations() {
    let function = sqlite_unsupported_function("notify");
    let enum_def = sqlite_unsupported_enum();
    let extension = ExtensionDef {
        name: "pgcrypto".to_string(),
        schema: None,
        version: None,
        opaque: Default::default(),
    };
    let operations = vec![
        Operation::CreateFunction {
            function: function.clone(),
        },
        Operation::AlterFunction {
            old: function.clone(),
            new: function.clone(),
        },
        Operation::DropFunction { function },
        Operation::CreateExtension {
            extension: extension.clone(),
        },
        Operation::DropExtension { extension },
        Operation::CreateEnum {
            enum_def: enum_def.clone(),
        },
        Operation::DropEnum {
            enum_def: enum_def.clone(),
        },
        Operation::RenameEnumValue {
            enum_name: "status".to_string(),
            schema: None,
            old_value: "draft".to_string(),
            new_value: "new".to_string(),
        },
        Operation::AlterEnum {
            old: enum_def.clone(),
            new: enum_def,
        },
    ];

    for operation in operations {
        let err = operation_to_sql(operation.clone()).unwrap_err();
        assert!(
            err.to_string()
                .contains("not supported by the SQLite dialect"),
            "unexpected error for {operation:?}: {err}"
        );
    }
}

fn migration_atomic(id: &str, atomic: bool, operations: Vec<Operation>) -> Migration {
    Migration {
        id: id.to_string(),
        dependencies: vec![],
        operations,
        atomic,
    }
}

struct SqliteEnvironment {
    config: Arc<Config>,
}

impl SqliteEnvironment {
    fn new() -> Self {
        Self {
            config: Arc::new(Config::new(
                "sqlite::memory:".to_string(),
                "migrations".into(),
                "schema.yaml".into(),
                Dialect::Sqlite,
            )),
        }
    }
}

impl Environment for SqliteEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor + Send>, EnvironmentError>> {
        Box::pin(async {
            Err(EnvironmentError::Config(
                "test environment does not create executors".into(),
            ))
        })
    }

    fn dialect(&self) -> Dialect {
        Dialect::Sqlite
    }
}

fn migrator(migrations: Vec<Migration>) -> Migrator {
    Migrator::new(
        Box::new(VecAdapter::new(migrations)),
        Box::new(SqliteEnvironment::new()),
    )
    .unwrap()
}

fn sql_for(migrations: Vec<Migration>) -> Vec<String> {
    let migrator = migrator(migrations.clone());
    migrator.sql_migrate(&migrations).unwrap()
}

#[test]
fn sqlite_renders_supported_create_table_subset() {
    let sql = operation_to_sql(Operation::CreateTable {
        table: table("users"),
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![
            r#"CREATE TABLE "users" ("id" integer PRIMARY KEY, CONSTRAINT "users_id_check" CHECK (id > 0))"#,
            r#"CREATE INDEX "users_id_idx" ON "users" ("id")"#,
        ]
    );
}

/// SQLite renders simple table drops directly.
#[test]
fn sqlite_renders_drop_table() {
    let sql = operation_to_sql(Operation::DropTable {
        table: table("users"),
    })
    .unwrap();

    assert_eq!(sql, vec![r#"DROP TABLE "users""#]);
}

/// SQLite renders simple table renames directly.
#[test]
fn sqlite_renders_rename_table() {
    let sql = operation_to_sql(Operation::RenameTable {
        old_name: "users".to_string(),
        new_name: "accounts".to_string(),
    })
    .unwrap();

    assert_eq!(sql, vec![r#"ALTER TABLE "users" RENAME TO "accounts""#]);
}

/// SQLite renders simple column additions directly.
#[test]
fn sqlite_renders_simple_add_column() {
    let sql = operation_to_sql(Operation::AddColumn {
        table_name: "users".to_string(),
        column: col("email", "text", true),
    })
    .unwrap();

    assert_eq!(sql, vec![r#"ALTER TABLE "users" ADD COLUMN "email" text"#]);
}

/// SQLite renders column renames directly.
#[test]
fn sqlite_renders_rename_column() {
    let sql = operation_to_sql(Operation::RenameColumn {
        table_name: "users".to_string(),
        old_name: "email".to_string(),
        new_name: "primary_email".to_string(),
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![r#"ALTER TABLE "users" RENAME COLUMN "email" TO "primary_email""#]
    );
}

/// SQLite renders direct index creation.
#[test]
fn sqlite_renders_add_index() {
    let sql = operation_to_sql(Operation::AddIndex {
        table_name: "users".to_string(),
        index: Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        },
        concurrent: false,
    })
    .unwrap();

    assert_eq!(
        sql,
        vec![r#"CREATE INDEX "users_email_idx" ON "users" ("email")"#]
    );
}

/// SQLite renders direct index drops.
#[test]
fn sqlite_renders_drop_index() {
    let sql = operation_to_sql(Operation::DropIndex {
        table_name: "users".to_string(),
        index: Index {
            name: "users_email_idx".to_string(),
            columns: vec!["email".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        },
        concurrent: false,
    })
    .unwrap();

    assert_eq!(sql, vec![r#"DROP INDEX "users_email_idx""#]);
}

/// SQLite preserves raw SQL statements exactly.
#[test]
fn sqlite_renders_statement_exactly() {
    let sql = operation_to_sql(Operation::Statement {
        up: "UPDATE users SET active = 1".to_string(),
        down: None,
    })
    .unwrap();

    assert_eq!(sql, vec!["UPDATE users SET active = 1"]);
}

/// SQLite renders views directly.
#[test]
fn sqlite_renders_view_operations() {
    let old = ViewDef {
        name: "active_users".to_string(),
        schema: None,
        definition: "SELECT id FROM users".to_string(),
        opaque: Default::default(),
    };
    let new = ViewDef {
        name: "active_users".to_string(),
        schema: None,
        definition: "SELECT id FROM users WHERE active = 1".to_string(),
        opaque: Default::default(),
    };

    assert_eq!(
        operation_to_sql(Operation::CreateView { view: old.clone() }).unwrap(),
        vec![r#"CREATE VIEW "active_users" AS SELECT id FROM users"#]
    );
    assert_eq!(
        operation_to_sql(Operation::ReplaceView {
            old: old.clone(),
            new
        })
        .unwrap(),
        vec![
            r#"DROP VIEW "active_users""#,
            r#"CREATE VIEW "active_users" AS SELECT id FROM users WHERE active = 1"#
        ]
    );
    assert_eq!(
        operation_to_sql(Operation::DropView { view: old }).unwrap(),
        vec![r#"DROP VIEW "active_users""#]
    );
}

#[test]
fn sqlite_errors_for_unsupported_extension_operations() {
    let err = operation_to_sql(Operation::CreateExtension {
        extension: ExtensionDef {
            name: "pgcrypto".to_string(),
            schema: None,
            version: None,
            opaque: Default::default(),
        },
    })
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("not supported by the SQLite dialect")
    );
}

#[test]
fn sqlite_errors_for_schema_qualified_tables() {
    let mut users = table("users");
    users.schema = Some("app".to_string());

    let err = operation_to_sql(Operation::CreateTable { table: users }).unwrap_err();

    assert!(err.to_string().contains("does not support schemas"));
}

#[test]
fn sqlite_operation_renderer_points_rebuild_ops_to_migrator() {
    let err = operation_to_sql_without_start(Operation::DropColumn {
        table_name: "users".to_string(),
        column: col("email", "text", true),
        cascade: false,
    })
    .unwrap_err();

    assert!(err.to_string().contains("table rebuild planning failed"));
}

#[test]
fn sqlite_rebuilds_drop_column_and_recreates_indexes() {
    let mut users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            pk_col("id"),
            col("username", "text", false),
            col("email", "text", true),
        ],
        foreign_keys: vec![],
        indexes: vec![Index {
            name: "users_username_idx".to_string(),
            columns: vec!["username".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        }],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let mut target_users = users.clone();
    target_users.columns.pop();

    let sql = sql_for(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_drop_email",
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: users.columns.pop().unwrap(),
                cascade: false,
            }],
        ),
    ]);

    assert_eq!(
        sql,
        vec![
            r#"CREATE TABLE "users" ("id" integer PRIMARY KEY, "username" text NOT NULL, "email" text)"#,
            r#"CREATE INDEX "users_username_idx" ON "users" ("username")"#,
            "PRAGMA defer_foreign_keys = ON",
            r#"CREATE TABLE "__gaman_rebuild_users" ("id" integer PRIMARY KEY, "username" text NOT NULL)"#,
            r#"INSERT INTO "__gaman_rebuild_users" ("id", "username") SELECT "id", "username" FROM "users""#,
            r#"DROP TABLE "users""#,
            r#"ALTER TABLE "__gaman_rebuild_users" RENAME TO "users""#,
            r#"CREATE INDEX "users_username_idx" ON "users" ("username")"#,
            r#"DROP TABLE IF EXISTS temp."__gaman_fk_check_users""#,
            r#"CREATE TEMP TABLE "__gaman_fk_check_users" ("violation" integer CHECK ("violation" = 0))"#,
            r#"INSERT INTO temp."__gaman_fk_check_users" ("violation") SELECT 1 FROM pragma_foreign_key_check"#,
            r#"DROP TABLE temp."__gaman_fk_check_users""#,
            "PRAGMA foreign_key_check",
        ]
    );
}

#[test]
fn sqlite_batches_same_table_rebuild_ops() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            pk_col("id"),
            col("age", "text", true),
            col("email", "text", true),
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let mut new_age = col("age", "integer", false);
    new_age.default = Some("0".to_string());

    let sql = sql_for(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_rebuild_users",
            vec![
                Operation::AlterColumn {
                    table_name: "users".to_string(),
                    old: col("age", "text", true),
                    new: new_age,
                    cast_expr: None,
                },
                Operation::DropColumn {
                    table_name: "users".to_string(),
                    column: col("email", "text", true),
                    cascade: false,
                },
                Operation::AddConstraint {
                    table_name: "users".to_string(),
                    constraint: Constraint::Check {
                        name: "users_age_check".to_string(),
                        expression: "age >= 0".to_string(),
                    },
                },
            ],
        ),
    ]);

    assert_eq!(
        sql.iter()
            .filter(|stmt| stmt.as_str() == "PRAGMA defer_foreign_keys = ON")
            .count(),
        1
    );
    assert!(sql.iter().any(|stmt| {
        stmt == r#"INSERT INTO "__gaman_rebuild_users" ("id", "age") SELECT "id", COALESCE(CAST("age" AS integer), 0) FROM "users""#
    }));
    assert!(sql.iter().any(|stmt| {
        stmt == r#"CREATE TABLE "__gaman_rebuild_users" ("id" integer PRIMARY KEY, "age" integer NOT NULL DEFAULT 0, CONSTRAINT "users_age_check" CHECK (age >= 0))"#
    }));
}

#[test]
fn sqlite_rebuilds_foreign_key_and_unique_constraint_changes() {
    let accounts = Table {
        name: "accounts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("account_id", "integer", false)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };

    let sql = sql_for(vec![
        migration(
            "0001_create_accounts",
            vec![Operation::CreateTable { table: accounts }],
        ),
        migration(
            "0002_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration(
            "0003_add_constraints",
            vec![
                Operation::AddForeignKey {
                    table_name: "users".to_string(),
                    foreign_key: ForeignKey::single(
                        "users_account_id_fkey",
                        "account_id",
                        "accounts",
                        "id",
                    ),
                },
                Operation::AddConstraint {
                    table_name: "users".to_string(),
                    constraint: Constraint::Unique {
                        name: "users_account_id_key".to_string(),
                        columns: vec!["account_id".to_string()],
                    },
                },
            ],
        ),
    ]);

    assert!(sql.iter().any(|stmt| {
        stmt.contains(r#"CONSTRAINT "users_account_id_fkey" FOREIGN KEY ("account_id") REFERENCES "accounts" ("id")"#)
            && stmt.contains(r#"CONSTRAINT "users_account_id_key" UNIQUE ("account_id")"#)
    }));
}

#[test]
fn sqlite_rebuild_adds_generated_column_without_copying_it() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("name", "text", false)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let mut slug = col("slug", "text", false);
    slug.generated = Some("lower(name)".to_string());

    let sql = sql_for(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration(
            "0002_add_slug",
            vec![Operation::AddColumn {
                table_name: "users".to_string(),
                column: slug,
            }],
        ),
    ]);

    assert!(sql.iter().any(|stmt| {
        stmt == r#"CREATE TABLE "__gaman_rebuild_users" ("id" integer PRIMARY KEY, "name" text NOT NULL, "slug" text GENERATED ALWAYS AS (lower(name)) STORED)"#
    }));
    assert!(sql.iter().any(|stmt| {
        stmt == r#"INSERT INTO "__gaman_rebuild_users" ("id", "name") SELECT "id", "name" FROM "users""#
    }));
}

#[test]
fn sqlite_rebuild_rejects_unsafe_cases() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("name", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };

    let mut new_id = pk_col("id");
    new_id.col_type = "text".to_string();
    let err = migrator(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_alter_pk",
            vec![Operation::AlterColumn {
                table_name: "users".to_string(),
                old: pk_col("id"),
                new: new_id,
                cast_expr: None,
            }],
        ),
    ])
    .sql_migrate(&[
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_alter_pk",
            vec![Operation::AlterColumn {
                table_name: "users".to_string(),
                old: pk_col("id"),
                new: {
                    let mut column = pk_col("id");
                    column.col_type = "text".to_string();
                    column
                },
                cast_expr: None,
            }],
        ),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("primary-key changes"));

    let mut required_name = col("name", "text", false);
    required_name.default = None;
    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_require_name",
            vec![Operation::AlterColumn {
                table_name: "users".to_string(),
                old: col("name", "text", true),
                new: required_name,
                cast_expr: None,
            }],
        ),
    ])
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("without a default or cast expression")
    );

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration_atomic(
            "0002_drop_name",
            false,
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: col("name", "text", true),
                cascade: false,
            }],
        ),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("require atomic migrations"));

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration(
            "0002_custom_type",
            vec![Operation::AlterColumn {
                table_name: "users".to_string(),
                old: col("name", "text", true),
                new: col("name", "email_address", true),
                cast_expr: None,
            }],
        ),
    ])
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("cannot automatically cast to ambiguous type 'email_address'")
    );
}

#[test]
fn sqlite_rebuild_rejects_temp_collision_triggers_and_views() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("name", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let temp = Table {
        name: "__gaman_rebuild_users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let fk_temp = Table {
        name: "__gaman_fk_check_users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let drop_name = Operation::DropColumn {
        table_name: "users".to_string(),
        column: col("name", "text", true),
        cascade: false,
    };

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_create_temp",
            vec![Operation::CreateTable { table: temp }],
        ),
        migration("0003_drop_name", vec![drop_name.clone()]),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("already exists"));

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_create_fk_temp",
            vec![Operation::CreateTable { table: fk_temp }],
        ),
        migration("0003_drop_name", vec![drop_name.clone()]),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("FK-check temp table"));

    let mut triggered = users.clone();
    triggered.triggers.push(TriggerDef {
        name: Some("users_audit".to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Update],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(user_id) VALUES (NEW.id);".to_string()),
        language: None,
        opaque: Default::default(),
    });
    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: triggered }],
        ),
        migration("0002_drop_name", vec![drop_name.clone()]),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("modeled triggers"));

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration(
            "0002_create_view",
            vec![Operation::CreateView {
                view: ViewDef {
                    name: "active_users".to_string(),
                    schema: None,
                    definition: r#"SELECT id FROM "users""#.to_string(),
                    opaque: Default::default(),
                },
            }],
        ),
        migration("0003_drop_name", vec![drop_name]),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("dependent view"));
}

#[test]
fn sqlite_dependent_view_detection_is_identifier_aware() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("name", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let drop_name = Operation::DropColumn {
        table_name: "users".to_string(),
        column: col("name", "text", true),
        cascade: false,
    };

    sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable {
                table: users.clone(),
            }],
        ),
        migration(
            "0002_create_unrelated_view",
            vec![Operation::CreateView {
                view: ViewDef {
                    name: "user_summary".to_string(),
                    schema: None,
                    definition: r#"SELECT 1 AS users_count"#.to_string(),
                    opaque: Default::default(),
                },
            }],
        ),
        migration("0003_drop_name", vec![drop_name.clone()]),
    ])
    .unwrap();

    let err = sql_for_result(vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration(
            "0002_create_quoted_view",
            vec![Operation::CreateView {
                view: ViewDef {
                    name: "quoted_users".to_string(),
                    schema: None,
                    definition: r#"SELECT id FROM "users""#.to_string(),
                    opaque: Default::default(),
                },
            }],
        ),
        migration("0003_drop_name", vec![drop_name]),
    ])
    .unwrap_err();
    assert!(err.to_string().contains("dependent view"));
}

fn sql_for_result(migrations: Vec<Migration>) -> Result<Vec<String>, gaman::core::MigratorError> {
    let migrator = migrator(migrations.clone());
    migrator.sql_migrate(&migrations)
}

#[tokio::test]
async fn sqlite_rebuild_live_preserves_data_and_constraints() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            pk_col("id"),
            col("age", "text", true),
            col("email", "text", true),
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let mut new_age = col("age", "integer", false);
    new_age.default = Some("0".to_string());

    let migrations = vec![
        migration("0001_create_users", vec![Operation::CreateTable { table: users }]),
        migration_after(
            "0002_seed",
            "0001_create_users",
            vec![Operation::Statement {
                up: r#"INSERT INTO "users" ("id", "age", "email") VALUES (1, NULL, 'a@example.com')"#.to_string(),
                down: None,
            }],
        ),
        migration_after(
            "0003_rebuild_users",
            "0002_seed",
            vec![
                Operation::AlterColumn {
                    table_name: "users".to_string(),
                    old: col("age", "text", true),
                    new: new_age,
                    cast_expr: None,
                },
                Operation::DropColumn {
                    table_name: "users".to_string(),
                    column: col("email", "text", true),
                    cascade: false,
                },
                Operation::AddConstraint {
                    table_name: "users".to_string(),
                    constraint: Constraint::Check {
                        name: "users_age_check".to_string(),
                        expression: "age >= 0".to_string(),
                    },
                },
            ],
        ),
    ];

    let migrator = migrator(migrations);
    let mut executor = sqlite_executor().await;
    migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap();

    assert_eq!(
        executor
            .fetch_strings(r#"SELECT CAST("age" AS text) FROM "users" WHERE "id" = 1"#)
            .await
            .unwrap(),
        vec!["0".to_string()]
    );
    let err = executor
        .execute(r#"INSERT INTO "users" ("id", "age") VALUES (2, -1)"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("users_age_check"));
}

#[tokio::test]
async fn sqlite_rebuild_live_supports_fk_and_rollback() {
    let accounts = Table {
        name: "accounts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("account_id", "integer", false)],
        foreign_keys: vec![],
        indexes: vec![Index {
            name: "users_account_id_idx".to_string(),
            columns: vec!["account_id".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        }],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let fk = ForeignKey::single("users_account_id_fkey", "account_id", "accounts", "id");

    let migrations = vec![
        migration(
            "0001_create_accounts",
            vec![Operation::CreateTable { table: accounts }],
        ),
        migration_after(
            "0002_create_users",
            "0001_create_accounts",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0003_add_fk",
            "0002_create_users",
            vec![Operation::AddForeignKey {
                table_name: "users".to_string(),
                foreign_key: fk,
            }],
        ),
    ];

    let migrator = migrator(migrations);
    let mut executor = sqlite_executor().await;
    let forward = migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap();
    assert_eq!(forward.applied, 3);
    assert_eq!(forward.reverted, 0);
    executor
        .execute(r#"INSERT INTO "accounts" ("id") VALUES (1)"#)
        .await
        .unwrap();
    executor
        .execute(r#"INSERT INTO "users" ("id", "account_id") VALUES (1, 1)"#)
        .await
        .unwrap();
    let err = executor
        .execute(r#"INSERT INTO "users" ("id", "account_id") VALUES (2, 404)"#)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("FOREIGN KEY"));

    let backward = migrator
        .apply_with(&mut executor, Some("0002_create_users"), false)
        .await
        .unwrap();
    assert_eq!(backward.applied, 0);
    assert_eq!(backward.reverted, 1);
    executor
        .execute(r#"INSERT INTO "users" ("id", "account_id") VALUES (2, 404)"#)
        .await
        .unwrap();
    assert_eq!(
        executor
            .fetch_strings(
                r#"SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'users_account_id_idx'"#,
            )
            .await
            .unwrap(),
        vec!["users_account_id_idx".to_string()]
    );
}

#[tokio::test]
async fn sqlite_rebuild_live_preserves_child_rows_when_parent_is_rebuilt() {
    let mut accounts_v1 = Table {
        name: "accounts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("legacy_code", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("account_id", "integer", false)],
        foreign_keys: vec![ForeignKey::single(
            "users_account_id_fkey",
            "account_id",
            "accounts",
            "id",
        )],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    accounts_v1.indexes.push(Index {
        name: "accounts_id_idx".to_string(),
        columns: vec!["id".to_string()],
        unique: false,
        predicate: None,
        opaque: Default::default(),
    });

    let migrations = vec![
        migration(
            "0001_create_accounts",
            vec![Operation::CreateTable { table: accounts_v1 }],
        ),
        migration_after(
            "0002_create_users",
            "0001_create_accounts",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0003_rebuild_parent",
            "0002_create_users",
            vec![Operation::DropColumn {
                table_name: "accounts".to_string(),
                column: col("legacy_code", "text", true),
                cascade: false,
            }],
        ),
    ];

    let migrator = migrator(migrations);
    let mut executor = sqlite_executor().await;
    migrator
        .apply_with(&mut executor, Some("0002_create_users"), false)
        .await
        .unwrap();
    executor
        .execute(r#"INSERT INTO "accounts" ("id", "legacy_code") VALUES (1, 'A')"#)
        .await
        .unwrap();
    executor
        .execute(r#"INSERT INTO "users" ("id", "account_id") VALUES (1, 1)"#)
        .await
        .unwrap();

    let err = migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("inbound foreign key"), "{err}");
    assert_eq!(
        executor
            .fetch_strings(r#"SELECT CAST(account_id AS TEXT) FROM "users" ORDER BY id"#)
            .await
            .unwrap(),
        vec!["1".to_string()]
    );
    assert_eq!(
        executor
            .fetch_strings(r#"SELECT legacy_code FROM "accounts" WHERE id = 1"#)
            .await
            .unwrap(),
        vec!["A".to_string()]
    );
    executor
        .execute(r#"INSERT INTO "users" ("id", "account_id") VALUES (2, 404)"#)
        .await
        .unwrap_err();
}

#[tokio::test]
async fn sqlite_rebuild_live_fails_foreign_key_check_for_existing_bad_data() {
    let accounts = Table {
        name: "accounts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("account_id", "integer", false)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let migrations = vec![
        migration(
            "0001_create_accounts",
            vec![Operation::CreateTable { table: accounts }],
        ),
        migration_after(
            "0002_create_users",
            "0001_create_accounts",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0003_seed_bad_user",
            "0002_create_users",
            vec![Operation::Statement {
                up: r#"INSERT INTO "users" ("id", "account_id") VALUES (1, 404)"#.to_string(),
                down: None,
            }],
        ),
        migration_after(
            "0004_add_fk",
            "0003_seed_bad_user",
            vec![Operation::AddForeignKey {
                table_name: "users".to_string(),
                foreign_key: ForeignKey::single(
                    "users_account_id_fkey",
                    "account_id",
                    "accounts",
                    "id",
                ),
            }],
        ),
    ];

    let migrator = migrator(migrations);
    let mut executor = sqlite_executor().await;
    let err = migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("__gaman_fk_check"));
}

async fn sqlite_executor() -> SqliteExecutor {
    let opts = "sqlite::memory:"
        .parse::<sqlx::sqlite::SqliteConnectOptions>()
        .unwrap()
        .foreign_keys(true);
    SqliteExecutor::new(opts.connect().await.unwrap())
}

#[derive(Default)]
struct RecordingExecutor {
    log: Vec<String>,
    fail_on: Option<&'static str>,
    lock_count: usize,
}

impl RecordingExecutor {
    fn failing(fail_on: &'static str) -> Self {
        Self {
            fail_on: Some(fail_on),
            ..Self::default()
        }
    }
}

impl Executor for RecordingExecutor {
    fn execute<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.log.push(sql.to_string());
        let should_fail = self.fail_on.is_some_and(|needle| sql.contains(needle));
        Box::pin(async move {
            if should_fail {
                Err(gaman::core::ExecutorError::Execute(
                    "forced failure".to_string(),
                ))
            } else {
                Ok(())
            }
        })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        _sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, gaman::core::ExecutorError>> {
        Box::pin(async { Ok(vec![]) })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.log.push("BEGIN".to_string());
        Box::pin(async { Ok(()) })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.log.push("COMMIT".to_string());
        Box::pin(async { Ok(()) })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.log.push("ROLLBACK".to_string());
        Box::pin(async { Ok(()) })
    }

    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.lock_count += 1;
        self.log.push("ACQUIRE_LOCK".to_string());
        Box::pin(async { Ok(()) })
    }

    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), gaman::core::ExecutorError>> {
        self.lock_count -= 1;
        self.log.push("RELEASE_LOCK".to_string());
        Box::pin(async { Ok(()) })
    }
}

#[tokio::test]
async fn sqlite_rebuild_uses_existing_migrator_transaction_and_record_flow() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("email", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let migrations = vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0002_drop_email",
            "0001_create_users",
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: col("email", "text", true),
                cascade: false,
            }],
        ),
    ];
    let migrator = migrator(migrations);
    let mut executor = RecordingExecutor::default();

    migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap();

    let begin_pos = executor
        .log
        .iter()
        .position(|entry| entry == "BEGIN")
        .unwrap();
    let rebuild_pos = executor
        .log
        .iter()
        .position(|entry| entry == "PRAGMA defer_foreign_keys = ON")
        .unwrap();
    let record_pos = executor
        .log
        .iter()
        .rposition(|entry| entry.contains(&format!("INSERT INTO {TRACKING_TABLE}")))
        .unwrap();
    let commit_pos = executor
        .log
        .iter()
        .rposition(|entry| entry == "COMMIT")
        .unwrap();

    assert!(begin_pos < rebuild_pos);
    assert!(rebuild_pos < record_pos);
    assert!(record_pos < commit_pos);
    assert_eq!(executor.lock_count, 0);
    assert_eq!(
        executor.log.last().map(String::as_str),
        Some("RELEASE_LOCK")
    );
}

#[tokio::test]
async fn sqlite_rebuild_failure_rolls_back_without_recording_and_releases_lock() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("email", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let migrations = vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0002_drop_email",
            "0001_create_users",
            vec![Operation::DropColumn {
                table_name: "users".to_string(),
                column: col("email", "text", true),
                cascade: false,
            }],
        ),
    ];
    let migrator = migrator(migrations);
    let mut executor = RecordingExecutor::failing(r#"INSERT INTO "__gaman_rebuild_users""#);

    let err = migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("forced failure"));
    assert!(executor.log.iter().any(|entry| entry == "ROLLBACK"));
    assert!(!executor.log.iter().any(|entry| {
        entry.contains(&format!("INSERT INTO {TRACKING_TABLE}"))
            && entry.contains("0002_drop_email")
    }));
    assert_eq!(executor.lock_count, 0);
    assert_eq!(
        executor.log.last().map(String::as_str),
        Some("RELEASE_LOCK")
    );
}

#[tokio::test]
async fn sqlite_rebuild_render_failure_preflights_before_install_lock_or_begin() {
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("name", "text", true)],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
        options: Default::default(),
    };
    let migrations = vec![
        migration(
            "0001_create_users",
            vec![Operation::CreateTable { table: users }],
        ),
        migration_after(
            "0002_require_name",
            "0001_create_users",
            vec![Operation::AlterColumn {
                table_name: "users".to_string(),
                old: col("name", "text", true),
                new: col("name", "text", false),
                cast_expr: None,
            }],
        ),
    ];
    let migrator = migrator(migrations);
    let mut executor = RecordingExecutor::default();

    let err = migrator
        .apply_with(&mut executor, None, false)
        .await
        .unwrap_err();

    assert!(
        err.to_string()
            .contains("without a default or cast expression")
    );
    assert!(
        executor.log.is_empty(),
        "unexpected executor activity: {:?}",
        executor.log
    );
    assert_eq!(executor.lock_count, 0);
}
