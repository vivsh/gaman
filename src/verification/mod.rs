//! Registry-driven drift verification for inspectable database properties.
//!
//! Verification is dialect-owned: each dialect registers the entity properties
//! it can compare accurately from live inspection. The generic engine matches
//! replayed and reflected entities, runs the registered comparators, and returns
//! structured findings plus repair operations.

mod mysql;
mod postgres;
mod sqlite;

use gaman_core::dialects::Dialect;
use gaman_core::operations::Operation;
use gaman_core::states::types::EntityKind;
use gaman_core::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Schema,
    Table, TriggerDef, ViewDef,
};

/// Result of live drift verification.
#[derive(Debug, Clone, Default)]
pub struct VerificationReport {
    pub findings: Vec<DriftFinding>,
    pub operations: Vec<Operation>,
    pub pending_migrations: Vec<String>,
}

/// A single property mismatch found by `verify_db`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriftFinding {
    pub operation: &'static str,
    pub entity_kind: EntityKind,
    pub entity_name: String,
    pub property: &'static str,
    pub expected: String,
    pub actual: String,
    pub note: Option<String>,
}

/// Result of comparing one verified property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PropertyMatch {
    Match,
    Drift {
        expected: String,
        actual: String,
        note: Option<String>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct VerificationContext<'a> {
    pub dialect: Dialect,
    pub table_name: Option<&'a str>,
}

