use super::*;
use crate::dialects::Dialect;
use crate::operations::Operation;

fn basic_table(name: &str) -> Table {
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

fn text_col(name: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: "text".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        ..Default::default()
    }
}

fn apply_ok(state: &mut Schema, op: Operation) {
    state.apply(&op).expect("apply should succeed");
}

#[test]
fn prepare_normalizes_common_postgres_type_aliases_from_yaml() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: int4
        primary_key: true
      - name: name
        type: character varying(100)
      - name: active
        type: bool
      - name: created_at
        type: timestamp
"#,
        Dialect::Postgres,
    )
    .expect("prepared schema");

    let columns = &schema.tables["users"].columns;
    assert_eq!(columns[0].col_type, "integer");
    assert_eq!(columns[1].col_type, "varchar(100)");
    assert_eq!(columns[2].col_type, "boolean");
    assert_eq!(columns[3].col_type, "timestamp without time zone");
}

#[test]
fn checked_frontends_prepare_to_the_same_schema() {
    let yaml = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: int4
        primary_key: true
      - name: name
        type: text
        nullable: true
"#,
        Dialect::Postgres,
    )
    .expect("yaml");
    let json = Schema::from_json_str(
        r#"{"tables":{"users":{"columns":[{"name":"id","type":"int4","primary_key":true},{"name":"name","type":"text","nullable":true}]}}}"#,
        Dialect::Postgres,
    )
    .expect("json");
    let sql = Schema::from_sql_str(
        "CREATE TABLE users (id int4 PRIMARY KEY, name text);",
        Dialect::Postgres,
    )
    .expect("sql");
    let builder = Schema::builder(Dialect::Postgres)
        .table::<PreparedUser>()
        .build_checked()
        .expect("builder");

    assert_eq!(yaml, json);
    assert_eq!(yaml, sql);
    assert_eq!(yaml, builder);
}

struct PreparedUser;

impl IntoTable for PreparedUser {
    fn into_table(_dialect: &Dialect) -> Table {
        TableBuilder::new("users")
            .column("id", "int4", |column| column.primary_key())
            .column("name", "text", |column| column.nullable())
            .build()
    }
}

#[test]
fn prepare_preserves_unknown_postgres_type_for_later_clarification() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: age
        type: intger
"#,
        Dialect::Postgres,
    )
    .expect("unknown type should remain flexible schema IR");

    assert_eq!(schema.tables["users"].columns[0].col_type, "intger");
}

#[test]
fn prepare_rejects_unknown_foreign_key_reference_before_sql_rendering() {
    let err = Schema::from_yaml_str(
        r#"
tables:
  posts:
    columns:
      - name: id
        type: integer
      - name: author_id
        type: integer
        references:
          table: users
          column: id
"#,
        Dialect::Postgres,
    )
    .unwrap_err();

    assert!(err.to_string().contains("referenced table users not found"));
}

#[test]
fn prepare_rejects_unknown_index_column() {
    let err = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: integer
    indexes:
      - name: users_missing_idx
        columns: [missing]
"#,
        Dialect::Postgres,
    )
    .unwrap_err();

    assert!(err.to_string().contains("index users_missing_idx"));
    assert!(err.to_string().contains("unknown column 'missing'"));
}

#[test]
fn prepare_normalizes_sqlite_type_aliases() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: int4
      - name: active
        type: bool
      - name: name
        type: varchar(100)
"#,
        Dialect::Sqlite,
    )
    .expect("prepared schema");

    let columns = &schema.tables["users"].columns;
    assert_eq!(columns[0].col_type, "integer");
    assert_eq!(columns[1].col_type, "integer");
    assert_eq!(columns[2].col_type, "text");
}

#[test]
fn prepare_rejects_sqlite_unsupported_features() {
    let err = Schema::from_yaml_str(
        r#"
extensions:
  pgcrypto:
    name: pgcrypto
tables:
  users:
    columns:
      - name: id
        type: integer
"#,
        Dialect::Sqlite,
    )
    .unwrap_err();

    assert!(
        err.to_string()
            .contains("SQLite does not support extensions: pgcrypto")
    );
}

#[test]
fn normalize_column_primary_key_flags_into_table_primary_key() {
    let mut schema = Schema::default();
    let mut table = basic_table("order_lines");
    table.columns.push(Column {
        name: "order_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: true,
        primary_key: true,
        ..Default::default()
    });
    table.columns.push(Column {
        name: "tenant_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: true,
        primary_key: true,
        ..Default::default()
    });
    schema.tables.insert("order_lines".to_string(), table);

    schema.normalize();
    let table = &schema.tables["order_lines"];
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.name, "order_lines_pkey");
    assert_eq!(pk.columns, ["order_id", "tenant_id"]);
    assert!(table.columns.iter().all(|column| !column.nullable));
}

#[test]
fn normalize_explicit_primary_key_preserves_name_and_order() {
    let mut schema = Schema::default();
    let mut table = basic_table("order_lines");
    table.primary_key = Some(PrimaryKey {
        name: "order_lines_identity".to_string(),
        columns: vec!["tenant_id".to_string(), "order_id".to_string()],
    });
    table.columns.push(Column {
        name: "order_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: true,
        ..Default::default()
    });
    table.columns.push(Column {
        name: "tenant_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: true,
        ..Default::default()
    });
    schema.tables.insert("order_lines".to_string(), table);

    schema.normalize();
    let table = &schema.tables["order_lines"];
    assert_eq!(table.primary_key_column_names(), ["tenant_id", "order_id"]);
    assert!(table.is_primary_key_column("tenant_id"));
    assert!(table.is_primary_key_column("order_id"));
    assert!(table.columns.iter().all(|column| !column.nullable));
}

#[test]
fn validate_rejects_conflicting_explicit_primary_key_and_column_flags() {
    let mut table = basic_table("order_lines");
    table.primary_key = Some(PrimaryKey {
        name: "order_lines_identity".to_string(),
        columns: vec!["tenant_id".to_string(), "order_id".to_string()],
    });
    table.columns.push(Column {
        name: "order_id".to_string(),
        col_type: "bigint".to_string(),
        primary_key: true,
        ..Default::default()
    });
    table.columns.push(Column {
        name: "tenant_id".to_string(),
        col_type: "bigint".to_string(),
        ..Default::default()
    });
    let mut schema = Schema::default();
    schema.tables.insert("order_lines".to_string(), table);

    let err = schema.validate().unwrap_err();
    assert!(err.contains("primary key column flags conflict"));
}

#[test]
fn validate_rejects_unknown_primary_key_column() {
    let mut table = basic_table("order_lines");
    table.primary_key = Some(PrimaryKey {
        name: "order_lines_identity".to_string(),
        columns: vec!["tenant_id".to_string()],
    });
    table.columns.push(Column {
        name: "order_id".to_string(),
        col_type: "bigint".to_string(),
        ..Default::default()
    });
    let mut schema = Schema::default();
    schema.tables.insert("order_lines".to_string(), table);

    let err = schema.validate().unwrap_err();
    assert!(err.contains("references unknown column 'tenant_id'"));
}

#[test]
fn yaml_accepts_explicit_primary_key_metadata() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  order_lines:
    primary_key:
      name: order_lines_identity
      columns: [tenant_id, order_id]
    columns:
      - name: order_id
        type: bigint
      - name: tenant_id
        type: bigint
"#,
        Dialect::Postgres,
    )
    .unwrap();

    let table = &schema.tables["order_lines"];
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.name, "order_lines_identity");
    assert_eq!(pk.columns, ["tenant_id", "order_id"]);
    assert!(table.columns.iter().all(|column| column.primary_key));
}

/// CreateTable inserts the table into the state.
#[test]
fn create_table() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    assert!(s.tables.contains_key("users"));
}

/// CreateTable on an existing table name returns TableAlreadyExists.
#[test]
fn create_table_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let err = s
        .apply(&Operation::CreateTable {
            table: basic_table("users"),
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::TableAlreadyExists("users".to_string()));
}

