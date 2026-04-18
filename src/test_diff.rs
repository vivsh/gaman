use super::*;
use crate::states::{
    Column, EnumDef, ExtensionDef, ForeignKey, FunctionDef,
    Index, Table, TriggerDef, TriggerEvent, TriggerScope, TriggerTiming,
    ViewDef, Volatility,
};

fn empty_schema() -> Schema { Schema::default() }

fn empty_table(name: &str) -> Table {
    Table {
        name: name.to_string(), schema: None, columns: vec![],
        foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![],
    }
}

fn text_col(name: &str) -> Column {
    Column {
        name: name.to_string(), col_type: "text".to_string(),
        nullable: false, default: None, primary_key: false, ..Default::default()
    }
}

fn int_col(name: &str) -> Column {
    Column {
        name: name.to_string(), col_type: "integer".to_string(),
        nullable: false, default: None, primary_key: false, ..Default::default()
    }
}

fn basic_function(name: &str) -> FunctionDef {
    FunctionDef {
        name: name.to_string(), schema: None,
        arguments: String::new(), returns: "void".to_string(),
        language: "sql".to_string(), body: "SELECT 1".to_string(),
        volatility: Volatility::Volatile, security_definer: false,
    }
}

fn basic_trigger(name: &str, fn_name: &str) -> TriggerDef {
    TriggerDef {
        name: Some(name.to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: Some(fn_name.to_string()),
        when: None, body: None, language: None,
    }
}

fn schema_with_table(t: Table) -> Schema {
    let mut s = Schema::default();
    s.tables.insert(t.name.clone(), t);
    s
}

fn schema_with_enum(e: EnumDef) -> Schema {
    let mut s = Schema::default();
    s.enums.insert(e.name.clone(), e);
    s
}

fn op_names(ops: &[Operation]) -> Vec<&'static str> {
    ops.iter().map(|op| op.type_name()).collect()
}

// -- generate_diff -----------------------------------------------------------

/// Identical empty schemas produce no operations.
#[test]
fn gen_identical_empty_schemas() {
    let s = empty_schema();
    assert!(generate_diff(&s, &s).is_empty());
}

/// Identical non-empty schemas produce no operations.
#[test]
fn gen_identical_schemas_no_diff() {
    let mut t = empty_table("users");
    t.columns.push(text_col("name"));
    let s = schema_with_table(t);
    assert!(generate_diff(&s, &s).is_empty());
}

/// Adding a new table produces a single CreateTable.
#[test]
fn gen_new_table() {
    let ops = generate_diff(&schema_with_table(empty_table("users")), &empty_schema());
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::CreateTable { table } if table.name == "users"));
}

/// Removing a table produces a single DropTable.
#[test]
fn gen_drop_table() {
    let ops = generate_diff(&empty_schema(), &schema_with_table(empty_table("users")));
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::DropTable { table } if table.name == "users"));
}

/// Adding a column to an existing table produces AddColumn (not CreateTable).
#[test]
fn gen_add_column() {
    let prev = schema_with_table(empty_table("users"));
    let mut t = empty_table("users");
    t.columns.push(text_col("email"));
    let ops = generate_diff(&schema_with_table(t), &prev);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::AddColumn { table_name, column }
        if table_name == "users" && column.name == "email"));
}

/// Changing a column type produces AlterColumn.
#[test]
fn gen_alter_column() {
    let mut t1 = empty_table("users");
    t1.columns.push(text_col("age"));
    let mut t2 = empty_table("users");
    t2.columns.push(int_col("age"));
    let ops = generate_diff(&schema_with_table(t2), &schema_with_table(t1));
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::AlterColumn { table_name, .. } if table_name == "users"));
}

/// Creating a new table with columns produces only CreateTable, not
/// CreateTable + AddColumn for each column (sub-entity suppression).
#[test]
fn gen_new_table_suppresses_sub_entity_adds() {
    let mut t = empty_table("users");
    t.columns.push(text_col("name"));
    t.columns.push(text_col("email"));
    t.indexes.push(Index {
        name: "idx_email".into(), columns: vec!["email".into()],
        unique: true, predicate: None,
    });
    let ops = generate_diff(&schema_with_table(t), &empty_schema());
    assert_eq!(ops.len(), 1, "only CreateTable expected, got: {:?}", op_names(&ops));
    assert!(matches!(&ops[0], Operation::CreateTable { .. }));
}