pub(crate) struct TableProperty {
    pub name: &'static str,
    pub compare: fn(&Table, &Table, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct ColumnProperty {
    pub name: &'static str,
    pub compare: fn(&Column, &Column, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct PrimaryKeyProperty {
    pub name: &'static str,
    pub compare: fn(&PrimaryKey, &PrimaryKey, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct ForeignKeyProperty {
    pub name: &'static str,
    pub compare: fn(&ForeignKey, &ForeignKey, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct IndexProperty {
    pub name: &'static str,
    pub compare: fn(&Index, &Index, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct ConstraintProperty {
    pub name: &'static str,
    pub compare: fn(&Constraint, &Constraint, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct TriggerProperty {
    pub name: &'static str,
    pub compare: fn(&TriggerDef, &TriggerDef, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct FunctionProperty {
    pub name: &'static str,
    pub compare: fn(&FunctionDef, &FunctionDef, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct ViewProperty {
    pub name: &'static str,
    pub compare: fn(&ViewDef, &ViewDef, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct EnumProperty {
    pub name: &'static str,
    pub compare: fn(&EnumDef, &EnumDef, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct ExtensionProperty {
    pub name: &'static str,
    pub compare: fn(&ExtensionDef, &ExtensionDef, VerificationContext<'_>) -> PropertyMatch,
}

pub(crate) struct VerificationRegistry {
    pub tables: &'static [TableProperty],
    pub columns: &'static [ColumnProperty],
    pub primary_keys: &'static [PrimaryKeyProperty],
    pub foreign_keys: &'static [ForeignKeyProperty],
    pub indexes: &'static [IndexProperty],
    pub constraints: &'static [ConstraintProperty],
    pub triggers: &'static [TriggerProperty],
    pub functions: &'static [FunctionProperty],
    pub views: &'static [ViewProperty],
    pub enums: &'static [EnumProperty],
    pub extensions: &'static [ExtensionProperty],
}

/// Verifies replayed migration state against live inspected state.
pub(crate) fn verify(
    mut replay: Schema,
    mut live: Schema,
    schema: &str,
    dialect: Dialect,
) -> VerificationReport {
    scope_schema(&mut replay, schema, dialect);
    scope_schema(&mut live, schema, dialect);

    let registry = registry_for(dialect);
    let mut report = VerificationReport::default();
    let context = VerificationContext {
        dialect,
        table_name: None,
    };

    verify_top_level_objects(&replay, &live, registry, context, &mut report);
    verify_tables(&replay, &live, registry, context, &mut report);
    report
}

fn registry_for(dialect: Dialect) -> &'static VerificationRegistry {
    match dialect {
        Dialect::Postgres => postgres::registry(),
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => sqlite::registry(),
        Dialect::Mysql => mysql::registry(),
    }
}

fn verify_tables(
    replay: &Schema,
    live: &Schema,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    for (name, expected) in &replay.tables {
        let Some(actual) = live.tables.get(name) else {
            report.operations.push(Operation::CreateTable {
                table: expected.clone(),
            });
            report.findings.push(missing_finding(
                "create_table",
                EntityKind::Table,
                name,
                "presence",
            ));
            continue;
        };

        compare_table_properties(expected, actual, registry, context, report);
        verify_table_children(expected, actual, registry, context, report);
    }
}

fn verify_table_children(
    expected: &Table,
    actual: &Table,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    let table_name = expected.qualified_name();
    let context = VerificationContext {
        table_name: Some(&table_name),
        ..context
    };
    verify_columns(expected, actual, registry, context, report);
    verify_primary_key(expected, actual, registry, context, report);
    verify_named_items(
        &expected.foreign_keys,
        &actual.foreign_keys,
        |fk| fk.name.as_str(),
        |fk| Operation::AddForeignKey {
            table_name: table_name.clone(),
            foreign_key: fk.clone(),
        },
        |old, new| {
            vec![
                Operation::DropForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: old.clone(),
                    cascade: false,
                },
                Operation::AddForeignKey {
                    table_name: table_name.clone(),
                    foreign_key: new.clone(),
                },
            ]
        },
        EntityKind::ForeignKey,
        "add_foreign_key",
        "drop_add_foreign_key",
        registry.foreign_keys,
        compare_foreign_key_properties,
        context,
        report,
    );
    verify_named_items(
        &expected.indexes,
        &actual.indexes,
        |index| index.name.as_str(),
        |index| Operation::AddIndex {
            table_name: table_name.clone(),
            index: index.clone(),
            concurrent: false,
        },
        |old, new| {
            vec![
                Operation::DropIndex {
                    table_name: table_name.clone(),
                    index: old.clone(),
                    concurrent: false,
                },
                Operation::AddIndex {
                    table_name: table_name.clone(),
                    index: new.clone(),
                    concurrent: false,
                },
            ]
        },
        EntityKind::Index,
        "add_index",
        "drop_add_index",
        registry.indexes,
        compare_index_properties,
        context,
        report,
    );
    verify_named_items(
        &expected.constraints,
        &actual.constraints,
        Constraint::name,
        |constraint| Operation::AddConstraint {
            table_name: table_name.clone(),
            constraint: constraint.clone(),
        },
        |old, new| {
            vec![
                Operation::DropConstraint {
                    table_name: table_name.clone(),
                    constraint: old.clone(),
                },
                Operation::AddConstraint {
                    table_name: table_name.clone(),
                    constraint: new.clone(),
                },
            ]
        },
        EntityKind::Constraint,
        "add_constraint",
        "drop_add_constraint",
        registry.constraints,
        compare_constraint_properties,
        context,
        report,
    );
    verify_named_items(
        &expected.triggers,
        &actual.triggers,
        |trigger| trigger.name.as_deref().unwrap_or(""),
        |trigger| Operation::CreateTrigger {
            table_name: table_name.clone(),
            trigger: sanitized_trigger(trigger),
        },
        |old, new| {
            vec![Operation::AlterTrigger {
                table_name: table_name.clone(),
                old: sanitized_trigger(old),
                new: sanitized_trigger(new),
            }]
        },
        EntityKind::Trigger,
        "create_trigger",
        "alter_trigger",
        registry.triggers,
        compare_trigger_properties,
        context,
        report,
    );
}

fn verify_columns(
    expected: &Table,
    actual: &Table,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    for column in &expected.columns {
        let Some(actual_column) = actual.columns.iter().find(|item| item.name == column.name)
        else {
            report.operations.push(Operation::AddColumn {
                table_name: expected.qualified_name(),
                column: column.clone(),
            });
            report.findings.push(missing_finding(
                "add_column",
                EntityKind::Column,
                &format!("{}.{}", expected.qualified_name(), column.name),
                "presence",
            ));
            continue;
        };

        let findings = compare_column_properties(column, actual_column, registry, context);
        if !findings.is_empty() {
            report.operations.push(Operation::AlterColumn {
                table_name: expected.qualified_name(),
                old: column_for_operation(actual_column, context.dialect),
                new: column_for_operation(column, context.dialect),
                cast_expr: None,
            });
            report.findings.extend(findings);
        }
    }
}

fn verify_primary_key(
    expected: &Table,
    actual: &Table,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    let Some(expected_pk) = &expected.primary_key else {
        return;
    };
    let entity_name = format!("{}.{}", expected.qualified_name(), expected_pk.name);
    let Some(actual_pk) = &actual.primary_key else {
        report.findings.push(missing_finding(
            "alter_primary_key",
            EntityKind::Constraint,
            &entity_name,
            "presence",
        ));
        return;
    };
    let findings = compare_primary_key_properties(
        expected_pk,
        actual_pk,
        registry.primary_keys,
        context,
        &entity_name,
    );
    report.findings.extend(findings);
}

fn verify_top_level_objects(
    replay: &Schema,
    live: &Schema,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    verify_top_map(
        &replay.functions,
        &live.functions,
        |function| function_verify_key(function),
        |function| Operation::CreateFunction {
            function: sanitized_function(function),
        },
        |old, new| {
            vec![Operation::AlterFunction {
                old: sanitized_function(old),
                new: sanitized_function(new),
            }]
        },
        EntityKind::Function,
        "create_function",
        "alter_function",
        registry.functions,
        compare_function_properties,
        context,
        report,
    );
    verify_top_map(
        &replay.views,
        &live.views,
        ViewDef::qualified_name,
        |view| Operation::CreateView {
            view: sanitized_view(view),
        },
        |old, new| {
            vec![Operation::ReplaceView {
                old: sanitized_view(old),
                new: sanitized_view(new),
            }]
        },
        EntityKind::View,
        "create_view",
        "replace_view",
        registry.views,
        compare_view_properties,
        context,
        report,
    );
    verify_top_map(
        &replay.enums,
        &live.enums,
        EnumDef::qualified_name,
        |enum_def| Operation::CreateEnum {
            enum_def: enum_def.clone(),
        },
        enum_repair_operations,
        EntityKind::Enum,
        "create_enum",
        "alter_enum",
        registry.enums,
        compare_enum_properties,
        context,
        report,
    );
    verify_top_map(
        &replay.extensions,
        &live.extensions,
        ExtensionDef::qualified_name,
        |extension| Operation::CreateExtension {
            extension: extension.clone(),
        },
        |old, new| {
            vec![
                Operation::DropExtension {
                    extension: old.clone(),
                },
                Operation::CreateExtension {
                    extension: new.clone(),
                },
            ]
        },
        EntityKind::Extension,
        "create_extension",
        "alter_extension",
        registry.extensions,
        compare_extension_properties,
        context,
        report,
    );
}

fn verify_top_map<T, P>(
    expected: &std::collections::BTreeMap<String, T>,
    actual: &std::collections::BTreeMap<String, T>,
    key: fn(&T) -> String,
    create: impl Fn(&T) -> Operation,
    alter: impl Fn(&T, &T) -> Vec<Operation>,
    kind: EntityKind,
    create_op: &'static str,
    alter_op: &'static str,
    properties: &'static [P],
    compare: fn(&T, &T, &'static [P], VerificationContext<'_>, &str) -> Vec<DriftFinding>,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) where
    P: RegisteredProperty<T> + 'static,
{
    for expected_value in expected.values() {
        let expected_key = key(expected_value);
        let Some(actual_value) = actual.values().find(|item| key(item) == expected_key) else {
            report.operations.push(create(expected_value));
            report
                .findings
                .push(missing_finding(create_op, kind, &expected_key, "presence"));
            continue;
        };
        let findings = compare(
            expected_value,
            actual_value,
            properties,
            context,
            &expected_key,
        );
        if !findings.is_empty() {
            report
                .operations
                .extend(alter(actual_value, expected_value));
            report
                .findings
                .extend(findings.into_iter().map(|finding| DriftFinding {
                    operation: alter_op,
                    ..finding
                }));
        }
    }
}

fn enum_repair_operations(old: &EnumDef, new: &EnumDef) -> Vec<Operation> {
    let old_set: std::collections::HashSet<&str> = old.values.iter().map(String::as_str).collect();
    let new_set: std::collections::HashSet<&str> = new.values.iter().map(String::as_str).collect();
    if old_set.is_subset(&new_set) && values_are_subsequence(&old.values, &new.values) {
        vec![Operation::AlterEnum {
            old: old.clone(),
            new: new.clone(),
        }]
    } else {
        vec![
            Operation::DropEnum {
                enum_def: old.clone(),
            },
            Operation::CreateEnum {
                enum_def: new.clone(),
            },
        ]
    }
}

fn values_are_subsequence(old: &[String], new: &[String]) -> bool {
    let mut new_iter = new.iter();
    old.iter()
        .all(|old_value| new_iter.by_ref().any(|new_value| new_value == old_value))
}

fn sanitized_function(function: &FunctionDef) -> FunctionDef {
    let mut function = function.clone();
    function.body.clear();
    function
}

fn sanitized_view(view: &ViewDef) -> ViewDef {
    let mut view = view.clone();
    view.definition.clear();
    view
}

fn sanitized_trigger(trigger: &TriggerDef) -> TriggerDef {
    let mut trigger = trigger.clone();
    trigger.query = None;
    trigger.when = None;
    trigger.language = None;
    trigger
}

fn column_for_operation(column: &Column, dialect: Dialect) -> Column {
    let mut column = column.clone();
    if dialect == Dialect::Postgres {
        column.default = postgres::canonical_column_default(column.default.as_deref());
    }
    column
}

fn verify_named_items<T, P>(
    expected: &[T],
    actual: &[T],
    name: fn(&T) -> &str,
    create: impl Fn(&T) -> Operation,
    alter: impl Fn(&T, &T) -> Vec<Operation>,
    kind: EntityKind,
    create_op: &'static str,
    alter_op: &'static str,
    properties: &'static [P],
    compare: fn(
        &T,
        &T,
        &'static [P],
        VerificationContext<'_>,
        &str,
        &'static str,
        EntityKind,
    ) -> Vec<DriftFinding>,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    for expected_value in expected {
        let expected_name = name(expected_value);
        let entity_name = scoped_child_name(context.table_name, expected_name);
        let Some(actual_value) = actual.iter().find(|item| name(item) == expected_name) else {
            report.operations.push(create(expected_value));
            report
                .findings
                .push(missing_finding(create_op, kind, &entity_name, "presence"));
            continue;
        };
        let findings = compare(
            expected_value,
            actual_value,
            properties,
            context,
            &entity_name,
            alter_op,
            kind,
        );
        if !findings.is_empty() {
            report
                .operations
                .extend(alter(actual_value, expected_value));
            report.findings.extend(findings);
        }
    }
}

fn compare_table_properties(
    expected: &Table,
    actual: &Table,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
    report: &mut VerificationReport,
) {
    for property in registry.tables {
        if let PropertyMatch::Drift {
            expected: expected_value,
            actual: actual_value,
            note,
        } = (property.compare)(expected, actual, context)
        {
            report.findings.push(finding(
                "alter_table",
                EntityKind::Table,
                &expected.qualified_name(),
                property.name,
                expected_value,
                actual_value,
                note,
            ));
        }
    }
}

fn compare_column_properties(
    expected: &Column,
    actual: &Column,
    registry: &VerificationRegistry,
    context: VerificationContext<'_>,
) -> Vec<DriftFinding> {
    let entity_name = scoped_child_name(context.table_name, &actual.name);
    registry
        .columns
        .iter()
        .filter_map(
            |property| match (property.compare)(expected, actual, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    actual,
                    note,
                } => Some(finding(
                    "alter_column",
                    EntityKind::Column,
                    &entity_name,
                    property.name,
                    expected,
                    actual,
                    note,
                )),
            },
        )
        .collect()
}

fn compare_primary_key_properties(
    expected: &PrimaryKey,
    actual: &PrimaryKey,
    properties: &'static [PrimaryKeyProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    properties
        .iter()
        .filter_map(
            |property| match (property.compare)(expected, actual, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    actual,
                    note,
                } => Some(finding(
                    "alter_primary_key",
                    EntityKind::Constraint,
                    entity_name,
                    property.name,
                    expected,
                    actual,
                    note,
                )),
            },
        )
        .collect()
}

fn compare_foreign_key_properties(
    expected: &ForeignKey,
    actual: &ForeignKey,
    properties: &'static [ForeignKeyProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_index_properties(
    expected: &Index,
    actual: &Index,
    properties: &'static [IndexProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_constraint_properties(
    expected: &Constraint,
    actual: &Constraint,
    properties: &'static [ConstraintProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_trigger_properties(
    expected: &TriggerDef,
    actual: &TriggerDef,
    properties: &'static [TriggerProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_function_properties(
    expected: &FunctionDef,
    actual: &FunctionDef,
    properties: &'static [FunctionProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        "alter_function",
        EntityKind::Function,
    )
}

fn compare_view_properties(
    expected: &ViewDef,
    actual: &ViewDef,
    properties: &'static [ViewProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        "replace_view",
        EntityKind::View,
    )
}

fn compare_enum_properties(
    expected: &EnumDef,
    actual: &EnumDef,
    properties: &'static [EnumProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        "alter_enum",
        EntityKind::Enum,
    )
}

fn compare_extension_properties(
    expected: &ExtensionDef,
    actual: &ExtensionDef,
    properties: &'static [ExtensionProperty],
    context: VerificationContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        actual,
        properties,
        context,
        entity_name,
        "alter_extension",
        EntityKind::Extension,
    )
}

trait RegisteredProperty<T> {
    fn name(&self) -> &'static str;
    fn compare(&self) -> fn(&T, &T, VerificationContext<'_>) -> PropertyMatch;
}

macro_rules! impl_registered_property {
    ($ty:ty, $target:ty) => {
        impl RegisteredProperty<$target> for $ty {
            fn name(&self) -> &'static str {
                self.name
            }
            fn compare(&self) -> fn(&$target, &$target, VerificationContext<'_>) -> PropertyMatch {
                self.compare
            }
        }
    };
}

impl_registered_property!(ForeignKeyProperty, ForeignKey);
impl_registered_property!(IndexProperty, Index);
impl_registered_property!(ConstraintProperty, Constraint);
impl_registered_property!(TriggerProperty, TriggerDef);
impl_registered_property!(FunctionProperty, FunctionDef);
impl_registered_property!(ViewProperty, ViewDef);
impl_registered_property!(EnumProperty, EnumDef);
impl_registered_property!(ExtensionProperty, ExtensionDef);

fn compare_properties<T, P>(
    expected: &T,
    actual: &T,
    properties: &'static [P],
    context: VerificationContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding>
where
    P: RegisteredProperty<T> + 'static,
{
    properties
        .iter()
        .filter_map(
            |property| match (property.compare())(expected, actual, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    actual,
                    note,
                } => Some(finding(
                    operation,
                    kind,
                    entity_name,
                    property.name(),
                    expected,
                    actual,
                    note,
                )),
            },
        )
        .collect()
}

fn scope_schema(state: &mut Schema, schema: &str, dialect: Dialect) {
    scope_tables(state, schema, dialect);
    scope_views(state, schema, dialect);
    scope_functions(state, schema, dialect);
    scope_extensions(state, schema, dialect);
    scope_enums(state, schema, dialect);
}

fn scope_tables(state: &mut Schema, schema: &str, dialect: Dialect) {
    let tables = std::mem::take(&mut state.tables);
    state.tables = tables
        .into_values()
        .filter_map(|mut table| match table.schema.as_deref() {
            None => {
                scope_table_references(&mut table, schema);
                Some(table)
            }
            Some(current)
                if schema_matches_verify_scope(dialect, EntityKind::Table, current, schema) =>
            {
                table.schema = None;
                scope_table_references(&mut table, schema);
                Some(table)
            }
            _ => None,
        })
        .map(|table| (table.qualified_name(), table))
        .collect();
}

fn scope_views(state: &mut Schema, schema: &str, dialect: Dialect) {
    let views = std::mem::take(&mut state.views);
    state.views = views
        .into_values()
        .filter_map(|mut view| match view.schema.as_deref() {
            None => Some(view),
            Some(current)
                if schema_matches_verify_scope(dialect, EntityKind::View, current, schema) =>
            {
                view.schema = None;
                Some(view)
            }
            _ => None,
        })
        .map(|view| (view.qualified_name(), view))
        .collect();
}

fn scope_functions(state: &mut Schema, schema: &str, dialect: Dialect) {
    let functions = std::mem::take(&mut state.functions);
    state.functions = functions
        .into_values()
        .filter_map(|mut function| match function.schema.as_deref() {
            None => Some(function),
            Some(current)
                if schema_matches_verify_scope(dialect, EntityKind::Function, current, schema) =>
            {
                function.schema = None;
                Some(function)
            }
            _ => None,
        })
        .map(|function| {
            let key = function_verify_key(&function);
            (key, function)
        })
        .collect();
}

fn scope_extensions(state: &mut Schema, schema: &str, dialect: Dialect) {
    let extensions = std::mem::take(&mut state.extensions);
    state.extensions = extensions
        .into_values()
        .filter_map(|mut extension| match extension.schema.as_deref() {
            None => Some(extension),
            Some(current)
                if schema_matches_verify_scope(dialect, EntityKind::Extension, current, schema) =>
            {
                extension.schema = None;
                Some(extension)
            }
            _ => None,
        })
        .map(|extension| (extension.qualified_name(), extension))
        .collect();
}

fn scope_enums(state: &mut Schema, schema: &str, dialect: Dialect) {
    let enums = std::mem::take(&mut state.enums);
    state.enums = enums
        .into_values()
        .filter_map(|mut enum_def| match enum_def.schema.as_deref() {
            None => Some(enum_def),
            Some(current)
                if schema_matches_verify_scope(dialect, EntityKind::Enum, current, schema) =>
            {
                enum_def.schema = None;
                Some(enum_def)
            }
            _ => None,
        })
        .map(|enum_def| (enum_def.qualified_name(), enum_def))
        .collect();
}

fn schema_matches_verify_scope(
    dialect: Dialect,
    kind: EntityKind,
    current: &str,
    requested: &str,
) -> bool {
    let current = dialect.canonicalize_schema_name(kind, Some(current));
    let requested = dialect.canonicalize_schema_name(kind, Some(requested));
    current == requested
}

fn scope_table_references(table: &mut Table, schema: &str) {
    let prefix = format!("{schema}.");
    for fk in &mut table.foreign_keys {
        if let Some(local) = fk.to_table.strip_prefix(&prefix) {
            fk.to_table = local.to_string();
        }
    }
    for trigger in &mut table.triggers {
        if let Some(function_name) = &mut trigger.function_name
            && let Some(local) = function_name.strip_prefix(&prefix)
        {
            *function_name = local.to_string();
        }
    }
}

fn function_verify_key(function: &FunctionDef) -> String {
    if function.arguments.is_empty() {
        function.qualified_name()
    } else {
        format!("{}({})", function.qualified_name(), function.arguments)
    }
}

pub fn format_report(report: &VerificationReport) -> Vec<String> {
    use std::collections::BTreeMap;

    let pending = report
        .pending_migrations
        .iter()
        .map(|id| format!("  pending migration: {id}"));
    let mut grouped: BTreeMap<(&'static str, String), Vec<&DriftFinding>> = BTreeMap::new();
    for finding in &report.findings {
        grouped
            .entry((finding.operation, finding.entity_name.clone()))
            .or_default()
            .push(finding);
    }

    pending
        .chain(
            grouped
                .into_iter()
                .flat_map(|((operation, entity), findings)| {
                    let mut lines = vec![format!("  drift: {operation} {entity}")];
                    lines.extend(findings.into_iter().map(|finding| {
                        let mut line = format!(
                            "    {}: expected {}, found {}",
                            finding.property, finding.expected, finding.actual
                        );
                        if let Some(note) = &finding.note {
                            line.push_str(&format!(" ({note})"));
                        }
                        line
                    }));
                    lines
                }),
        )
        .collect()
}

fn finding(
    operation: &'static str,
    entity_kind: EntityKind,
    entity_name: &str,
    property: &'static str,
    expected: String,
    actual: String,
    note: Option<String>,
) -> DriftFinding {
    DriftFinding {
        operation,
        entity_kind,
        entity_name: entity_name.to_string(),
        property,
        expected,
        actual,
        note,
    }
}

fn missing_finding(
    operation: &'static str,
    entity_kind: EntityKind,
    entity_name: &str,
    property: &'static str,
) -> DriftFinding {
    finding(
        operation,
        entity_kind,
        entity_name,
        property,
        "present".to_string(),
        "<missing>".to_string(),
        None,
    )
}

fn scoped_child_name(table_name: Option<&str>, child_name: &str) -> String {
    match table_name {
        Some(table) if !child_name.is_empty() => format!("{table}.{child_name}"),
        Some(table) => table.to_string(),
        None => child_name.to_string(),
    }
}

pub(crate) fn exact_string(expected: &str, actual: &str) -> PropertyMatch {
    if expected == actual {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_str(expected),
            actual: display_str(actual),
            note: None,
        }
    }
}

pub(crate) fn exact_bool(expected: bool, actual: bool) -> PropertyMatch {
    if expected == actual {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected.to_string(),
            actual: actual.to_string(),
            note: None,
        }
    }
}

pub(crate) fn exact_option(expected: &Option<String>, actual: &Option<String>) -> PropertyMatch {
    if expected == actual {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_option(expected),
            actual: display_option(actual),
            note: None,
        }
    }
}

pub(crate) fn exact_vec(expected: &[String], actual: &[String]) -> PropertyMatch {
    if expected == actual {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_vec(expected),
            actual: display_vec(actual),
            note: None,
        }
    }
}

pub(crate) fn display_option(value: &Option<String>) -> String {
    value
        .as_deref()
        .map(display_str)
        .unwrap_or_else(|| "<none>".to_string())
}

pub(crate) fn display_str(value: &str) -> String {
    if value.is_empty() {
        "<empty>".to_string()
    } else {
        value.to_string()
    }
}

pub(crate) fn display_vec(values: &[String]) -> String {
    if values.is_empty() {
        "[]".to_string()
    } else {
        format!("[{}]", values.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use gaman_core::dialects::Dialect;
    use gaman_core::states::{Column, FunctionDef, Schema, Table, Volatility};

    use super::{format_report, verify};

    /// Verifies PostgreSQL literal casts do not create default drift.
    #[test]
    fn postgres_literal_text_cast_default_matches() {
        let replay = schema_with_column(defaulted_column("theme", Some("'light'")));
        let live = schema_with_column(defaulted_column("theme", Some("'light'::text")));

        let report = verify(replay, live, "public", Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {:?}", report);
    }

    /// Verifies real default drift includes the table, column, and property name.
    #[test]
    fn column_default_drift_reports_entity_property_and_values() {
        let replay = schema_with_column(defaulted_column("posts_per_page", None));
        let live = schema_with_column(defaulted_column("posts_per_page", Some("10")));

        let report = verify(replay, live, "public", Dialect::Postgres);
        let lines = format_report(&report);

        assert!(
            lines
                .iter()
                .any(|line| { line.contains("drift: alter_column preferences.posts_per_page") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line.contains("default: expected <none>, found 10") })
        );
    }

    /// Verifies function body-only differences are outside live drift detection.
    #[test]
    fn function_body_only_drift_is_not_reported() {
        let mut replay = Schema::default();
        replay
            .functions
            .insert("audit_users".to_string(), function("SELECT 1"));
        let mut live = Schema::default();
        live.functions
            .insert("audit_users".to_string(), function("SELECT 2"));

        let report = verify(replay, live, "public", Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {:?}", report);
    }

    fn schema_with_column(column: Column) -> Schema {
        let mut schema = Schema::default();
        schema.tables.insert(
            "preferences".to_string(),
            Table {
                name: "preferences".to_string(),
                schema: None,
                primary_key: None,
                columns: vec![column],
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                triggers: Vec::new(),
            },
        );
        schema
    }

    fn defaulted_column(name: &str, default: Option<&str>) -> Column {
        Column {
            name: name.to_string(),
            col_type: "text".to_string(),
            nullable: false,
            default: default.map(ToString::to_string),
            primary_key: false,
            references: None,
            check: None,
            generated: None,
        }
    }

    fn function(body: &str) -> FunctionDef {
        FunctionDef {
            name: "audit_users".to_string(),
            schema: None,
            arguments: String::new(),
            returns: "trigger".to_string(),
            language: "plpgsql".to_string(),
            body: body.to_string(),
            volatility: Volatility::Volatile,
            security_definer: false,
        }
    }
}