/// DropTable removes the table from state.
#[test]
fn drop_table() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::DropTable {
            table: basic_table("users"),
        },
    );
    assert!(!s.tables.contains_key("users"));
}

/// DropTable on a nonexistent table returns TableNotFound.
#[test]
fn drop_table_not_found() {
    let mut s = Schema::default();
    let err = s
        .apply(&Operation::DropTable {
            table: basic_table("ghost"),
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::TableNotFound("ghost".to_string()));
}

/// RenameTable moves the table to the new name and updates its `name` field.
#[test]
fn rename_table() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::RenameTable {
            old_name: "users".to_string(),
            new_name: "accounts".to_string(),
        },
    );
    assert!(!s.tables.contains_key("users"));
    assert_eq!(s.tables["accounts"].name, "accounts");
}

/// RenameTable errors if the target name already exists.
#[test]
fn rename_table_target_exists() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("accounts"),
        },
    );
    let err = s
        .apply(&Operation::RenameTable {
            old_name: "users".to_string(),
            new_name: "accounts".to_string(),
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::RenameTargetExists { .. }));
}

/// AddColumn appends the column to the table.
#[test]
fn add_column() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    assert_eq!(s.tables["users"].columns[0].name, "email");
}

/// AddColumn on an existing column name returns ColumnAlreadyExists.
#[test]
fn add_column_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    let err = s
        .apply(&Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::ColumnAlreadyExists { .. }));
}

/// DropColumn removes the named column.
#[test]
fn drop_column() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    apply_ok(
        &mut s,
        Operation::DropColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
            cascade: false,
        },
    );
    assert!(s.tables["users"].columns.is_empty());
}

/// DropColumn on a missing column returns ColumnNotFound.
#[test]
fn drop_column_not_found() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let err = s
        .apply(&Operation::DropColumn {
            table_name: "users".to_string(),
            column: text_col("ghost"),
            cascade: false,
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::ColumnNotFound { .. }));
}

/// RenameColumn changes the column's name in place.
#[test]
fn rename_column() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    apply_ok(
        &mut s,
        Operation::RenameColumn {
            table_name: "users".to_string(),
            old_name: "email".to_string(),
            new_name: "email_address".to_string(),
        },
    );
    assert_eq!(s.tables["users"].columns[0].name, "email_address");
}

/// AlterColumn replaces the column definition.
#[test]
fn alter_column() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("bio"),
        },
    );
    let new_col = Column {
        name: "bio".to_string(),
        col_type: "varchar(500)".to_string(),
        nullable: true,
        default: None,
        primary_key: false,
        ..Default::default()
    };
    apply_ok(
        &mut s,
        Operation::AlterColumn {
            table_name: "users".to_string(),
            old: text_col("bio"),
            new: new_col,
            cast_expr: None,
        },
    );
    assert_eq!(s.tables["users"].columns[0].col_type, "varchar(500)");
    assert!(s.tables["users"].columns[0].nullable);
}

/// AddForeignKey attaches a FK to the table.
#[test]
fn add_foreign_key() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("posts"),
        },
    );
    let fk = ForeignKey::single("fk_user", "user_id", "users", "id");
    apply_ok(
        &mut s,
        Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: fk,
        },
    );
    assert_eq!(s.tables["posts"].foreign_keys[0].name, "fk_user");
}

/// AddForeignKey with a duplicate name returns ForeignKeyAlreadyExists.
#[test]
fn add_foreign_key_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("posts"),
        },
    );
    let fk = ForeignKey::single("fk_user", "user_id", "users", "id");
    apply_ok(
        &mut s,
        Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: fk.clone(),
        },
    );
    let err = s
        .apply(&Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: fk,
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::ForeignKeyAlreadyExists { .. }));
}

/// DropForeignKey removes the FK from the table.
#[test]
fn drop_foreign_key() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("posts"),
        },
    );
    let fk = ForeignKey::single("fk_user", "user_id", "users", "id");
    apply_ok(
        &mut s,
        Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: fk,
        },
    );
    apply_ok(
        &mut s,
        Operation::DropForeignKey {
            table_name: "posts".to_string(),
            foreign_key: ForeignKey::single("fk_user", "user_id", "users", "id"),
            cascade: false,
        },
    );
    assert!(s.tables["posts"].foreign_keys.is_empty());
}

/// AddIndex attaches an index to the table.
#[test]
fn add_index() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let idx = Index {
        name: "idx_email".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
        predicate: None,
    };
    apply_ok(
        &mut s,
        Operation::AddIndex {
            table_name: "users".to_string(),
            index: idx,
            concurrent: false,
        },
    );
    assert_eq!(s.tables["users"].indexes[0].name, "idx_email");
}

/// AddIndex with a duplicate name returns IndexAlreadyExists.
#[test]
fn add_index_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let idx = Index {
        name: "idx_email".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
        predicate: None,
    };
    apply_ok(
        &mut s,
        Operation::AddIndex {
            table_name: "users".to_string(),
            index: idx.clone(),
            concurrent: false,
        },
    );
    let err = s
        .apply(&Operation::AddIndex {
            table_name: "users".to_string(),
            index: idx,
            concurrent: false,
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::IndexAlreadyExists { .. }));
}

/// DropIndex removes the index.
#[test]
fn drop_index() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let idx = Index {
        name: "idx_email".to_string(),
        columns: vec!["email".to_string()],
        unique: true,
        predicate: None,
    };
    apply_ok(
        &mut s,
        Operation::AddIndex {
            table_name: "users".to_string(),
            index: idx,
            concurrent: false,
        },
    );
    apply_ok(
        &mut s,
        Operation::DropIndex {
            table_name: "users".to_string(),
            index: Index {
                name: "idx_email".to_string(),
                columns: vec!["email".to_string()],
                unique: true,
                predicate: None,
            },
            concurrent: false,
        },
    );
    assert!(s.tables["users"].indexes.is_empty());
}

/// AddConstraint attaches a check constraint to the table.
#[test]
fn add_constraint() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let c = Constraint::Check {
        name: "chk_age".to_string(),
        expression: "age > 0".to_string(),
    };
    apply_ok(
        &mut s,
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: c,
        },
    );
    assert_eq!(s.tables["users"].constraints[0].name(), "chk_age");
}

/// AddConstraint with a duplicate name returns ConstraintAlreadyExists.
#[test]
fn add_constraint_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let c = Constraint::Unique {
        name: "uq_email".to_string(),
        columns: vec!["email".to_string()],
    };
    apply_ok(
        &mut s,
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: c.clone(),
        },
    );
    let err = s
        .apply(&Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: c,
        })
        .unwrap_err();
    assert!(matches!(err, ReplayError::ConstraintAlreadyExists { .. }));
}

/// DropConstraint removes the named constraint.
#[test]
fn drop_constraint() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let c = Constraint::Unique {
        name: "uq_email".to_string(),
        columns: vec!["email".to_string()],
    };
    apply_ok(
        &mut s,
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: c,
        },
    );
    apply_ok(
        &mut s,
        Operation::DropConstraint {
            table_name: "users".to_string(),
            constraint: Constraint::Unique {
                name: "uq_email".to_string(),
                columns: vec!["email".to_string()],
            },
        },
    );
    assert!(s.tables["users"].constraints.is_empty());
}

/// Statement is a no-op — state is unchanged.
#[test]
fn statement_is_noop() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::Statement {
            up: "SELECT 1".to_string(),
            down: None,
        },
    );
    assert!(s.tables.is_empty());
}