/// Dropping a table with columns produces only DropTable, not
/// DropTable + DropColumn for each column.
#[test]
fn gen_drop_table_suppresses_sub_entity_drops() {
    let mut t = empty_table("users");
    t.columns.push(text_col("name"));
    t.foreign_keys.push(ForeignKey {
        name: "fk".into(), from_column: "name".into(),
        to_table: "other".into(), to_column: "id".into(),
    });
    let ops = generate_diff(&empty_schema(), &schema_with_table(t));
    assert_eq!(ops.len(), 1, "only DropTable expected, got: {:?}", op_names(&ops));
}

/// Enum values reordered produces DropEnum + CreateEnum because PG
/// label ordering is significant.
#[test]
fn gen_enum_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["active".into(), "inactive".into(), "pending".into()],
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["pending".into(), "active".into(), "inactive".into()],
    });
    let ops = generate_diff(&curr, &prev);
    let names = op_names(&ops);
    assert!(names.contains(&"drop_enum"), "reordering should produce DropEnum, got: {:?}", names);
    assert!(names.contains(&"create_enum"), "reordering should produce CreateEnum, got: {:?}", names);
}

/// Appending enum values at the end produces AlterEnum (PG ADD VALUE).
#[test]
fn gen_enum_strict_append() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["a".into(), "b".into()],
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
    });
    let ops = generate_diff(&curr, &prev);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::AlterEnum { .. }));
}

/// Adding values but also reordering existing ones is destructive.
#[test]
fn gen_enum_append_with_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["a".into(), "b".into()],
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["b".into(), "a".into(), "c".into()],
    });
    let ops = generate_diff(&curr, &prev);
    let names = op_names(&ops);
    assert!(names.contains(&"drop_enum"), "reorder+append should be destructive, got: {:?}", names);
}

/// Removing an enum value produces DropEnum + CreateEnum.
#[test]
fn gen_enum_value_removed() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(), schema: None,
        values: vec!["a".into(), "c".into()],
    });
    let ops = generate_diff(&curr, &prev);
    let names = op_names(&ops);
    assert!(names.contains(&"drop_enum"));
    assert!(names.contains(&"create_enum"));
}

/// Type canonicalization: int8 vs bigint produces no diff after canonicalize.
#[test]
fn gen_canonicalized_types_no_diff() {
    let mut t1 = empty_table("t");
    t1.columns.push(Column {
        name: "id".into(), col_type: "int8".into(),
        nullable: false, default: None, primary_key: true, ..Default::default()
    });
    let mut t2 = empty_table("t");
    t2.columns.push(Column {
        name: "id".into(), col_type: "bigint".into(),
        nullable: false, default: None, primary_key: true, ..Default::default()
    });
    let mut s1 = schema_with_table(t1);
    let mut s2 = schema_with_table(t2);
    s1.canonicalize(&Dialect::Postgres);
    s2.canonicalize(&Dialect::Postgres);
    assert!(generate_diff(&s2, &s1).is_empty());
}

// -- inject / decompose ------------------------------------------------------

/// Dropping a function injects DropTrigger for surviving triggers that
/// reference it.
#[test]
fn inject_orphan_trigger_drops() {
    let mut prev = Schema::default();
    let mut t = empty_table("events");
    t.triggers.push(basic_trigger("trg_log", "log_fn"));
    prev.tables.insert("events".into(), t);
    prev.functions.insert("log_fn".into(), basic_function("log_fn"));

    let ops = vec![Operation::DropFunction { function: basic_function("log_fn") }];
    let result = inject_orphan_triggers(ops, &prev);
    let names = op_names(&result);
    assert!(names.contains(&"drop_trigger"), "should inject DropTrigger, got: {:?}", names);
}

