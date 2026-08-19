use super::*;
use crate::dialects::Dialect;
use crate::states::{
    Column, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Table, TriggerDef,
    TriggerEvent, TriggerScope, TriggerTiming, ViewDef, Volatility,
};

fn empty_schema() -> Schema {
    Schema::default()
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
        options: Default::default(),
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

#[test]
fn diff_rejects_primary_key_mutation() {
    let mut previous = empty_schema();
    let mut current = empty_schema();
    let mut old_table = empty_table("order_lines");
    old_table.columns.push(Column {
        name: "order_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        primary_key: true,
        ..Default::default()
    });
    old_table.primary_key = Some(PrimaryKey {
        name: "order_lines_pkey".to_string(),
        columns: vec!["order_id".to_string()],
    });
    let mut new_table = old_table.clone();
    new_table.columns.push(Column {
        name: "tenant_id".to_string(),
        col_type: "bigint".to_string(),
        nullable: false,
        primary_key: true,
        ..Default::default()
    });
    new_table.primary_key = Some(PrimaryKey {
        name: "order_lines_pkey".to_string(),
        columns: vec!["order_id".to_string(), "tenant_id".to_string()],
    });
    previous.tables.insert("order_lines".to_string(), old_table);
    current.tables.insert("order_lines".to_string(), new_table);

    let err = DiffEngine::new()
        .diff(&current, &previous, &Dialect::Postgres)
        .unwrap_err();

    assert!(matches!(err, DiffError::PrimaryKeyMutation(table) if table == "order_lines"));
}

fn int_col(name: &str) -> Column {
    Column {
        name: name.to_string(),
        col_type: "integer".to_string(),
        nullable: false,
        default: None,
        primary_key: false,
        ..Default::default()
    }
}

fn basic_function(name: &str) -> FunctionDef {
    FunctionDef {
        name: name.to_string(),
        schema: None,
        arguments: String::new(),
        parameters: Vec::new(),
        depends_on: Vec::new(),
        returns: "void".to_string(),
        language: "sql".to_string(),
        body: "SELECT 1".to_string(),
        volatility: Volatility::Volatile,
        security_definer: false,
        opaque: Default::default(),
    }
}

fn basic_trigger(name: &str, fn_name: &str) -> TriggerDef {
    TriggerDef {
        name: Some(name.to_string()),
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: Some(fn_name.to_string()),
        when: None,
        query: None,
        language: None,
        opaque: Default::default(),
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

fn schema_with_function(f: FunctionDef) -> Schema {
    let mut s = Schema::default();
    s.functions.insert(f.name.clone(), f);
    s
}

fn schema_with_view(v: ViewDef) -> Schema {
    let mut s = Schema::default();
    s.views.insert(v.name.clone(), v);
    s
}

fn op_names(ops: &[Operation]) -> Vec<&'static str> {
    ops.iter().map(|op| op.type_name()).collect()
}

#[test]
fn opaque_source_comparison_ignores_whitespace_and_comments() {
    assert!(crate::opaque::opaque_sources_equal(
        "SELECT  a -- ignored\nFROM users /* ignored */ WHERE id = 1",
        "SELECT a FROM users WHERE id = 1"
    ));
}

#[test]
fn opaque_source_comparison_preserves_quoted_string_contents() {
    assert!(!crate::opaque::opaque_sources_equal(
        "SELECT 'a b'",
        "SELECT 'ab'"
    ));
}

#[test]
fn opaque_source_comparison_preserves_dollar_quoted_contents() {
    assert!(!crate::opaque::opaque_sources_equal(
        "SELECT $$a b$$",
        "SELECT $$ab$$"
    ));
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

/// Verifies default-expression keyword case does not produce a schema migration.
#[test]
fn default_expression_case_only_change_is_noop() {
    let mut table = empty_table("users");
    table.columns.push(text_col("created_at"));
    let mut previous = schema_with_table(table);
    let mut current = previous.clone();
    previous.tables.get_mut("users").unwrap().columns[0].default = Some("NOW()".to_string());
    current.tables.get_mut("users").unwrap().columns[0].default = Some("now()".to_string());

    let operations = DiffEngine::new()
        .diff(&current, &previous, &Dialect::Postgres)
        .unwrap();

    assert!(
        operations.is_empty(),
        "unexpected operations: {operations:?}"
    );
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
    assert!(
        matches!(&ops[0], Operation::AddColumn { table_name, column }
        if table_name == "users" && column.name == "email")
    );
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
        name: "idx_email".into(),
        columns: vec!["email".into()],
        unique: true,
        predicate: None,
        opaque: Default::default(),
    });
    let ops = generate_diff(&schema_with_table(t), &empty_schema());
    assert_eq!(
        ops.len(),
        1,
        "only CreateTable expected, got: {:?}",
        op_names(&ops)
    );
    assert!(matches!(&ops[0], Operation::CreateTable { .. }));
}

/// Dropping a table with columns produces only DropTable, not
/// DropTable + DropColumn for each column.
#[test]
fn gen_drop_table_suppresses_sub_entity_drops() {
    let mut t = empty_table("users");
    t.columns.push(text_col("name"));
    t.foreign_keys
        .push(ForeignKey::single("fk", "name", "other", "id"));
    let ops = generate_diff(&empty_schema(), &schema_with_table(t));
    assert_eq!(
        ops.len(),
        1,
        "only DropTable expected, got: {:?}",
        op_names(&ops)
    );
}

/// Enum values reordered produces DropEnum + CreateEnum because PG
/// label ordering is significant.
#[test]
fn gen_enum_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["active".into(), "inactive".into(), "pending".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["pending".into(), "active".into(), "inactive".into()],
        opaque: Default::default(),
    });
    let ops = generate_diff(&curr, &prev);
    let names = op_names(&ops);
    assert!(
        names.contains(&"drop_enum"),
        "reordering should produce DropEnum, got: {:?}",
        names
    );
    assert!(
        names.contains(&"create_enum"),
        "reordering should produce CreateEnum, got: {:?}",
        names
    );
}