/// Replaying a sequence of operations builds the expected state end-to-end.
#[test]
fn end_to_end_replay() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("bio"),
        },
    );
    apply_ok(
        &mut s,
        Operation::DropColumn {
            table_name: "users".to_string(),
            column: text_col("bio"),
            cascade: false,
        },
    );
    apply_ok(
        &mut s,
        Operation::RenameColumn {
            table_name: "users".to_string(),
            old_name: "email".to_string(),
            new_name: "email_address".to_string(),
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("posts"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "posts".to_string(),
            column: text_col("title"),
        },
    );

    assert_eq!(s.tables.len(), 2);
    assert_eq!(s.tables["users"].columns.len(), 1);
    assert_eq!(s.tables["users"].columns[0].name, "email_address");
    assert_eq!(s.tables["posts"].columns[0].name, "title");
}

/// Operations on a nonexistent table return TableNotFound.
#[test]
fn table_not_found_propagates() {
    let mut s = Schema::default();
    let err = s
        .apply(&Operation::AddColumn {
            table_name: "ghost".to_string(),
            column: text_col("x"),
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::TableNotFound("ghost".to_string()));
}

/// CreateTable with two primary_key columns creates a composite primary key.
#[test]
fn create_table_with_multiple_pk_creates_composite_primary_key() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            Column {
                name: "id".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "alt_id".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    s.apply(&Operation::CreateTable { table }).unwrap();
    let table = &s.tables["users"];
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.columns, ["id", "alt_id"]);
}

/// AddColumn with primary_key=true on an existing table is an unsafe PK mutation.
#[test]
fn add_pk_column_when_pk_exists_returns_error() {
    let mut s = Schema::default();
    let pk_col = Column {
        name: "id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    };
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    s.apply(&Operation::CreateTable { table }).unwrap();
    let second_pk = Column {
        name: "alt_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    };
    let err = s
        .apply(&Operation::AddColumn {
            table_name: "users".to_string(),
            column: second_pk,
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::PrimaryKeyMutation("users".to_string()));
}

/// AlterColumn that promotes a column to primary_key is an unsafe PK mutation.
#[test]
fn alter_column_to_pk_when_pk_exists_returns_error() {
    let mut s = Schema::default();
    let pk_col = Column {
        name: "id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    };
    let other_col = Column {
        name: "other".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        ..Default::default()
    };
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col, other_col.clone()],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    s.apply(&Operation::CreateTable { table }).unwrap();
    let promoted = Column {
        name: "other".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    };
    let err = s
        .apply(&Operation::AlterColumn {
            table_name: "users".to_string(),
            old: other_col,
            new: promoted,
            cast_expr: None,
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::PrimaryKeyMutation("users".to_string()));
}

/// validate() returns Ok for a state with at most one PK column per table.
#[test]
fn validate_single_pk_ok() {
    let mut s = Schema::default();
    let pk_col = Column {
        name: "id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    };
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![pk_col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    s.tables.insert("users".to_string(), table);
    assert!(s.validate().is_ok());
}

/// validate() returns Err when a table has more than one primary key column.
#[test]
fn validate_multiple_pk_returns_err() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![
            Column {
                name: "id".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
            Column {
                name: "alt".to_string(),
                col_type: "bigint".to_string(),
                nullable: false,
                default: None,
                primary_key: true,
                ..Default::default()
            },
        ],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    s.tables.insert("users".to_string(), table);
    s.normalize();
    assert!(s.validate().is_ok());
    assert_eq!(s.tables["users"].primary_key_column_names(), ["id", "alt"]);
}

/// normalize() moves an inline `references` into `table.foreign_keys` and clears the column field.
/// The FK name is auto-generated as `{table}_{column}_fkey` when no explicit name is given.
#[test]
fn normalize_moves_inline_fk_to_foreign_keys() {
    let col = Column {
        name: "user_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        references: Some(ColumnRef {
            table: "users".to_string(),
            column: "id".to_string(),
            name: None,
            on_delete: None,
        }),
        check: None,
        generated: None,
    };
    let table = Table {
        name: "posts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut s = Schema::default();
    s.tables.insert("posts".to_string(), table);
    s.normalize();
    let t = &s.tables["posts"];
    assert_eq!(t.foreign_keys.len(), 1);
    assert_eq!(t.foreign_keys[0].name, "posts_user_id_fkey");
    assert_eq!(t.foreign_keys[0].columns, ["user_id"]);
    assert_eq!(t.foreign_keys[0].to_table, "users");
    assert_eq!(t.foreign_keys[0].to_columns, ["id"]);
    assert!(t.columns[0].references.is_none());
}

/// normalize() uses the explicit `name` from ColumnRef instead of auto-generating when provided.
#[test]
fn normalize_inline_fk_uses_explicit_name_when_provided() {
    let col = Column {
        name: "user_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        references: Some(ColumnRef {
            table: "users".to_string(),
            column: "id".to_string(),
            name: Some("fk_posts_user".to_string()),
            on_delete: None,
        }),
        check: None,
        generated: None,
    };
    let table = Table {
        name: "posts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut s = Schema::default();
    s.tables.insert("posts".to_string(), table);
    s.normalize();
    assert_eq!(s.tables["posts"].foreign_keys[0].name, "fk_posts_user");
}

#[test]
fn yaml_accepts_composite_foreign_key_metadata() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: tenant_id
        type: integer
        primary_key: true
      - name: id
        type: integer
        primary_key: true
  orders:
    columns:
      - name: tenant_id
        type: integer
      - name: user_id
        type: integer
    foreign_keys:
      - name: orders_user_fkey
        columns: [tenant_id, user_id]
        to_table: users
        to_columns: [tenant_id, id]
"#,
        Dialect::Postgres,
    )
    .expect("prepared schema");

    let fk = &schema.tables["orders"].foreign_keys[0];
    assert_eq!(fk.name, "orders_user_fkey");
    assert_eq!(fk.columns, ["tenant_id", "user_id"]);
    assert_eq!(fk.to_table, "users");
    assert_eq!(fk.to_columns, ["tenant_id", "id"]);
}

#[test]
fn yaml_accepts_unnamed_derived_schema_objects() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: tenant_id
        type: integer
      - name: id
        type: integer
      - name: email
        type: text
      - name: active
        type: boolean
    primary_key:
      columns: [tenant_id, id]
    indexes:
      - columns: [email]
    constraints:
      - kind: unique
        columns: [tenant_id, email]
      - kind: check
        expression: active IN (true, false)
  orders:
    columns:
      - name: tenant_id
        type: integer
      - name: user_id
        type: integer
    foreign_keys:
      - columns: [tenant_id, user_id]
        to_table: users
        to_columns: [tenant_id, id]
"#,
        Dialect::Postgres,
    )
    .expect("prepared schema");

    let users = &schema.tables["users"];
    assert_eq!(users.primary_key.as_ref().unwrap().name, "users_pkey");
    assert_eq!(users.indexes[0].name, "users_email_idx");
    assert!(
        users
            .constraints
            .iter()
            .any(|constraint| matches!(constraint, Constraint::Unique { name, columns } if name == "users_tenant_id_email_key" && columns == &["tenant_id", "email"]))
    );
    assert!(users.constraints.iter().any(
        |constraint| matches!(constraint, Constraint::Check { name, .. } if name == "users_check")
    ));

    let fk = &schema.tables["orders"].foreign_keys[0];
    assert_eq!(fk.name, "orders_tenant_id_user_id_fkey");
}

#[test]
fn yaml_accepts_legacy_single_column_foreign_key_metadata() {
    let schema = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: integer
        primary_key: true
  posts:
    columns:
      - name: user_id
        type: integer
    foreign_keys:
      - name: posts_user_fkey
        from_column: user_id
        to_table: users
        to_column: id
"#,
        Dialect::Postgres,
    )
    .expect("prepared schema");

    let fk = &schema.tables["posts"].foreign_keys[0];
    assert_eq!(fk.columns, ["user_id"]);
    assert_eq!(fk.to_columns, ["id"]);
}