/// decompose splits self-referential FK out of CreateTable.
#[test]
fn decompose_breaks_self_ref_fk() {
    let mut t = empty_table("nodes");
    t.columns.push(Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() });
    t.columns.push(int_col("parent_id"));
    t.foreign_keys.push(ForeignKey {
        name: "nodes_parent_fkey".into(), from_column: "parent_id".into(),
        to_table: "nodes".into(), to_column: "id".into(),
    });
    let ops = vec![Operation::CreateTable { table: t }];
    let result = decompose(ops);

    let create = result.iter().find(|op| matches!(op, Operation::CreateTable { .. }));
    let add_fk = result.iter().find(|op| matches!(op, Operation::AddForeignKey { .. }));
    assert!(create.is_some() && add_fk.is_some());
    if let Some(Operation::CreateTable { table }) = create {
        assert!(table.foreign_keys.is_empty(), "FK should be decomposed out");
    }
}

/// decompose splits FKs from mutually-referencing new tables.
#[test]
fn decompose_breaks_mutual_fk_cycle() {
    let mut a = empty_table("a");
    a.columns.push(Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() });
    a.columns.push(int_col("b_id"));
    a.foreign_keys.push(ForeignKey {
        name: "a_b_fkey".into(), from_column: "b_id".into(),
        to_table: "b".into(), to_column: "id".into(),
    });
    let mut b = empty_table("b");
    b.columns.push(Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() });
    b.columns.push(int_col("a_id"));
    b.foreign_keys.push(ForeignKey {
        name: "b_a_fkey".into(), from_column: "a_id".into(),
        to_table: "a".into(), to_column: "id".into(),
    });
    let ops = vec![
        Operation::CreateTable { table: a },
        Operation::CreateTable { table: b },
    ];
    let result = decompose(ops);
    let deferred: Vec<_> = result.iter()
        .filter(|op| matches!(op, Operation::AddForeignKey { .. }))
        .collect();
    assert_eq!(deferred.len(), 2, "both FKs should be decomposed out");
}

// -- sort_operations (Kahn's) ------------------------------------------------

