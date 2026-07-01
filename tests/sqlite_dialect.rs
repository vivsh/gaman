#![cfg(feature = "sqlite")]

use std::sync::Arc;

use gaman::core::{
    BoxFuture, Dialect, Environment, EnvironmentError, EnvironmentExecutor, Executor, Invoker,
    Migrator, SqliteExecutor, VecAdapter,
};
use gaman::schema::{
    Column, Constraint, ExtensionDef, ForeignKey, Index, Operation, PrimaryKey, Table, TriggerDef,
    TriggerEvent, TriggerScope, TriggerTiming, ViewDef,
};
use gaman::{Config, Migration};
use sqlx::ConnectOptions;

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
        }],
        constraints: vec![Constraint::Check {
            name: format!("{name}_id_check"),
            expression: "id > 0".to_string(),
        }],
        triggers: vec![],
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
    };

    let sql = Dialect::Sqlite
        .operation_to_sql(&Operation::CreateTable { table })
        .unwrap();

    assert_eq!(sql.len(), 1);
    assert!(
        sql[0].contains(
            "CONSTRAINT \"order_lines_identity\" PRIMARY KEY (\"tenant_id\", \"order_id\")"
        )
    );
    assert!(!sql[0].contains("\"tenant_id\" integer PRIMARY KEY"));
    assert!(!sql[0].contains("\"order_id\" integer PRIMARY KEY"));
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
            config: Arc::new(Config::default()),
        }
    }
}

impl Environment for SqliteEnvironment {
    fn config(&self) -> &Arc<Config> {
        &self.config
    }

    fn executor<'a>(
        &'a self,
    ) -> BoxFuture<'a, Result<Box<dyn EnvironmentExecutor>, EnvironmentError>> {
        Box::pin(async {
            Err(EnvironmentError::Config(
                "test environment does not create executors".into(),
            ))
        })
    }

    fn invoker(&self) -> Result<Option<Box<dyn Invoker>>, EnvironmentError> {
        Ok(None)
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
    let sql = Dialect::Sqlite
        .operation_to_sql(&Operation::CreateTable {
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

#[test]
fn sqlite_errors_for_unsupported_extension_operations() {
    let err = Dialect::Sqlite
        .operation_to_sql(&Operation::CreateExtension {
            extension: ExtensionDef {
                name: "pgcrypto".to_string(),
                schema: None,
                version: None,
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

    let err = Dialect::Sqlite
        .operation_to_sql(&Operation::CreateTable { table: users })
        .unwrap_err();

    assert!(err.to_string().contains("does not support schemas"));
}

#[test]
fn sqlite_operation_renderer_points_rebuild_ops_to_migrator() {
    let err = Dialect::Sqlite
        .operation_to_sql(&Operation::DropColumn {
            table_name: "users".to_string(),
            column: col("email", "text", true),
            cascade: false,
        })
        .unwrap_err();

    assert!(err.to_string().contains("render through Migrator"));
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
        }],
        constraints: vec![],
        triggers: vec![],
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
                    foreign_key: ForeignKey {
                        name: "users_account_id_fkey".to_string(),
                        from_column: "account_id".to_string(),
                        to_table: "accounts".to_string(),
                        to_column: "id".to_string(),
                    },
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
    assert!(err.to_string().contains("ambiguous type"));
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
        function_name: Some("audit_users".to_string()),
        when: None,
        body: None,
        language: None,
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
        .migrate_with(&mut executor, None, None, false)
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
        }],
        constraints: vec![],
        triggers: vec![],
    };
    let fk = ForeignKey {
        name: "users_account_id_fkey".to_string(),
        from_column: "account_id".to_string(),
        to_table: "accounts".to_string(),
        to_column: "id".to_string(),
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
    migrator
        .migrate_with(&mut executor, None, None, false)
        .await
        .unwrap();
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

    migrator
        .migrate_with(&mut executor, None, Some("0002_create_users"), false)
        .await
        .unwrap();
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
    };
    let users = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col("id"), col("account_id", "integer", false)],
        foreign_keys: vec![ForeignKey {
            name: "users_account_id_fkey".to_string(),
            from_column: "account_id".to_string(),
            to_table: "accounts".to_string(),
            to_column: "id".to_string(),
        }],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    accounts_v1.indexes.push(Index {
        name: "accounts_id_idx".to_string(),
        columns: vec!["id".to_string()],
        unique: false,
        predicate: None,
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
        .migrate_with(&mut executor, None, Some("0002_create_users"), false)
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
        .migrate_with(&mut executor, None, None, false)
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
                foreign_key: ForeignKey {
                    name: "users_account_id_fkey".to_string(),
                    from_column: "account_id".to_string(),
                    to_table: "accounts".to_string(),
                    to_column: "id".to_string(),
                },
            }],
        ),
    ];

    let migrator = migrator(migrations);
    let mut executor = sqlite_executor().await;
    let err = migrator
        .migrate_with(&mut executor, None, None, false)
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
        .migrate_with(&mut executor, None, None, false)
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
        .rposition(|entry| entry.contains("INSERT INTO gaman_migrations"))
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
        .migrate_with(&mut executor, None, None, false)
        .await
        .unwrap_err();

    assert!(err.to_string().contains("forced failure"));
    assert!(executor.log.iter().any(|entry| entry == "ROLLBACK"));
    assert!(
        !executor
            .log
            .iter()
            .any(|entry| entry.contains("INSERT INTO gaman_migrations")
                && entry.contains("0002_drop_email"))
    );
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
        .migrate_with(&mut executor, None, None, false)
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