#[test]
fn prepare_rejects_invalid_composite_foreign_key_metadata() {
    let err = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: tenant_id
        type: integer
      - name: id
        type: integer
  orders:
    columns:
      - name: tenant_id
        type: integer
      - name: user_id
        type: integer
    foreign_keys:
      - name: orders_user_fkey
        columns: [tenant_id, user_id]
        to_table: users
        to_columns: [tenant_id, id]
"#,
        Dialect::Postgres,
    )
    .expect_err("non-unique target should fail");

    assert!(
        err.to_string()
            .contains("referenced columns 'users(tenant_id, id)' are not a primary key")
    );
}

#[test]
fn builder_composite_foreign_key_preserves_order() {
    let table = TableBuilder::new("orders")
        .column("tenant_id", "integer", |c| c)
        .column("user_id", "integer", |c| c)
        .foreign_key_named_columns(
            "orders_user_fkey",
            &["tenant_id", "user_id"],
            "users",
            &["tenant_id", "id"],
        )
        .build();

    let fk = &table.foreign_keys[0];
    assert_eq!(fk.name, "orders_user_fkey");
    assert_eq!(fk.columns, ["tenant_id", "user_id"]);
    assert_eq!(fk.to_columns, ["tenant_id", "id"]);
}

/// TableBuilder::column_from_type uses ColumnType metadata and then applies overrides.
#[test]
fn builder_column_from_type_infers_type_and_allows_overrides() {
    let table = TableBuilder::new("users")
        .column_from_type::<String>(&Dialect::Postgres, "name", |c| c.not_null())
        .column_from_type::<Option<i64>>(&Dialect::Postgres, "age", |c| c)
        .column_from_type::<Option<String>>(&Dialect::Postgres, "email", |c| c.not_null())
        .build();

    let name = table.columns.iter().find(|c| c.name == "name").unwrap();
    let age = table.columns.iter().find(|c| c.name == "age").unwrap();
    let email = table.columns.iter().find(|c| c.name == "email").unwrap();

    assert_eq!(name.col_type, "text");
    assert!(!name.nullable);
    assert_eq!(age.col_type, "bigint");
    assert!(age.nullable);
    assert_eq!(email.col_type, "text");
    assert!(!email.nullable);
}

#[test]
fn builder_unnamed_helpers_generate_deterministic_names() {
    let table = TableBuilder::new("orders")
        .column("tenant_id", "integer", |c| c)
        .column("user_id", "integer", |c| c)
        .column("email", "text", |c| c)
        .primary_key_columns(&["tenant_id", "user_id"])
        .foreign_key_columns(&["tenant_id", "user_id"], "users", &["tenant_id", "id"])
        .index_columns(&["email"])
        .unique_columns(&["tenant_id", "email"])
        .check_expr("tenant_id > 0")
        .build();

    assert_eq!(table.primary_key.as_ref().unwrap().name, "orders_pkey");
    assert_eq!(table.foreign_keys[0].name, "orders_tenant_id_user_id_fkey");
    assert_eq!(table.indexes[0].name, "orders_email_idx");
    assert!(
        table
            .constraints
            .iter()
            .any(|constraint| matches!(constraint, Constraint::Unique { name, .. } if name == "orders_tenant_id_email_key"))
    );
    assert!(table.constraints.iter().any(
        |constraint| matches!(constraint, Constraint::Check { name, .. } if name == "orders_check")
    ));
}

/// TableBuilder::trigger accepts the model directly and normalization derives names.
#[test]
fn builder_trigger_accepts_model_and_normalize_derives_name() {
    let table = TableBuilder::new("orders")
        .trigger(TriggerDef {
            name: None,
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: None,
            when: None,
            query: Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);".to_string()),
            language: None,
        })
        .build();
    let mut schema = Schema::default();
    schema.tables.insert("orders".to_string(), table);
    schema.normalize();

    let trigger = &schema.tables["orders"].triggers[0];
    assert_eq!(trigger.name.as_deref(), Some("orders_insert_after_trg"));
    assert_eq!(
        trigger.query.as_deref(),
        Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);")
    );
}

#[test]
fn generated_name_collision_fails_validation() {
    let err = Schema::from_yaml_str(
        r#"
tables:
  users:
    columns:
      - name: id
        type: integer
    indexes:
      - columns: [id]
      - columns: [id]
"#,
        Dialect::Postgres,
    )
    .expect_err("duplicate generated index name should fail");

    assert!(err.to_string().contains("duplicate index 'users_id_idx'"));
}

/// normalize() moves an inline `check` into `table.constraints` and clears the column field.
/// The constraint name is auto-generated as `{table}_{column}_check`.
#[test]
fn normalize_moves_inline_check_to_constraints() {
    let col = Column {
        name: "score".to_string(),
        col_type: "integer".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        references: None,
        check: Some("score >= 0".to_string()),
        generated: None,
    };
    let table = Table {
        name: "results".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut s = Schema::default();
    s.tables.insert("results".to_string(), table);
    s.normalize();
    let t = &s.tables["results"];
    assert_eq!(t.constraints.len(), 1);
    assert!(
        matches!(&t.constraints[0], Constraint::Check { name, expression } if name == "results_score_check" && expression == "score >= 0")
    );
    assert!(t.columns[0].check.is_none());
}

/// normalize() is idempotent — running it twice produces the same result.
#[test]
fn normalize_is_idempotent() {
    let col = Column {
        name: "user_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        references: Some(ColumnRef {
            table: "users".to_string(),
            column: "id".to_string(),
            name: None,
            on_delete: None,
        }),
        check: None,
        generated: None,
    };
    let table = Table {
        name: "posts".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![col],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    };
    let mut s = Schema::default();
    s.tables.insert("posts".to_string(), table);
    s.normalize();
    s.normalize();
    assert_eq!(s.tables["posts"].foreign_keys.len(), 1);
}

/// Column.col_type serializes as the key "type" in YAML, not "col_type".
#[test]
fn col_type_serializes_as_type_in_yaml() {
    let col = Column {
        name: "id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        default: None,
        primary_key: true,
        references: None,
        check: None,
        generated: None,
    };
    let yaml = serde_yaml::to_string(&col).expect("serialize");
    assert!(
        yaml.contains("type: bigint"),
        "expected 'type: bigint' in: {yaml}"
    );
    assert!(
        !yaml.contains("col_type"),
        "col_type should not appear in: {yaml}"
    );
    let back: Column = serde_yaml::from_str(&yaml).expect("deserialize");
    assert_eq!(back.col_type, "bigint");
}

fn basic_function(name: &str) -> FunctionDef {
    FunctionDef {
        name: name.to_string(),
        schema: None,
        arguments: String::new(),
        returns: "void".to_string(),
        language: "sql".to_string(),
        body: "SELECT 1".to_string(),
        volatility: Volatility::Volatile,
        security_definer: false,
    }
}

fn basic_trigger(name: &str) -> TriggerDef {
    TriggerDef {
        name: Some(name.to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: Some("some_fn".to_string()),
        when: None,
        query: None,
        language: None,
    }
}

/// CreateFunction inserts the function into state.
#[test]
fn create_function() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateFunction {
            function: basic_function("notify"),
        },
    );
    assert!(s.functions.contains_key("notify"));
}

/// CreateFunction on an existing name returns FunctionAlreadyExists.
#[test]
fn create_function_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateFunction {
            function: basic_function("notify"),
        },
    );
    let err = s
        .apply(&Operation::CreateFunction {
            function: basic_function("notify"),
        })
        .unwrap_err();
    assert_eq!(
        err,
        ReplayError::FunctionAlreadyExists("notify".to_string())
    );
}

/// DropFunction removes the function from state.
#[test]
fn drop_function() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateFunction {
            function: basic_function("notify"),
        },
    );
    apply_ok(
        &mut s,
        Operation::DropFunction {
            function: basic_function("notify"),
        },
    );
    assert!(!s.functions.contains_key("notify"));
}

