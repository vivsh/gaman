use gaman::migrations::Migration;
use gaman::operations::Operation;
use gaman::states::{Column, ForeignKey, Index, Table};

pub fn col(name: &str, col_type: &str) -> Column {
    Column { name: name.into(), col_type: col_type.into(), nullable: false, default: None, primary_key: false, ..Default::default() }
}

pub fn pk_col(name: &str) -> Column {
    Column { name: name.into(), col_type: "integer".into(), nullable: false, default: None, primary_key: true, ..Default::default() }
}

pub fn nullable_col(name: &str, col_type: &str) -> Column {
    Column { name: name.into(), col_type: col_type.into(), nullable: true, default: None, primary_key: false, ..Default::default() }
}

pub fn users_table() -> Table {
    Table {
        name: "users".into(),
        schema: None,
        columns: vec![pk_col("id"), col("username", "text")],
        foreign_keys: vec![],
        indexes: vec![],
        constraints: vec![],
        triggers: vec![],
    }
}

pub fn posts_table() -> Table {
    Table {
        name: "posts".into(),
        schema: None,
        columns: vec![pk_col("id"), col("title", "text"), col("user_id", "integer")],
        foreign_keys: vec![ForeignKey {
            name: "posts_user_id_fk".into(),
            from_column: "user_id".into(),
            to_table: "users".into(),
            to_column: "id".into(),
        }],
        indexes: vec![Index { name: "posts_user_id_idx".into(), columns: vec!["user_id".into()], unique: false, predicate: None }],
        constraints: vec![],
        triggers: vec![],
    }
}

/// 3-migration chain: create users → add email column → create posts
pub fn three_migration_chain() -> Vec<Migration> {
    let m1 = Migration {
        id: "0001_create_users".into(),
        dependencies: vec![],
        operations: vec![Operation::CreateTable { table: users_table() }],
    };
    let m2 = Migration {
        id: "0002_add_email".into(),
        dependencies: vec!["0001_create_users".into()],
        operations: vec![Operation::AddColumn {
            table_name: "users".into(),
            column: nullable_col("email", "text"),
        }],
    };
    let m3 = Migration {
        id: "0003_create_posts".into(),
        dependencies: vec!["0002_add_email".into()],
        operations: vec![Operation::CreateTable { table: posts_table() }],
    };
    vec![m1, m2, m3]
}

/// A single migration that creates a table with a raw SQL op that has no inverse.
pub fn irreversible_migration() -> Migration {
    Migration {
        id: "0001_irreversible".into(),
        dependencies: vec![],
        operations: vec![Operation::Statement {
            up: "SELECT 1".into(),
            down: None,
        }],
    }
}