/// CreateEnum is sorted before CreateTable that uses the enum type.
#[test]
fn sort_enum_before_table_using_it() {
    let mut t = empty_table("users");
    t.columns.push(Column {
        name: "status".into(), col_type: "user_status".into(),
        nullable: false, default: None, primary_key: false, ..Default::default()
    });
    let ops = vec![
        Operation::CreateTable { table: t },
        Operation::CreateEnum { enum_def: EnumDef {
            name: "user_status".into(), schema: None,
            values: vec!["active".into(), "inactive".into()],
        }},
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(enum_pos < table_pos, "CreateEnum before CreateTable, got: {:?}", names);
}

/// CreateFunction is sorted before CreateTrigger that references it.
#[test]
fn sort_function_before_trigger() {
    let ops = vec![
        Operation::CreateTrigger {
            table_name: "users".into(),
            trigger: basic_trigger("trg_audit", "audit_fn"),
        },
        Operation::CreateFunction { function: basic_function("audit_fn") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let trg_pos = names.iter().position(|n| *n == "create_trigger").unwrap();
    assert!(fn_pos < trg_pos, "CreateFunction before CreateTrigger, got: {:?}", names);
}

/// DropTrigger is sorted before DropFunction of its function.
#[test]
fn sort_drop_trigger_before_drop_function() {
    let ops = vec![
        Operation::DropFunction { function: basic_function("audit_fn") },
        Operation::DropTrigger {
            table_name: "users".into(),
            trigger: basic_trigger("trg_audit", "audit_fn"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let trg_pos = names.iter().position(|n| *n == "drop_trigger").unwrap();
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    assert!(trg_pos < fn_pos);
}

/// DropForeignKey before DropTable of the referenced table.
#[test]
fn sort_drop_fk_before_drop_table() {
    let ops = vec![
        Operation::DropTable { table: empty_table("orders") },
        Operation::DropForeignKey {
            table_name: "items".into(),
            foreign_key: ForeignKey {
                name: "items_order_fkey".into(), from_column: "order_id".into(),
                to_table: "orders".into(), to_column: "id".into(),
            },
            cascade: false,
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fk_pos = names.iter().position(|n| *n == "drop_foreign_key").unwrap();
    let table_pos = names.iter().position(|n| *n == "drop_table").unwrap();
    assert!(fk_pos < table_pos);
}

/// Intra-table ordering: drops before adds.
#[test]
fn sort_intra_table_drops_before_adds() {
    let ops = vec![
        Operation::AddColumn { table_name: "t".into(), column: text_col("new_col") },
        Operation::DropColumn { table_name: "t".into(), column: text_col("old_col"), cascade: false },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let drop_pos = names.iter().position(|n| *n == "drop_column").unwrap();
    let add_pos = names.iter().position(|n| *n == "add_column").unwrap();
    assert!(drop_pos < add_pos, "DropColumn before AddColumn, got: {:?}", names);
}

/// ReplaceView depends on CreateFunction via Kahn's edges.
#[test]
fn sort_replace_view_after_create_function() {
    let ops = vec![
        Operation::ReplaceView {
            old: ViewDef { name: "v".into(), schema: None, definition: "SELECT 1".into() },
            new: ViewDef { name: "v".into(), schema: None, definition: "SELECT fn1()".into() },
        },
        Operation::CreateFunction { function: basic_function("fn1") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let view_pos = names.iter().position(|n| *n == "replace_view").unwrap();
    assert!(fn_pos < view_pos, "CreateFunction before ReplaceView, got: {:?}", names);
}

/// ReplaceView depends on CreateTable via Kahn's edges.
#[test]
fn sort_replace_view_after_create_table() {
    let ops = vec![
        Operation::ReplaceView {
            old: ViewDef { name: "v".into(), schema: None, definition: "SELECT 1".into() },
            new: ViewDef { name: "v".into(), schema: None, definition: "SELECT * FROM t".into() },
        },
        Operation::CreateTable { table: empty_table("t") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    let view_pos = names.iter().position(|n| *n == "replace_view").unwrap();
    assert!(table_pos < view_pos, "CreateTable before ReplaceView, got: {:?}", names);
}

/// CreateExtension before CreateTable.
#[test]
fn sort_extension_before_table() {
    let ops = vec![
        Operation::CreateTable { table: empty_table("t") },
        Operation::CreateExtension { extension: ExtensionDef { name: "uuid-ossp".into(), schema: None, version: None }},
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let ext_pos = names.iter().position(|n| *n == "create_extension").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(ext_pos < table_pos);
}

/// sort_operations produces deterministic output across many runs.
#[test]
fn sort_deterministic() {
    let make_ops = || vec![
        Operation::CreateTable { table: empty_table("b") },
        Operation::CreateTable { table: empty_table("a") },
        Operation::CreateEnum { enum_def: EnumDef { name: "x".into(), schema: None, values: vec!["v".into()] }},
        Operation::CreateExtension { extension: ExtensionDef { name: "pgcrypto".into(), schema: None, version: None }},
    ];
    let first = sort_operations(make_ops()).unwrap();
    let first_names: Vec<_> = first.iter().map(|op| (op.type_name(), op.entity_name().to_string())).collect();
    for _ in 0..50 {
        let run = sort_operations(make_ops()).unwrap();
        let run_names: Vec<_> = run.iter().map(|op| (op.type_name(), op.entity_name().to_string())).collect();
        assert_eq!(first_names, run_names, "sort must be deterministic");
    }
}

/// DropView before DropTable and DropFunction.
#[test]
fn sort_drop_view_before_drop_table_and_function() {
    let ops = vec![
        Operation::DropTable { table: empty_table("t1") },
        Operation::DropFunction { function: basic_function("fn1") },
        Operation::DropView { view: ViewDef { name: "v1".into(), schema: None, definition: "SELECT 1".into() }},
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let view_pos = names.iter().position(|n| *n == "drop_view").unwrap();
    let table_pos = names.iter().position(|n| *n == "drop_table").unwrap();
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    assert!(view_pos < table_pos, "DropView before DropTable, got: {:?}", names);
    assert!(view_pos < fn_pos, "DropView before DropFunction, got: {:?}", names);
}

/// CreateView after both CreateTable and CreateFunction.
#[test]
fn sort_create_view_after_tables_and_functions() {
    let ops = vec![
        Operation::CreateView { view: ViewDef { name: "v".into(), schema: None, definition: "SELECT 1".into() }},
        Operation::CreateFunction { function: basic_function("fn1") },
        Operation::CreateTable { table: empty_table("t1") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let view_pos = names.iter().position(|n| *n == "create_view").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    assert!(table_pos < view_pos, "CreateTable before CreateView, got: {:?}", names);
    assert!(fn_pos < view_pos, "CreateFunction before CreateView, got: {:?}", names);
}

/// DropTable comes after DropEnum (tables that used the enum must drop first).
#[test]
fn sort_drop_table_before_drop_enum() {
    let ops = vec![
        Operation::DropEnum { enum_def: EnumDef { name: "status".into(), schema: None, values: vec!["a".into()] }},
        Operation::DropTable { table: empty_table("users") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let table_pos = names.iter().position(|n| *n == "drop_table").unwrap();
    let enum_pos = names.iter().position(|n| *n == "drop_enum").unwrap();
    assert!(table_pos < enum_pos, "DropTable before DropEnum, got: {:?}", names);
}

/// DropFunction before DropExtension.
#[test]
fn sort_drop_function_before_drop_extension() {
    let ops = vec![
        Operation::DropExtension { extension: ExtensionDef { name: "pgcrypto".into(), schema: None, version: None }},
        Operation::DropFunction { function: basic_function("fn1") },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    let ext_pos = names.iter().position(|n| *n == "drop_extension").unwrap();
    assert!(fn_pos < ext_pos, "DropFunction before DropExtension, got: {:?}", names);
}

/// Empty operations list sorts without error.
#[test]
fn sort_empty_ops() {
    let sorted = sort_operations(vec![]).unwrap();
    assert!(sorted.is_empty());
}

// -- full pipeline -----------------------------------------------------------

/// Full pipeline: int8 vs bigint with dialect canonicalization is a no-op.
#[test]
fn full_pipeline_canonicalized_noop() {
    let mut t1 = empty_table("t");
    t1.columns.push(Column {
        name: "id".into(), col_type: "int8".into(),
        nullable: false, default: None, primary_key: true, ..Default::default()
    });
    let mut t2 = empty_table("t");
    t2.columns.push(Column {
        name: "id".into(), col_type: "bigint".into(),
        nullable: false, default: None, primary_key: true, ..Default::default()
    });
    let engine = DiffEngine::new();
    let ops = engine.diff(&schema_with_table(t2), &schema_with_table(t1), &Dialect::Postgres).unwrap();
    assert!(ops.is_empty());
}

/// Full pipeline: enum reorder produces DropEnum + CreateEnum.
#[test]
fn full_pipeline_enum_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "s".into(), schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
    });
    let curr = schema_with_enum(EnumDef {
        name: "s".into(), schema: None,
        values: vec!["c".into(), "a".into(), "b".into()],
    });
    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();
    let names = op_names(&ops);
    assert!(names.contains(&"drop_enum"), "reorder should be destructive, got: {:?}", names);
    assert!(names.contains(&"create_enum"));
}

/// Full pipeline: new table with enum-typed column orders CreateEnum before CreateTable.
#[test]
fn full_pipeline_enum_table_ordering() {
    let mut curr = Schema::default();
    curr.enums.insert("role".into(), EnumDef {
        name: "role".into(), schema: None,
        values: vec!["admin".into(), "user".into()],
    });
    let mut t = empty_table("accounts");
    t.columns.push(Column {
        name: "role".into(), col_type: "role".into(),
        nullable: false, default: None, primary_key: false, ..Default::default()
    });
    curr.tables.insert("accounts".into(), t);

    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &empty_schema(), &Dialect::Postgres).unwrap();
    let names = op_names(&ops);
    let enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(enum_pos < table_pos, "CreateEnum before CreateTable, got: {:?}", names);
}

/// Full pipeline: function + trigger + table all created together sorts correctly.
#[test]
fn full_pipeline_function_trigger_table() {
    let mut curr = Schema::default();
    curr.functions.insert("audit_fn".into(), basic_function("audit_fn"));
    let mut t = empty_table("events");
    t.columns.push(text_col("data"));
    t.triggers.push(basic_trigger("trg_audit", "audit_fn"));
    curr.tables.insert("events".into(), t);

    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &empty_schema(), &Dialect::Postgres).unwrap();
    let names = op_names(&ops);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(fn_pos < table_pos, "CreateFunction before CreateTable, got: {:?}", names);
}

/// Full pipeline: dropping function + table that has trigger referencing it.
#[test]
fn full_pipeline_drop_function_with_orphan_trigger() {
    let mut prev = Schema::default();
    prev.functions.insert("log_fn".into(), basic_function("log_fn"));
    let mut t = empty_table("events");
    t.columns.push(text_col("data"));
    t.triggers.push(basic_trigger("trg_log", "log_fn"));
    prev.tables.insert("events".into(), t);

    // Current: table survives but function is gone
    let mut curr = Schema::default();
    let mut t2 = empty_table("events");
    t2.columns.push(text_col("data"));
    curr.tables.insert("events".into(), t2);

    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();
    let names = op_names(&ops);
    assert!(names.contains(&"drop_trigger"), "orphan trigger should be dropped, got: {:?}", names);
    assert!(names.contains(&"drop_function"));
    let trg_pos = names.iter().position(|n| *n == "drop_trigger").unwrap();
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    assert!(trg_pos < fn_pos, "DropTrigger before DropFunction, got: {:?}", names);
}

/// Incompatible enum change with a surviving column must inject temporary
/// casts to text so DropEnum doesn't fail on a type still in use.
#[test]
fn incompatible_enum_change_injects_column_casts() {
    let mut prev = empty_schema();
    prev.enums.insert("status".into(), EnumDef {
        name: "status".into(), schema: None, values: vec!["a".into(), "b".into()],
    });
    prev.tables.insert("orders".into(), Table {
        name: "orders".into(), schema: None,
        columns: vec![
            Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() },
            Column { name: "state".into(), col_type: "status".into(), ..Default::default() },
        ],
        foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![],
    });

    let mut curr = prev.clone();
    curr.enums.insert("status".into(), EnumDef {
        name: "status".into(), schema: None, values: vec!["x".into(), "y".into()],
    });

    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();
    let names = op_names(&ops);

    // Should contain: AlterColumn (status→text), DropEnum, CreateEnum, AlterColumn (text→status)
    let alter_positions: Vec<usize> = names.iter().enumerate()
        .filter(|(_, n)| **n == "alter_column").map(|(i, _)| i).collect();
    let drop_enum_pos = names.iter().position(|n| *n == "drop_enum").unwrap();
    let create_enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();

    assert!(alter_positions.len() >= 2, "need at least 2 AlterColumn ops for cast roundtrip, got: {:?}", names);
    assert!(alter_positions[0] < drop_enum_pos, "pre-cast must come before DropEnum, got: {:?}", names);
    assert!(create_enum_pos < *alter_positions.last().unwrap(), "post-cast must come after CreateEnum, got: {:?}", names);
}

/// Drop ordering must respect FK dependencies: if A→B, drop A before B.
#[test]
fn drop_order_respects_fk_dependencies() {
    let mut prev = empty_schema();
    prev.tables.insert("parents".into(), Table {
        name: "parents".into(), schema: None,
        columns: vec![Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() }],
        foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![],
    });
    prev.tables.insert("children".into(), Table {
        name: "children".into(), schema: None,
        columns: vec![
            Column { name: "id".into(), col_type: "integer".into(), primary_key: true, ..Default::default() },
            Column { name: "parent_id".into(), col_type: "integer".into(), ..Default::default() },
        ],
        foreign_keys: vec![ForeignKey {
            name: "children_parent_fkey".into(),
            from_column: "parent_id".into(),
            to_table: "parents".into(),
            to_column: "id".into(),
        }],
        indexes: vec![], constraints: vec![], triggers: vec![],
    });

    let curr = empty_schema();
    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();

    let drop_positions: Vec<(usize, &str)> = ops.iter().enumerate()
        .filter_map(|(i, op)| match op {
            Operation::DropTable { table } => Some((i, table.name.as_str())),
            _ => None,
        })
        .collect();

    let children_pos = drop_positions.iter().find(|(_, n)| *n == "children").unwrap().0;
    let parents_pos = drop_positions.iter().find(|(_, n)| *n == "parents").unwrap().0;
    assert!(children_pos < parents_pos, "children (dependent) must be dropped before parents, got: {:?}", op_names(&ops));
}

mod proptest_diff {
    use super::*;
    use proptest::prelude::*;

    fn arb_column() -> impl Strategy<Value = Column> {
        let types = prop_oneof![
            Just("integer".to_string()),
            Just("text".to_string()),
            Just("boolean".to_string()),
            Just("bigint".to_string()),
            Just("varchar(255)".to_string()),
        ];
        (
            "[a-z]{3,8}",
            types,
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(Just("0".to_string())),
        )
            .prop_map(|(name, col_type, nullable, primary_key, default)| Column {
                name,
                col_type,
                nullable,
                primary_key,
                default,
                references: None,
                check: None,
            })
    }

    fn arb_table() -> impl Strategy<Value = Table> {
        ("[a-z]{3,8}", proptest::collection::vec(arb_column(), 1..5))
            .prop_map(|(name, columns)| {
                let mut seen = std::collections::HashSet::new();
                let mut columns: Vec<Column> = columns.into_iter()
                    .filter(|c| seen.insert(c.name.clone()))
                    .collect();
                let mut has_pk = false;
                for col in &mut columns {
                    if col.primary_key {
                        if has_pk {
                            col.primary_key = false;
                        }
                        has_pk = true;
                    }
                }
                Table {
                    name: name.clone(),
                    schema: None,
                    columns,
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                }
            })
    }

    fn arb_enum_def() -> impl Strategy<Value = EnumDef> {
        ("[a-z]{3,8}", proptest::collection::vec("[a-z]{2,6}", 1..5))
            .prop_map(|(name, values)| EnumDef {
                name,
                schema: None,
                values,
            })
    }

    fn arb_extension() -> impl Strategy<Value = ExtensionDef> {
        "[a-z]{3,10}".prop_map(|name| ExtensionDef {
            name,
            schema: None,
            version: None,
        })
    }

    fn arb_schema() -> impl Strategy<Value = Schema> {
        (
            proptest::collection::vec(arb_table(), 0..4),
            proptest::collection::vec(arb_enum_def(), 0..3),
            proptest::collection::vec(arb_extension(), 0..2),
        )
            .prop_map(|(tables, enums, extensions)| {
                let mut schema = Schema::default();
                for t in tables {
                    schema.tables.insert(t.name.clone(), t);
                }
                for e in enums {
                    schema.enums.insert(e.name.clone(), e);
                }
                for ext in extensions {
                    schema.extensions.insert(ext.name.clone(), ext);
                }
                schema
            })
    }

    proptest! {
        #[test]
        #[doc = "Diffing a schema against itself must produce zero operations."]
        fn identity_diff_is_empty(schema in arb_schema()) {
            let ops = generate_diff(&schema, &schema);
            prop_assert!(ops.is_empty(), "diff(s, s) should be empty, got {} ops", ops.len());
        }
    }

    proptest! {
        #[test]
        #[doc = "Applying the diff to the previous schema must reconstruct the current schema."]
        fn apply_roundtrip(current in arb_schema(), previous in arb_schema()) {
            let raw_ops = generate_diff(&current, &previous);
            let ops = inject_orphan_triggers(raw_ops, &previous);
            let ops = inject_enum_column_casts(ops, &previous);
            let ops = decompose(ops);
            let sorted = sort_operations(ops);
            prop_assert!(sorted.is_ok(), "sort_operations failed: {:?}", sorted.err());
            let sorted = sorted.unwrap();

            let mut rebuilt = previous.clone();
            for op in &sorted {
                let result = rebuilt.apply(op);
                prop_assert!(result.is_ok(), "apply failed on {:?}: {:?}", op, result.err());
            }
            prop_assert_eq!(rebuilt, current);
        }
    }

    proptest! {
        #[test]
        #[doc = "Diff output must be deterministic across multiple invocations."]
        fn deterministic_diff(current in arb_schema(), previous in arb_schema()) {
            let ops1 = generate_diff(&current, &previous);
            let ops2 = generate_diff(&current, &previous);
            prop_assert_eq!(ops1, ops2, "diff must be deterministic");
        }
    }
}