/// DropFunction on a nonexistent function returns FunctionNotFound.
#[test]
fn drop_function_not_found() {
    let mut s = Schema::default();
    let err = s
        .apply(&Operation::DropFunction {
            function: basic_function("ghost"),
        })
        .unwrap_err();
    assert_eq!(err, ReplayError::FunctionNotFound("ghost".to_string()));
}

/// AlterFunction replaces the old function entry with the new one.
#[test]
fn alter_function() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateFunction {
            function: basic_function("notify"),
        },
    );
    let mut updated = basic_function("notify");
    updated.body = "SELECT 2".to_string();
    apply_ok(
        &mut s,
        Operation::AlterFunction {
            old: basic_function("notify"),
            new: updated.clone(),
        },
    );
    assert_eq!(s.functions["notify"].body, "SELECT 2");
}

/// CreateTrigger attaches a trigger to the table.
#[test]
fn create_trigger() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        },
    );
    assert_eq!(s.tables["users"].triggers.len(), 1);
}

/// CreateTrigger with a duplicate name returns TriggerAlreadyExists.
#[test]
fn create_trigger_duplicate() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        },
    );
    let err = s
        .apply(&Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        })
        .unwrap_err();
    assert_eq!(
        err,
        ReplayError::TriggerAlreadyExists {
            table: "users".to_string(),
            trigger: "audit_trg".to_string()
        }
    );
}

/// DropTrigger removes the trigger from the table.
#[test]
fn drop_trigger() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        },
    );
    apply_ok(
        &mut s,
        Operation::DropTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        },
    );
    assert!(s.tables["users"].triggers.is_empty());
}

/// DropTrigger on a nonexistent trigger returns TriggerNotFound.
#[test]
fn drop_trigger_not_found() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let err = s
        .apply(&Operation::DropTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("ghost_trg"),
        })
        .unwrap_err();
    assert_eq!(
        err,
        ReplayError::TriggerNotFound {
            table: "users".to_string(),
            trigger: "ghost_trg".to_string()
        }
    );
}

/// normalize() auto-generates a trigger name from table + events + timing.
#[test]
fn normalize_auto_generates_trigger_name() {
    let yaml = r#"
tables:
  users:
    name: users
    columns: []
    foreign_keys: []
    indexes: []
    constraints: []
    triggers:
      - timing: after
        events: [update, insert]
        scope: row
        function_name: some_fn
"#;
    let state = Schema::from_yaml_str(yaml, Dialect::Postgres).expect("parse");
    let trigger = &state.tables["users"].triggers[0];
    assert_eq!(
        trigger.name.as_deref(),
        Some("users_insert_update_after_trg")
    );
}

/// normalize() preserves trigger query source exactly.
#[test]
fn normalize_preserves_trigger_query() {
    let yaml = r#"
tables:
  orders:
    name: orders
    columns: []
    foreign_keys: []
    indexes: []
    constraints: []
    triggers:
      - name: orders_insert_after_trg
        timing: after
        events: [insert]
        scope: row
        query: "INSERT INTO audit_log(order_id) VALUES (NEW.id);"
"#;
    let state = Schema::from_yaml_str(yaml, Dialect::Postgres).expect("parse");
    let trigger = &state.tables["orders"].triggers[0];
    assert_eq!(
        trigger.query.as_deref(),
        Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);")
    );
    assert!(trigger.function_name.is_none());
    assert!(state.functions.is_empty());
}

/// Loading removed trigger body syntax returns a clear unknown-field error.
#[test]
fn trigger_body_is_rejected() {
    let yaml = r#"
tables:
  orders:
    name: orders
    columns: []
    triggers:
      - name: orders_insert_after_trg
        timing: after
        events: [insert]
        scope: row
        body: "BEGIN RETURN NEW; END;"
"#;
    let err = Schema::from_yaml_str(yaml, Dialect::Postgres)
        .unwrap_err()
        .to_string();
    assert!(err.contains("body"), "expected body in error, got {err}");
    assert!(err.contains("query"), "expected query in error, got {err}");
}

/// validate() rejects a trigger with no events.
#[test]
fn validate_trigger_empty_events() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![TriggerDef {
            name: Some("t".to_string()),
            timing: TriggerTiming::After,
            events: vec![],
            scope: TriggerScope::Row,
            function_name: Some("fn".to_string()),
            when: None,
            query: None,
            language: None,
        }],
    };
    s.tables.insert("users".to_string(), table);
    assert!(s.validate().unwrap_err().contains("no events"));
}

/// validate() rejects a trigger with no query or function_name after normalize.
#[test]
fn validate_trigger_no_source() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![TriggerDef {
            name: Some("t".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: None,
            when: None,
            query: None,
            language: None,
        }],
    };
    s.tables.insert("users".to_string(), table);
    assert!(
        s.validate()
            .unwrap_err()
            .contains("must set either `function_name` or `query`")
    );
}

/// validate() rejects a trigger that mixes query and function_name.
#[test]
fn validate_trigger_rejects_query_and_function_name() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![TriggerDef {
            name: Some("t".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("fn".to_string()),
            when: None,
            query: Some("SELECT 1".to_string()),
            language: None,
        }],
    };
    s.tables.insert("users".to_string(), table);
    assert!(s.validate().unwrap_err().contains("not both"));
}

/// validate() rejects blank query strings as missing trigger source.
#[test]
fn validate_trigger_rejects_empty_query() {
    let mut s = Schema::default();
    let table = Table {
        name: "users".to_string(),
        schema: None,
        primary_key: None,
        columns: vec![],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![TriggerDef {
            name: Some("t".to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: None,
            when: None,
            query: Some("   ".to_string()),
            language: None,
        }],
    };
    s.tables.insert("users".to_string(), table);
    assert!(
        s.validate()
            .unwrap_err()
            .contains("must set either `function_name` or `query`")
    );
}

/// Tables are stored in a BTreeMap so keys are always alphabetical regardless of creation order.
#[test]
fn tables_always_in_alphabetical_order() {
    let mut s = Schema::default();
    for name in &["zebra", "alpha", "mango", "bravo"] {
        apply_ok(
            &mut s,
            Operation::CreateTable {
                table: basic_table(name),
            },
        );
    }
    let keys: Vec<&str> = s.tables.keys().map(String::as_str).collect();
    assert_eq!(keys, vec!["alpha", "bravo", "mango", "zebra"]);
}

/// Columns are stored in a Vec so insertion order is always preserved.
#[test]
fn columns_preserve_insertion_order() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    for name in &["c", "a", "b"] {
        apply_ok(
            &mut s,
            Operation::AddColumn {
                table_name: "users".to_string(),
                column: text_col(name),
            },
        );
    }
    let names: Vec<&str> = s.tables["users"]
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, vec!["c", "a", "b"]);
}

/// Dropping the middle column preserves surrounding columns in their original positions.
#[test]
fn drop_middle_column_preserves_order() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    for name in &["first", "middle", "last"] {
        apply_ok(
            &mut s,
            Operation::AddColumn {
                table_name: "users".to_string(),
                column: text_col(name),
            },
        );
    }
    apply_ok(
        &mut s,
        Operation::DropColumn {
            table_name: "users".to_string(),
            column: text_col("middle"),
            cascade: false,
        },
    );
    let names: Vec<&str> = s.tables["users"]
        .columns
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, vec!["first", "last"]);
}