/// Appending enum values at the end produces AlterEnum (PG ADD VALUE).
#[test]
fn gen_enum_strict_append() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "b".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
        opaque: Default::default(),
    });
    let ops = generate_diff(&curr, &prev);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::AlterEnum { .. }));
}

/// Adding enum values between existing labels preserves old label order, so PostgreSQL can
/// render it losslessly with ADD VALUE BEFORE/AFTER.
#[test]
fn gen_enum_insert_without_reorder_is_alter() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "c".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        opaque: Default::default(),
    });
    let ops = generate_diff(&curr, &prev);
    assert_eq!(ops.len(), 1);
    assert!(matches!(&ops[0], Operation::AlterEnum { .. }));
}

/// Adding values but also reordering existing ones is destructive.
#[test]
fn gen_enum_append_with_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "b".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["b".into(), "a".into(), "c".into()],
        opaque: Default::default(),
    });
    let ops = generate_diff(&curr, &prev);
    let names = op_names(&ops);
    assert!(
        names.contains(&"drop_enum"),
        "reorder+append should be destructive, got: {:?}",
        names
    );
}

/// Removing an enum value produces DropEnum + CreateEnum.
#[test]
fn gen_enum_value_removed() {
    let prev = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "status".into(),
        schema: None,
        values: vec!["a".into(), "c".into()],
        opaque: Default::default(),
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
        name: "id".into(),
        col_type: "int8".into(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    });
    let mut t2 = empty_table("t");
    t2.columns.push(Column {
        name: "id".into(),
        col_type: "bigint".into(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
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
    prev.functions
        .insert("log_fn".into(), basic_function("log_fn"));

    let ops = vec![Operation::DropFunction {
        function: basic_function("log_fn"),
    }];
    let result = inject_orphan_triggers(ops, &prev);
    let names = op_names(&result);
    assert!(
        names.contains(&"drop_trigger"),
        "should inject DropTrigger, got: {:?}",
        names
    );
}

/// decompose splits self-referential FK out of CreateTable.
#[test]
fn decompose_breaks_self_ref_fk() {
    let mut t = empty_table("nodes");
    t.columns.push(Column {
        name: "id".into(),
        col_type: "integer".into(),
        primary_key: true,
        ..Default::default()
    });
    t.columns.push(int_col("parent_id"));
    t.foreign_keys.push(ForeignKey::single(
        "nodes_parent_fkey",
        "parent_id",
        "nodes",
        "id",
    ));
    let ops = vec![Operation::CreateTable { table: t }];
    let result = decompose(ops);

    let create = result
        .iter()
        .find(|op| matches!(op, Operation::CreateTable { .. }));
    let add_fk = result
        .iter()
        .find(|op| matches!(op, Operation::AddForeignKey { .. }));
    assert!(create.is_some() && add_fk.is_some());
    if let Some(Operation::CreateTable { table }) = create {
        assert!(table.foreign_keys.is_empty(), "FK should be decomposed out");
    }
}

/// decompose splits FKs from mutually-referencing new tables.
#[test]
fn decompose_breaks_mutual_fk_cycle() {
    let mut a = empty_table("a");
    a.columns.push(Column {
        name: "id".into(),
        col_type: "integer".into(),
        primary_key: true,
        ..Default::default()
    });
    a.columns.push(int_col("b_id"));
    a.foreign_keys
        .push(ForeignKey::single("a_b_fkey", "b_id", "b", "id"));
    let mut b = empty_table("b");
    b.columns.push(Column {
        name: "id".into(),
        col_type: "integer".into(),
        primary_key: true,
        ..Default::default()
    });
    b.columns.push(int_col("a_id"));
    b.foreign_keys
        .push(ForeignKey::single("b_a_fkey", "a_id", "a", "id"));
    let ops = vec![
        Operation::CreateTable { table: a },
        Operation::CreateTable { table: b },
    ];
    let result = decompose(ops);
    let deferred: Vec<_> = result
        .iter()
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
        name: "status".into(),
        col_type: "user_status".into(),
        nullable: false,
        default: None,
        primary_key: false,
        ..Default::default()
    });
    let ops = vec![
        Operation::CreateTable { table: t },
        Operation::CreateEnum {
            enum_def: EnumDef {
                name: "user_status".into(),
                schema: None,
                values: vec!["active".into(), "inactive".into()],
                opaque: Default::default(),
            },
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(
        enum_pos < table_pos,
        "CreateEnum before CreateTable, got: {:?}",
        names
    );
}

/// CreateFunction is sorted before CreateTrigger that references it.
#[test]
fn sort_function_before_trigger() {
    let ops = vec![
        Operation::CreateTrigger {
            table_name: "users".into(),
            trigger: basic_trigger("trg_audit", "audit_fn"),
        },
        Operation::CreateFunction {
            function: basic_function("audit_fn"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let trg_pos = names.iter().position(|n| *n == "create_trigger").unwrap();
    assert!(
        fn_pos < trg_pos,
        "CreateFunction before CreateTrigger, got: {:?}",
        names
    );
}

/// SQL-language function calls order helper creation before dependent creates and alters.
#[test]
fn sort_sql_function_body_dependencies_before_dependent_alter() {
    let previous_daily_report = basic_function("dynrs_daily_report");
    let mut daily_report = previous_daily_report.clone();
    daily_report.body =
        "SELECT dynrs_report_provider_daily(); SELECT dynrs_report_aggregate();".to_string();
    daily_report.depends_on = vec![
        crate::EntityDependency::parse("function::dynrs_report_provider_earnings").unwrap(),
        crate::EntityDependency::parse("function::dynrs_report_aggregate").unwrap(),
    ];

    let mut provider_daily = basic_function("dynrs_report_provider_daily");
    provider_daily.body = "SELECT dynrs_report_provider_sessions();".to_string();
    provider_daily.depends_on = vec![
        crate::EntityDependency::parse("function::dynrs_report_provider_sessions").unwrap(),
    ];
    let mut provider_earnings = basic_function("dynrs_report_provider_earnings");
    provider_earnings.body = "SELECT dynrs_report_provider_daily();".to_string();
    provider_earnings.depends_on = vec![
        crate::EntityDependency::parse("function::dynrs_report_provider_daily").unwrap(),
    ];
    let mut report_aggregate = basic_function("dynrs_report_aggregate");
    report_aggregate.body = "SELECT dynrs_report_provider_daily();".to_string();
    report_aggregate.depends_on = vec![
        crate::EntityDependency::parse("function::dynrs_report_provider_daily").unwrap(),
    ];

    let sorted = sort_operations(vec![
        Operation::AlterFunction {
            old: previous_daily_report,
            new: daily_report,
        },
        Operation::CreateFunction {
            function: report_aggregate,
        },
        Operation::CreateFunction {
            function: provider_daily,
        },
        Operation::CreateFunction {
            function: provider_earnings,
        },
        Operation::CreateFunction {
            function: basic_function("dynrs_report_provider_sessions"),
        },
    ])
    .unwrap();
    let names = sorted
        .iter()
        .map(|operation| operation.entity_name().to_string())
        .collect::<Vec<_>>();
    let position = |name| names.iter().position(|candidate| candidate == name).unwrap();

    assert!(position("dynrs_report_provider_sessions") < position("dynrs_report_provider_daily"));
    assert!(position("dynrs_report_provider_daily") < position("dynrs_report_provider_earnings"));
    assert!(position("dynrs_report_provider_daily") < position("dynrs_report_aggregate"));
    assert!(position("dynrs_report_provider_earnings") < position("dynrs_daily_report"));
    assert!(position("dynrs_report_aggregate") < position("dynrs_daily_report"));
}

/// DropTrigger is sorted before DropFunction of its function.
#[test]
fn sort_drop_trigger_before_drop_function() {
    let ops = vec![
        Operation::DropFunction {
            function: basic_function("audit_fn"),
        },
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
        Operation::DropTable {
            table: empty_table("orders"),
        },
        Operation::DropForeignKey {
            table_name: "items".into(),
            foreign_key: ForeignKey::single("items_order_fkey", "order_id", "orders", "id"),
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
        Operation::AddColumn {
            table_name: "t".into(),
            column: text_col("new_col"),
        },
        Operation::DropColumn {
            table_name: "t".into(),
            column: text_col("old_col"),
            cascade: false,
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let drop_pos = names.iter().position(|n| *n == "drop_column").unwrap();
    let add_pos = names.iter().position(|n| *n == "add_column").unwrap();
    assert!(
        drop_pos < add_pos,
        "DropColumn before AddColumn, got: {:?}",
        names
    );
}

/// ReplaceView depends on CreateFunction via Kahn's edges.
#[test]
fn sort_replace_view_after_create_function() {
    let ops = vec![
        Operation::ReplaceView {
            old: ViewDef {
                name: "v".into(),
                schema: None,
                definition: "SELECT 1".into(),
                opaque: Default::default(),
            },
            new: ViewDef {
                name: "v".into(),
                schema: None,
                definition: "SELECT fn1()".into(),
                opaque: Default::default(),
            },
        },
        Operation::CreateFunction {
            function: basic_function("fn1"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let view_pos = names.iter().position(|n| *n == "replace_view").unwrap();
    assert!(
        fn_pos < view_pos,
        "CreateFunction before ReplaceView, got: {:?}",
        names
    );
}

/// ReplaceView depends on CreateTable via Kahn's edges.
#[test]
fn sort_replace_view_after_create_table() {
    let ops = vec![
        Operation::ReplaceView {
            old: ViewDef {
                name: "v".into(),
                schema: None,
                definition: "SELECT 1".into(),
                opaque: Default::default(),
            },
            new: ViewDef {
                name: "v".into(),
                schema: None,
                definition: "SELECT * FROM t".into(),
                opaque: Default::default(),
            },
        },
        Operation::CreateTable {
            table: empty_table("t"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    let view_pos = names.iter().position(|n| *n == "replace_view").unwrap();
    assert!(
        table_pos < view_pos,
        "CreateTable before ReplaceView, got: {:?}",
        names
    );
}

/// CreateExtension before CreateTable.
#[test]
fn sort_extension_before_table() {
    let ops = vec![
        Operation::CreateTable {
            table: empty_table("t"),
        },
        Operation::CreateExtension {
            extension: ExtensionDef {
                name: "uuid-ossp".into(),
                schema: None,
                version: None,
                opaque: Default::default(),
            },
        },
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
    let make_ops = || {
        vec![
            Operation::CreateTable {
                table: empty_table("b"),
            },
            Operation::CreateTable {
                table: empty_table("a"),
            },
            Operation::CreateEnum {
                enum_def: EnumDef {
                    name: "x".into(),
                    schema: None,
                    values: vec!["v".into()],
                    opaque: Default::default(),
                },
            },
            Operation::CreateExtension {
                extension: ExtensionDef {
                    name: "pgcrypto".into(),
                    schema: None,
                    version: None,
                    opaque: Default::default(),
                },
            },
        ]
    };
    let first = sort_operations(make_ops()).unwrap();
    let first_names: Vec<_> = first
        .iter()
        .map(|op| (op.type_name(), op.entity_name().to_string()))
        .collect();
    for _ in 0..50 {
        let run = sort_operations(make_ops()).unwrap();
        let run_names: Vec<_> = run
            .iter()
            .map(|op| (op.type_name(), op.entity_name().to_string()))
            .collect();
        assert_eq!(first_names, run_names, "sort must be deterministic");
    }
}

/// DropView before DropTable and DropFunction.
#[test]
fn sort_drop_view_before_drop_table_and_function() {
    let ops = vec![
        Operation::DropTable {
            table: empty_table("t1"),
        },
        Operation::DropFunction {
            function: basic_function("fn1"),
        },
        Operation::DropView {
            view: ViewDef {
                name: "v1".into(),
                schema: None,
                definition: "SELECT 1".into(),
                opaque: Default::default(),
            },
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let view_pos = names.iter().position(|n| *n == "drop_view").unwrap();
    let table_pos = names.iter().position(|n| *n == "drop_table").unwrap();
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    assert!(
        view_pos < table_pos,
        "DropView before DropTable, got: {:?}",
        names
    );
    assert!(
        view_pos < fn_pos,
        "DropView before DropFunction, got: {:?}",
        names
    );
}

/// CreateView after both CreateTable and CreateFunction.
#[test]
fn sort_create_view_after_tables_and_functions() {
    let ops = vec![
        Operation::CreateView {
            view: ViewDef {
                name: "v".into(),
                schema: None,
                definition: "SELECT 1".into(),
                opaque: Default::default(),
            },
        },
        Operation::CreateFunction {
            function: basic_function("fn1"),
        },
        Operation::CreateTable {
            table: empty_table("t1"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let view_pos = names.iter().position(|n| *n == "create_view").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    assert!(
        table_pos < view_pos,
        "CreateTable before CreateView, got: {:?}",
        names
    );
    assert!(
        fn_pos < view_pos,
        "CreateFunction before CreateView, got: {:?}",
        names
    );
}

/// DropTable comes after DropEnum (tables that used the enum must drop first).
#[test]
fn sort_drop_table_before_drop_enum() {
    let ops = vec![
        Operation::DropEnum {
            enum_def: EnumDef {
                name: "status".into(),
                schema: None,
                values: vec!["a".into()],
                opaque: Default::default(),
            },
        },
        Operation::DropTable {
            table: empty_table("users"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let table_pos = names.iter().position(|n| *n == "drop_table").unwrap();
    let enum_pos = names.iter().position(|n| *n == "drop_enum").unwrap();
    assert!(
        table_pos < enum_pos,
        "DropTable before DropEnum, got: {:?}",
        names
    );
}

/// DropFunction before DropExtension.
#[test]
fn sort_drop_function_before_drop_extension() {
    let ops = vec![
        Operation::DropExtension {
            extension: ExtensionDef {
                name: "pgcrypto".into(),
                schema: None,
                version: None,
                opaque: Default::default(),
            },
        },
        Operation::DropFunction {
            function: basic_function("fn1"),
        },
    ];
    let sorted = sort_operations(ops).unwrap();
    let names = op_names(&sorted);
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    let ext_pos = names.iter().position(|n| *n == "drop_extension").unwrap();
    assert!(
        fn_pos < ext_pos,
        "DropFunction before DropExtension, got: {:?}",
        names
    );
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
        name: "id".into(),
        col_type: "int8".into(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    });
    let mut t2 = empty_table("t");
    t2.columns.push(Column {
        name: "id".into(),
        col_type: "bigint".into(),
        nullable: false,
        default: None,
        primary_key: true,
        ..Default::default()
    });
    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_table(t2),
            &schema_with_table(t1),
            &Dialect::Postgres,
        )
        .unwrap();
    assert!(ops.is_empty());
}

/// Full pipeline: enum reorder produces DropEnum + CreateEnum.
#[test]
fn full_pipeline_enum_reorder_is_destructive() {
    let prev = schema_with_enum(EnumDef {
        name: "s".into(),
        schema: None,
        values: vec!["a".into(), "b".into(), "c".into()],
        opaque: Default::default(),
    });
    let curr = schema_with_enum(EnumDef {
        name: "s".into(),
        schema: None,
        values: vec!["c".into(), "a".into(), "b".into()],
        opaque: Default::default(),
    });
    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();
    let names = op_names(&ops);
    assert!(
        names.contains(&"drop_enum"),
        "reorder should be destructive, got: {:?}",
        names
    );
    assert!(names.contains(&"create_enum"));
}

/// Full pipeline: new table with enum-typed column orders CreateEnum before CreateTable.
#[test]
fn full_pipeline_enum_table_ordering() {
    let mut curr = Schema::default();
    curr.enums.insert(
        "role".into(),
        EnumDef {
            name: "role".into(),
            schema: None,
            values: vec!["admin".into(), "user".into()],
            opaque: Default::default(),
        },
    );
    let mut t = empty_table("accounts");
    t.columns.push(Column {
        name: "role".into(),
        col_type: "role".into(),
        nullable: false,
        default: None,
        primary_key: false,
        ..Default::default()
    });
    curr.tables.insert("accounts".into(), t);

    let engine = DiffEngine::new();
    let ops = engine
        .diff(&curr, &empty_schema(), &Dialect::Postgres)
        .unwrap();
    let names = op_names(&ops);
    let enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(
        enum_pos < table_pos,
        "CreateEnum before CreateTable, got: {:?}",
        names
    );
}

/// Full pipeline: function + trigger + table all created together sorts correctly.
#[test]
fn full_pipeline_function_trigger_table() {
    let mut curr = Schema::default();
    curr.functions
        .insert("audit_fn".into(), basic_function("audit_fn"));
    let mut t = empty_table("events");
    t.columns.push(text_col("data"));
    t.triggers.push(basic_trigger("trg_audit", "audit_fn"));
    curr.tables.insert("events".into(), t);

    let engine = DiffEngine::new();
    let ops = engine
        .diff(&curr, &empty_schema(), &Dialect::Postgres)
        .unwrap();
    let names = op_names(&ops);
    let fn_pos = names.iter().position(|n| *n == "create_function").unwrap();
    let table_pos = names.iter().position(|n| *n == "create_table").unwrap();
    assert!(
        fn_pos < table_pos,
        "CreateFunction before CreateTable, got: {:?}",
        names
    );
}

#[test]
fn full_pipeline_function_formatting_only_change_is_noop() {
    let mut prev_fn = basic_function("audit_fn");
    prev_fn.body = "SELECT  a -- ignored\nFROM users".to_string();
    let mut curr_fn = prev_fn.clone();
    curr_fn.body = "SELECT a FROM users".to_string();

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_function(curr_fn),
            &schema_with_function(prev_fn),
            &Dialect::Postgres,
        )
        .unwrap();

    assert!(ops.is_empty(), "expected no drift, got: {ops:?}");
}

#[test]
fn full_pipeline_function_real_body_change_alters_function() {
    let mut prev_fn = basic_function("audit_fn");
    prev_fn.body = "SELECT 'a b'".to_string();
    let mut curr_fn = prev_fn.clone();
    curr_fn.body = "SELECT 'ab'".to_string();

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_function(curr_fn),
            &schema_with_function(prev_fn),
            &Dialect::Postgres,
        )
        .unwrap();

    assert_eq!(op_names(&ops), vec!["alter_function"]);
}

#[test]
fn full_pipeline_view_formatting_only_change_is_noop() {
    let prev = ViewDef {
        name: "active_users".to_string(),
        schema: None,
        definition: "SELECT  id -- ignored\nFROM users".to_string(),
        opaque: Default::default(),
    };
    let curr = ViewDef {
        definition: "SELECT id FROM users".to_string(),
        opaque: Default::default(),
        ..prev.clone()
    };

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_view(curr),
            &schema_with_view(prev),
            &Dialect::Postgres,
        )
        .unwrap();

    assert!(ops.is_empty(), "expected no drift, got: {ops:?}");
}

#[test]
fn full_pipeline_view_real_definition_change_replaces_view() {
    let prev = ViewDef {
        name: "active_users".to_string(),
        schema: None,
        definition: "SELECT 'a b'".to_string(),
        opaque: Default::default(),
    };
    let curr = ViewDef {
        definition: "SELECT 'ab'".to_string(),
        opaque: Default::default(),
        ..prev.clone()
    };

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_view(curr),
            &schema_with_view(prev),
            &Dialect::Postgres,
        )
        .unwrap();

    assert_eq!(op_names(&ops), vec!["replace_view"]);
}

#[test]
fn full_pipeline_trigger_formatting_only_query_change_is_noop() {
    let mut prev_table = empty_table("events");
    let mut prev_trigger = basic_trigger("trg_audit", "audit_fn");
    prev_trigger.function_name = None;
    prev_trigger.query = Some("SELECT  a -- ignored\nFROM users".to_string());
    prev_table.triggers.push(prev_trigger);

    let mut curr_table = empty_table("events");
    let mut curr_trigger = basic_trigger("trg_audit", "audit_fn");
    curr_trigger.function_name = None;
    curr_trigger.query = Some("SELECT a FROM users".to_string());
    curr_table.triggers.push(curr_trigger);

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_table(curr_table),
            &schema_with_table(prev_table),
            &Dialect::Postgres,
        )
        .unwrap();

    assert!(ops.is_empty(), "expected no drift, got: {ops:?}");
}

#[test]
fn full_pipeline_trigger_real_query_change_alters_trigger() {
    let mut prev_table = empty_table("events");
    let mut prev_trigger = basic_trigger("trg_audit", "audit_fn");
    prev_trigger.function_name = None;
    prev_trigger.query = Some("SELECT 'a b'".to_string());
    prev_table.triggers.push(prev_trigger);

    let mut curr_table = empty_table("events");
    let mut curr_trigger = basic_trigger("trg_audit", "audit_fn");
    curr_trigger.function_name = None;
    curr_trigger.query = Some("SELECT 'ab'".to_string());
    curr_table.triggers.push(curr_trigger);

    let engine = DiffEngine::new();
    let ops = engine
        .diff(
            &schema_with_table(curr_table),
            &schema_with_table(prev_table),
            &Dialect::Postgres,
        )
        .unwrap();

    assert_eq!(op_names(&ops), vec!["alter_trigger"]);
}

/// Full pipeline: dropping function + table that has trigger referencing it.
#[test]
fn full_pipeline_drop_function_with_orphan_trigger() {
    let mut prev = Schema::default();
    prev.functions
        .insert("log_fn".into(), basic_function("log_fn"));
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
    assert!(
        names.contains(&"drop_trigger"),
        "orphan trigger should be dropped, got: {:?}",
        names
    );
    assert!(names.contains(&"drop_function"));
    let trg_pos = names.iter().position(|n| *n == "drop_trigger").unwrap();
    let fn_pos = names.iter().position(|n| *n == "drop_function").unwrap();
    assert!(
        trg_pos < fn_pos,
        "DropTrigger before DropFunction, got: {:?}",
        names
    );
}

/// Incompatible enum change with a surviving column must inject temporary
/// casts to text so DropEnum doesn't fail on a type still in use.
#[test]
fn incompatible_enum_change_injects_column_casts() {
    let mut prev = empty_schema();
    prev.enums.insert(
        "status".into(),
        EnumDef {
            name: "status".into(),
            schema: None,
            values: vec!["a".into(), "b".into()],
            opaque: Default::default(),
        },
    );
    prev.tables.insert(
        "orders".into(),
        Table {
            name: "orders".into(),
            schema: None,
            primary_key: None,
            columns: vec![
                Column {
                    name: "id".into(),
                    col_type: "integer".into(),
                    primary_key: true,
                    ..Default::default()
                },
                Column {
                    name: "state".into(),
                    col_type: "status".into(),
                    ..Default::default()
                },
            ],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
            options: Default::default(),
        },
    );

    let mut curr = prev.clone();
    curr.enums.insert(
        "status".into(),
        EnumDef {
            name: "status".into(),
            schema: None,
            values: vec!["x".into(), "y".into()],
            opaque: Default::default(),
        },
    );

    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();
    let names = op_names(&ops);

    // Should contain: AlterColumn (status→text), DropEnum, CreateEnum, AlterColumn (text→status)
    let alter_positions: Vec<usize> = names
        .iter()
        .enumerate()
        .filter(|(_, n)| **n == "alter_column")
        .map(|(i, _)| i)
        .collect();
    let drop_enum_pos = names.iter().position(|n| *n == "drop_enum").unwrap();
    let create_enum_pos = names.iter().position(|n| *n == "create_enum").unwrap();

    assert!(
        alter_positions.len() >= 2,
        "need at least 2 AlterColumn ops for cast roundtrip, got: {:?}",
        names
    );
    assert!(
        alter_positions[0] < drop_enum_pos,
        "pre-cast must come before DropEnum, got: {:?}",
        names
    );
    assert!(
        create_enum_pos < *alter_positions.last().unwrap(),
        "post-cast must come after CreateEnum, got: {:?}",
        names
    );
}

/// Drop ordering must respect FK dependencies: if A→B, drop A before B.
#[test]
fn drop_order_respects_fk_dependencies() {
    let mut prev = empty_schema();
    prev.tables.insert(
        "parents".into(),
        Table {
            name: "parents".into(),
            schema: None,
            primary_key: None,
            columns: vec![Column {
                name: "id".into(),
                col_type: "integer".into(),
                primary_key: true,
                ..Default::default()
            }],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
            options: Default::default(),
        },
    );
    prev.tables.insert(
        "children".into(),
        Table {
            name: "children".into(),
            schema: None,
            primary_key: None,
            columns: vec![
                Column {
                    name: "id".into(),
                    col_type: "integer".into(),
                    primary_key: true,
                    ..Default::default()
                },
                Column {
                    name: "parent_id".into(),
                    col_type: "integer".into(),
                    ..Default::default()
                },
            ],
            foreign_keys: vec![ForeignKey::single(
                "children_parent_fkey",
                "parent_id",
                "parents",
                "id",
            )],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
            options: Default::default(),
        },
    );

    let curr = empty_schema();
    let engine = DiffEngine::new();
    let ops = engine.diff(&curr, &prev, &Dialect::Postgres).unwrap();

    let drop_positions: Vec<(usize, &str)> = ops
        .iter()
        .enumerate()
        .filter_map(|(i, op)| match op {
            Operation::DropTable { table } => Some((i, table.name.as_str())),
            _ => None,
        })
        .collect();

    let children_pos = drop_positions
        .iter()
        .find(|(_, n)| *n == "children")
        .unwrap()
        .0;
    let parents_pos = drop_positions
        .iter()
        .find(|(_, n)| *n == "parents")
        .unwrap()
        .0;
    assert!(
        children_pos < parents_pos,
        "children (dependent) must be dropped before parents, got: {:?}",
        op_names(&ops)
    );
}

mod property_diff {
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
            Just("varchar(255)".to_string())
        ];
        (arb_column_name(), types, any::<bool>()).prop_map(|(name, col_type, nullable)| Column {
            name,
            col_type,
            nullable,
            primary_key: false,
            default: None,
            references: None,
            check: None,
            generated: None,
            generated_storage: None,
            dialect_options: Default::default(),
        })
    }

    fn arb_table() -> impl Strategy<Value = Table> {
        (
            arb_identifier(),
            proptest::collection::vec(arb_column(), 1..5),
        )
            .prop_map(|(name, columns)| {
                let mut seen = BTreeSet::new();
                let mut columns: Vec<Column> = columns
                    .into_iter()
                    .filter(|column| seen.insert(column.name.clone()))
                    .collect();
                if columns.is_empty() {
                    columns.push(Column {
                        name: "id".to_string(),
                        col_type: "integer".to_string(),
                        nullable: false,
                        primary_key: false,
                        ..Default::default()
                    });
                }
                Table {
                    name,
                    schema: None,
                    primary_key: None,
                    columns,
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                    options: Default::default(),
                }
            })
    }

    fn arb_schema() -> impl Strategy<Value = Schema> {
        proptest::collection::vec(arb_table(), 0..4).prop_map(|tables| {
            let mut schema = Schema::default();
            for table in tables {
                schema.tables.insert(table.name.clone(), table);
            }
            schema.normalize();
            schema
        })
    }

    fn add_generated_table(mut schema: Schema, suffix: u16) -> Schema {
        let mut name = format!("generated_{suffix}");
        while schema.tables.contains_key(&name) {
            name.push_str("_next");
        }
        schema.tables.insert(
            name.clone(),
            Table {
                name,
                schema: None,
                primary_key: None,
                columns: vec![Column {
                    name: "id".to_string(),
                    col_type: "integer".to_string(),
                    nullable: false,
                    primary_key: true,
                    ..Default::default()
                }],
                foreign_keys: vec![],
                indexes: vec![],
                constraints: vec![],
                triggers: vec![],
                options: Default::default(),
            },
        );
        schema.normalize();
        schema
    }

    fn apply_ops(mut schema: Schema, ops: &[Operation]) -> Result<Schema, String> {
        for operation in ops {
            schema
                .apply(operation)
                .map_err(|error| format!("failed to apply {operation:?}: {error}"))?;
        }
        Ok(schema)
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        #[test]
        #[doc = "Diffing a normalized schema against itself produces no operations."]
        fn identity_diff_is_empty(schema in arb_schema()) {
            let ops = DiffEngine::new()
                .diff(&schema, &schema, &Dialect::Postgres)
                .expect("identity diff should not fail");
            prop_assert!(ops.is_empty(), "diff(schema, schema) should be empty, got {ops:?}");
        }

        #[test]
        #[doc = "Diff output is deterministic for the same normalized input pair."]
        fn diff_output_is_deterministic(previous in arb_schema(), suffix in any::<u16>()) {
            let current = add_generated_table(previous.clone(), suffix);
            let first = DiffEngine::new()
                .diff(&current, &previous, &Dialect::Postgres)
                .expect("diff should succeed");
            let second = DiffEngine::new()
                .diff(&current, &previous, &Dialect::Postgres)
                .expect("diff should succeed");
            prop_assert_eq!(first, second);
        }

        #[test]
        #[doc = "Applying a generated add-table diff reconstructs the target schema."]
        fn add_table_diff_replays_to_target(previous in arb_schema(), suffix in any::<u16>()) {
            let current = add_generated_table(previous.clone(), suffix);
            let ops = DiffEngine::new()
                .diff(&current, &previous, &Dialect::Postgres)
                .expect("diff should succeed");
            let rebuilt = apply_ops(previous, &ops);
            prop_assert!(rebuilt.is_ok(), "apply failed: {:?}", rebuilt.err());
            let rebuilt = rebuilt.expect("checked ok");
            prop_assert_eq!(rebuilt, current);
        }
    }
}