/// CreateTable with inline columns produces the same state as CreateTable + AddColumn for each.
#[test]
fn create_table_inline_vs_incremental_are_equal() {
    let cols = vec![text_col("name"), text_col("email"), text_col("bio")];
    let mut s1 = Schema::default();
    apply_ok(
        &mut s1,
        Operation::CreateTable {
            table: Table {
                name: "users".to_string(),
                schema: None,
                primary_key: None,
                columns: cols.clone(),
                foreign_keys: vec![],
                indexes: vec![],
                constraints: vec![],
                triggers: vec![],
            },
        },
    );

    let mut s2 = Schema::default();
    apply_ok(
        &mut s2,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    for col in cols {
        apply_ok(
            &mut s2,
            Operation::AddColumn {
                table_name: "users".to_string(),
                column: col,
            },
        );
    }

    assert_eq!(s1, s2);
}

/// Applying inverse(op) immediately after op restores the original state for all invertible ops.
#[test]
fn inverse_restores_state_for_all_invertible_ops() {
    let fk = ForeignKey::single("fk_x", "col", "other", "id");
    let idx = Index {
        name: "idx_col".to_string(),
        columns: vec!["col".to_string()],
        unique: true,
        predicate: None,
    };
    let chk = Constraint::Check {
        name: "chk_col".to_string(),
        expression: "col != ''".to_string(),
    };

    let verify = |mut s: Schema, op: Operation| {
        let before = s.clone();
        s.apply(&op).expect("op should apply");
        s.apply(&op.inverse().expect("should be invertible"))
            .expect("inverse should apply");
        assert_eq!(
            s,
            before,
            "inverse of '{}' did not restore state",
            op.type_name()
        );
    };

    verify(
        Schema::default(),
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );

    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    verify(
        s.clone(),
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("col"),
        },
    );

    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("col"),
        },
    );
    verify(
        s.clone(),
        Operation::AddForeignKey {
            table_name: "users".to_string(),
            foreign_key: fk,
        },
    );

    verify(
        s.clone(),
        Operation::AddIndex {
            table_name: "users".to_string(),
            index: idx,
            concurrent: false,
        },
    );

    verify(
        s.clone(),
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: chk,
        },
    );

    verify(
        s.clone(),
        Operation::RenameTable {
            old_name: "users".to_string(),
            new_name: "accounts".to_string(),
        },
    );

    verify(
        s.clone(),
        Operation::AlterColumn {
            table_name: "users".to_string(),
            old: text_col("col"),
            new: Column {
                name: "col".to_string(),
                col_type: "varchar(100)".to_string(),
                nullable: true,
                ..Default::default()
            },
            cast_expr: None,
        },
    );

    verify(
        s.clone(),
        Operation::RenameColumn {
            table_name: "users".to_string(),
            old_name: "col".to_string(),
            new_name: "alias".to_string(),
        },
    );
}

/// RenameTable followed by its inverse restores original state.
#[test]
fn rename_table_inverse_restores_state() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    let before = s.clone();
    let op = Operation::RenameTable {
        old_name: "users".to_string(),
        new_name: "accounts".to_string(),
    };
    apply_ok(&mut s, op.clone());
    apply_ok(&mut s, op.inverse().unwrap());
    assert_eq!(s, before);
}

/// RenameColumn followed by its inverse restores original state.
#[test]
fn rename_column_inverse_restores_state() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    let before = s.clone();
    let op = Operation::RenameColumn {
        table_name: "users".to_string(),
        old_name: "email".to_string(),
        new_name: "email_address".to_string(),
    };
    apply_ok(&mut s, op.clone());
    apply_ok(&mut s, op.inverse().unwrap());
    assert_eq!(s, before);
}

/// AlterColumn followed by its inverse restores original state.
#[test]
fn alter_column_inverse_restores_state() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("bio"),
        },
    );
    let before = s.clone();
    let new_col = Column {
        name: "bio".to_string(),
        col_type: "varchar(500)".to_string(),
        nullable: true,
        ..Default::default()
    };
    let op = Operation::AlterColumn {
        table_name: "users".to_string(),
        old: text_col("bio"),
        new: new_col,
        cast_expr: None,
    };
    apply_ok(&mut s, op.clone());
    apply_ok(&mut s, op.inverse().unwrap());
    assert_eq!(s, before);
}

/// Replay is idempotent: applying the same operation sequence on two fresh states gives identical results.
#[test]
fn replay_is_idempotent() {
    let ops = vec![
        Operation::CreateTable {
            table: basic_table("users"),
        },
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
        Operation::CreateTable {
            table: basic_table("posts"),
        },
        Operation::AddColumn {
            table_name: "posts".to_string(),
            column: text_col("title"),
        },
        Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: ForeignKey::single("fk_posts_user", "user_id", "users", "id"),
        },
    ];
    let mut s1 = Schema::default();
    let mut s2 = Schema::default();
    for op in &ops {
        s1.apply(op).unwrap();
        s2.apply(op).unwrap();
    }
    assert_eq!(s1, s2);
}

/// Full multi-operation replay produces the exact expected state.
#[test]
fn full_replay_produces_exact_state() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: Column {
                name: "age".to_string(),
                col_type: "integer".to_string(),
                nullable: true,
                ..Default::default()
            },
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("posts"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "posts".to_string(),
            column: text_col("title"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "posts".to_string(),
            column: text_col("user_id"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddForeignKey {
            table_name: "posts".to_string(),
            foreign_key: ForeignKey::single("fk_posts_user_id", "user_id", "users", "id"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddIndex {
            table_name: "users".to_string(),
            index: Index {
                name: "users_email_idx".to_string(),
                columns: vec!["email".to_string()],
                unique: true,
                predicate: None,
            },
            concurrent: false,
        },
    );
    apply_ok(
        &mut s,
        Operation::DropColumn {
            table_name: "users".to_string(),
            column: Column {
                name: "age".to_string(),
                col_type: "integer".to_string(),
                nullable: true,
                ..Default::default()
            },
            cascade: false,
        },
    );

    assert_eq!(s.tables.len(), 2);
    let users = &s.tables["users"];
    assert_eq!(users.columns.len(), 1);
    assert_eq!(users.columns[0].name, "email");
    assert_eq!(users.indexes.len(), 1);

    let posts = &s.tables["posts"];
    assert_eq!(
        posts
            .columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["title", "user_id"]
    );
    assert_eq!(posts.foreign_keys[0].name, "fk_posts_user_id");
}

/// Statement operations are transparent — they do not mutate existing state.
#[test]
fn statement_is_transparent_to_existing_state() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateTable {
            table: basic_table("users"),
        },
    );
    apply_ok(
        &mut s,
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
    );
    let before = s.clone();

    apply_ok(
        &mut s,
        Operation::Statement {
            up: "UPDATE users SET email = 'x'".to_string(),
            down: None,
        },
    );
    assert_eq!(s, before);
}

/// schema_qualified_key: None and "public" both produce the bare table name.
#[test]
fn schema_qualified_key_public_and_none_are_bare() {
    assert_eq!(schema_qualified_key("users", None), "users");
    assert_eq!(schema_qualified_key("users", Some("public")), "users");
}

/// schema_qualified_key: any non-public schema produces "schema.name".
#[test]
fn schema_qualified_key_non_public_is_qualified() {
    assert_eq!(schema_qualified_key("users", Some("myapp")), "myapp.users");
    assert_eq!(
        schema_qualified_key("orders", Some("billing")),
        "billing.orders"
    );
}

/// CreateView and DropView use schema_qualified_key for their map key.
#[test]
fn create_view_key_respects_schema() {
    let mut s = Schema::default();
    apply_ok(
        &mut s,
        Operation::CreateView {
            view: ViewDef {
                name: "active_users".to_string(),
                schema: None,
                definition: "SELECT 1".to_string(),
            },
        },
    );
    apply_ok(
        &mut s,
        Operation::CreateView {
            view: ViewDef {
                name: "summary".to_string(),
                schema: Some("reporting".to_string()),
                definition: "SELECT 2".to_string(),
            },
        },
    );
    assert!(
        s.views.contains_key("active_users"),
        "public view uses bare key"
    );
    assert!(
        s.views.contains_key("reporting.summary"),
        "non-public view uses qualified key"
    );
}

/// Applying all operations in order then replaying the chain produces an identical state.
#[test]
fn sequential_replay_matches_single_pass() {
    let ops = vec![
        Operation::CreateTable {
            table: basic_table("users"),
        },
        Operation::AddColumn {
            table_name: "users".to_string(),
            column: text_col("email"),
        },
        Operation::AddConstraint {
            table_name: "users".to_string(),
            constraint: Constraint::Unique {
                name: "uq_email".to_string(),
                columns: vec!["email".to_string()],
            },
        },
        Operation::CreateTable {
            table: basic_table("orders"),
        },
        Operation::RenameTable {
            old_name: "orders".to_string(),
            new_name: "purchases".to_string(),
        },
    ];

    let apply_all = |ops: &Vec<Operation>| {
        let mut s = Schema::default();
        for op in ops {
            s.apply(op).unwrap();
        }
        s
    };

    let s1 = apply_all(&ops);
    let s2 = apply_all(&ops);
    assert_eq!(s1, s2);
    assert!(
        !s1.tables.contains_key("orders"),
        "renamed table should not exist under old name"
    );
    assert!(s1.tables.contains_key("purchases"));
    assert_eq!(s1.tables["users"].constraints[0].name(), "uq_email");
}

/// builder().table::<T>() inserts the table using schema_qualified_key.
#[test]
fn builder_table_via_into_table() {
    struct Posts;
    impl IntoTable for Posts {
        fn into_table(_dialect: &Dialect) -> Table {
            TableBuilder::new("posts")
                .id()
                .column("title", "text", |c| c.not_null())
                .build()
        }
    }

    let state = Schema::builder(Dialect::Postgres).table::<Posts>().build();
    assert!(state.tables.contains_key("posts"));
    assert_eq!(state.tables["posts"].columns.len(), 2);
    assert_eq!(state.tables["posts"].columns[0].name, "id");
    assert!(state.tables["posts"].columns[0].primary_key);
}

/// builder().extension() registers an extension without a version.
#[test]
fn builder_extension() {
    let state = Schema::builder(Dialect::Postgres)
        .extension("pgcrypto")
        .build();
    assert!(state.extensions.contains_key("pgcrypto"));
    assert!(state.extensions["pgcrypto"].version.is_none());
}

/// builder().enum_type() registers a named enum with the given values.
#[test]
fn builder_enum_type() {
    let state = Schema::builder(Dialect::Postgres)
        .enum_type("status", &["active", "inactive"])
        .build();
    assert!(state.enums.contains_key("status"));
    assert_eq!(state.enums["status"].values, vec!["active", "inactive"]);
}

/// qualified_name() returns plain name when schema is None.
#[test]
fn qualified_name_no_schema() {
    let t = basic_table("users");
    assert_eq!(t.qualified_name(), "users");
}

/// qualified_name() returns plain name when schema is "public".
#[test]
fn qualified_name_public_schema() {
    let t = Table {
        schema: Some("public".to_string()),
        ..basic_table("users")
    };
    assert_eq!(t.qualified_name(), "users");
}

/// qualified_name() returns schema.name for non-public schemas.
#[test]
fn qualified_name_custom_schema() {
    let t = Table {
        schema: Some("analytics".to_string()),
        ..basic_table("events")
    };
    assert_eq!(t.qualified_name(), "analytics.events");
}

/// qualified_name() works on all entity types with schema fields.
#[test]
fn qualified_name_all_entities() {
    let f = FunctionDef {
        name: "my_fn".to_string(),
        schema: Some("utils".to_string()),
        arguments: String::new(),
        returns: "void".to_string(),
        language: "sql".to_string(),
        body: String::new(),
        volatility: Volatility::Volatile,
        security_definer: false,
    };
    assert_eq!(f.qualified_name(), "utils.my_fn");

    let v = ViewDef {
        name: "v1".to_string(),
        schema: None,
        definition: "SELECT 1".to_string(),
    };
    assert_eq!(v.qualified_name(), "v1");

    let e = ExtensionDef {
        name: "pgcrypto".to_string(),
        schema: Some("public".to_string()),
        version: None,
    };
    assert_eq!(e.qualified_name(), "pgcrypto");

    let en = EnumDef {
        name: "status".to_string(),
        schema: Some("core".to_string()),
        values: vec![],
    };
    assert_eq!(en.qualified_name(), "core.status");
}

/// canonicalize normalizes schema: Some("public") to None for tables.
#[test]
fn canonicalize_public_schema_to_none() {
    let mut s = Schema::default();
    s.tables.insert(
        "users".to_string(),
        Table {
            schema: Some("public".to_string()),
            ..basic_table("users")
        },
    );
    s.canonicalize(&Dialect::Postgres);
    assert_eq!(s.tables["users"].schema, None);
}

/// canonicalize normalizes schema: Some("pg_catalog") to None for extensions.
#[test]
fn canonicalize_pg_catalog_extension() {
    let mut s = Schema::default();
    s.extensions.insert(
        "plpgsql".to_string(),
        ExtensionDef {
            name: "plpgsql".to_string(),
            schema: Some("pg_catalog".to_string()),
            version: None,
        },
    );
    s.canonicalize(&Dialect::Postgres);
    assert_eq!(s.extensions["plpgsql"].schema, None);
}

/// canonicalize does NOT normalize pg_catalog for non-extension entities.
#[test]
fn canonicalize_pg_catalog_not_for_tables() {
    let mut s = Schema::default();
    s.tables.insert(
        "pg_catalog.sometable".to_string(),
        Table {
            name: "sometable".to_string(),
            schema: Some("pg_catalog".to_string()),
            ..basic_table("sometable")
        },
    );
    s.canonicalize(&Dialect::Postgres);
    assert_eq!(
        s.tables["pg_catalog.sometable"].schema,
        Some("pg_catalog".to_string())
    );
}

/// canonicalize re-keys BTreeMaps when schema normalization changes the qualified name.
#[test]
fn canonicalize_rekeys_btreemap() {
    let mut s = Schema::default();
    s.enums.insert(
        "public.status".to_string(),
        EnumDef {
            name: "status".to_string(),
            schema: Some("public".to_string()),
            values: vec!["active".to_string()],
        },
    );
    s.canonicalize(&Dialect::Postgres);
    assert!(!s.enums.contains_key("public.status"));
    assert!(s.enums.contains_key("status"));
    assert_eq!(s.enums["status"].schema, None);
}

/// apply(CreateTable) with non-public schema inserts under qualified key.
#[test]
fn apply_create_table_qualified_key() {
    let mut s = Schema::default();
    let table = Table {
        name: "events".to_string(),
        schema: Some("analytics".to_string()),
        ..basic_table("events")
    };
    apply_ok(&mut s, Operation::CreateTable { table });
    assert!(s.tables.contains_key("analytics.events"));
    assert!(!s.tables.contains_key("events"));
}

/// Verifies inline column references preserve canonical on_delete metadata during normalization.
#[test]
fn normalize_inline_fk_preserves_on_delete() {
    let mut schema = Schema::default();
    schema.tables.insert(
        "users".to_string(),
        Table {
            name: "users".to_string(),
            columns: vec![Column {
                name: "id".to_string(),
                col_type: "integer".to_string(),
                primary_key: true,
                ..Default::default()
            }],
            ..basic_table("users")
        },
    );
    schema.tables.insert(
        "posts".to_string(),
        Table {
            name: "posts".to_string(),
            columns: vec![Column {
                name: "user_id".to_string(),
                col_type: "integer".to_string(),
                references: Some(ColumnRef {
                    table: "users".to_string(),
                    column: "id".to_string(),
                    name: None,
                    on_delete: Some("CASCADE".to_string()),
                }),
                ..Default::default()
            }],
            ..basic_table("posts")
        },
    );

    schema.normalize();

    assert_eq!(
        schema.tables["posts"].foreign_keys[0].on_delete.as_deref(),
        Some("cascade")
    );
}

/// Verifies schema validation rejects unsupported foreign-key delete actions.
#[test]
fn validate_rejects_invalid_foreign_key_on_delete() {
    let mut schema = Schema::default();
    schema.tables.insert(
        "users".to_string(),
        Table {
            name: "users".to_string(),
            columns: vec![Column {
                name: "id".to_string(),
                col_type: "integer".to_string(),
                primary_key: true,
                ..Default::default()
            }],
            ..basic_table("users")
        },
    );
    let mut foreign_key = ForeignKey::single("posts_user_id_fkey", "user_id", "users", "id");
    foreign_key.on_delete = Some("explode".to_string());
    schema.tables.insert(
        "posts".to_string(),
        Table {
            name: "posts".to_string(),
            columns: vec![Column {
                name: "user_id".to_string(),
                col_type: "integer".to_string(),
                ..Default::default()
            }],
            foreign_keys: vec![foreign_key],
            ..basic_table("posts")
        },
    );

    let err = schema
        .validate()
        .expect_err("invalid on_delete should fail");

    assert!(err.contains("unsupported on_delete action 'explode'"));
}

mod property_states {
    use super::*;
    use std::collections::BTreeSet;

    use proptest::prelude::*;

    fn arb_identifier() -> impl Strategy<Value = String> {
        prop_oneof![
            "[a-z][a-z0-9_]{0,10}".prop_map(|value| value),
            "[A-Z][A-Za-z0-9_]{0,10}".prop_map(|value| value),
            "[a-z]{1,8}_[0-9]{1,3}".prop_map(|value| value),
            Just("user".to_string()),
            Just("order".to_string()),
            Just("very_long_identifier_name_for_property_tests".to_string()),
        ]
    }

    fn arb_column_name() -> impl Strategy<Value = String> {
        "[a-z][a-z0-9_]{0,8}".prop_map(|value| value)
    }

    fn arb_column() -> impl Strategy<Value = Column> {
        let types = prop_oneof![
            Just("integer".to_string()),
            Just("text".to_string()),
            Just("boolean".to_string()),
            Just("bigint".to_string()),
        ];
        (arb_column_name(), types, any::<bool>(), any::<bool>()).prop_map(
            |(name, col_type, nullable, primary_key)| Column {
                name,
                col_type,
                nullable,
                primary_key,
                default: None,
                references: None,
                check: None,
                generated: None,
            },
        )
    }

    fn arb_table() -> impl Strategy<Value = Table> {
        (
            arb_identifier(),
            proptest::collection::vec(arb_column(), 1..6),
        )
            .prop_map(|(name, columns)| {
                let mut seen = BTreeSet::new();
                let columns = columns
                    .into_iter()
                    .filter(|column| seen.insert(column.name.clone()))
                    .collect::<Vec<_>>();
                Table {
                    name,
                    schema: None,
                    primary_key: None,
                    columns,
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                }
            })
            .prop_filter("table must retain at least one unique column", |table| {
                !table.columns.is_empty()
            })
    }

    fn arb_schema() -> impl Strategy<Value = Schema> {
        proptest::collection::vec(arb_table(), 0..5).prop_map(|tables| {
            let mut schema = Schema::default();
            for table in tables {
                schema.tables.insert(table.name.clone(), table);
            }
            schema
        })
    }

    fn no_duplicate_table_children(schema: &Schema) -> bool {
        schema.tables.values().all(|table| {
            unique(table.columns.iter().map(|column| column.name.as_str()))
                && unique(table.foreign_keys.iter().map(|fk| fk.name.as_str()))
                && unique(table.indexes.iter().map(|index| index.name.as_str()))
                && unique(table.constraints.iter().map(Constraint::name))
        })
    }

    fn unique<'a>(values: impl IntoIterator<Item = &'a str>) -> bool {
        let mut seen = BTreeSet::new();
        values.into_iter().all(|value| seen.insert(value))
    }

    fn create_table_operation(table: Table) -> Operation {
        Operation::CreateTable { table }
    }

    fn apply_all(operations: &[Operation]) -> Result<Schema, String> {
        let mut schema = Schema::default();
        for operation in operations {
            schema
                .apply(operation)
                .map_err(|error| format!("failed to apply {operation:?}: {error}"))?;
        }
        Ok(schema)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        #[doc = "Schema normalization is idempotent for generated valid schemas."]
        fn normalize_is_idempotent_for_generated_schemas(mut schema in arb_schema()) {
            schema.normalize();
            let once = schema.clone();
            schema.normalize();
            prop_assert_eq!(schema, once);
        }

        #[test]
        #[doc = "Schema preparation remains idempotent and does not create duplicate child names."]
        fn prepare_is_idempotent_and_keeps_child_names_unique(schema in arb_schema()) {
            let mut prepared = schema.clone();
            prepared
                .prepare_mut(&Dialect::Postgres)
                .expect("generated schema should prepare");
            let once = prepared.clone();
            prepared
                .prepare_mut(&Dialect::Postgres)
                .expect("prepared schema should prepare again");
            prop_assert_eq!(&prepared, &once);
            prop_assert!(no_duplicate_table_children(&prepared));
        }

        #[test]
        #[doc = "Column primary-key shorthand normalizes to deterministic table-level primary-key metadata."]
        fn primary_key_flags_normalize_to_ordered_table_primary_key(
            table_name in arb_identifier(),
            column_names in proptest::collection::vec(arb_column_name(), 1..6),
        ) {
            let mut seen = BTreeSet::new();
            let column_names = column_names
                .into_iter()
                .filter(|name| seen.insert(name.clone()))
                .collect::<Vec<_>>();
            prop_assume!(!column_names.is_empty());
            let table = Table {
                name: table_name.clone(),
                schema: None,
                primary_key: None,
                columns: column_names
                    .iter()
                    .map(|name| Column {
                        name: name.clone(),
                        col_type: "integer".to_string(),
                        nullable: true,
                        primary_key: true,
                        ..Default::default()
                    })
                    .collect(),
                foreign_keys: vec![],
                indexes: vec![],
                constraints: vec![],
                triggers: vec![],
            };

            let mut schema = Schema::default();
            schema.tables.insert(table_name.clone(), table);
            schema.normalize();
            let table = schema
                .tables
                .remove(&table_name)
                .expect("normalized table should remain present");

            let pk = table.primary_key.expect("primary key should be derived");
            prop_assert_eq!(pk.name, Table::pk_constraint_name_for(&table_name));
            prop_assert_eq!(pk.columns, column_names);
            prop_assert!(table.columns.iter().all(|column| column.primary_key && !column.nullable));
        }

        #[test]
        #[doc = "Replaying the same generated create-table chain twice produces identical schemas."]
        fn replay_create_table_chain_is_deterministic(tables in proptest::collection::vec(arb_table(), 0..5)) {
            let mut seen = BTreeSet::new();
            let operations = tables
                .into_iter()
                .filter(|table| seen.insert(table.name.clone()))
                .map(create_table_operation)
                .collect::<Vec<_>>();

            let first = apply_all(&operations);
            prop_assert!(first.is_ok(), "first replay failed: {:?}", first.err());
            let second = apply_all(&operations);
            prop_assert!(second.is_ok(), "second replay failed: {:?}", second.err());
            let first = first.expect("checked ok");
            let second = second.expect("checked ok");
            prop_assert_eq!(first, second);
        }

        #[test]
        #[doc = "Independent create-table replay order produces the same final schema."]
        fn independent_create_table_branches_commute(left in arb_table(), right in arb_table()) {
            prop_assume!(left.name != right.name);
            let left_first = apply_all(&[
                create_table_operation(left.clone()),
                create_table_operation(right.clone()),
            ]);
            prop_assert!(left_first.is_ok(), "left-first replay failed: {:?}", left_first.err());
            let right_first = apply_all(&[
                create_table_operation(right),
                create_table_operation(left),
            ]);
            prop_assert!(right_first.is_ok(), "right-first replay failed: {:?}", right_first.err());
            let left_first = left_first.expect("checked ok");
            let right_first = right_first.expect("checked ok");
            prop_assert_eq!(left_first, right_first);
        }
    }
}
