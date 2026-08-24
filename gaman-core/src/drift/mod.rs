//! Registry-driven drift verification for inspectable database properties.
//!
//! Verification is dialect-owned: each dialect registers the entity properties
//! it can compare accurately from live inspection. The generic engine matches
//! replayed and reflected entities, runs the registered comparators, and returns
//! structured findings plus repair operations.

mod contract;
pub(crate) mod mariadb;
pub(crate) mod mysql;
mod mysql_family;
pub(crate) mod postgres;
mod report;
pub(crate) mod sqlite;

use std::sync::OnceLock;

use crate::dialects::Dialect;
use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Schema,
    SequenceDef, Table, TriggerDef, ViewDef,
};

pub use contract::DriftPropertyDoc;
pub use report::{DriftFinding, VerificationReport};

/// Result of comparing one verified property.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PropertyMatch {
    Match,
    Drift {
        expected: String,
        observed: String,
        note: Option<String>,
    },
}

#[derive(Clone, Copy)]
pub(crate) struct DriftContext<'a> {
    pub dialect: Dialect,
    pub table_name: Option<&'a str>,
}

pub(crate) struct TableProperty {
    pub name: &'static str,
    pub compare: fn(&Table, &Table, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct ColumnProperty {
    pub name: &'static str,
    pub compare: fn(&Column, &Column, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct PrimaryKeyProperty {
    pub name: &'static str,
    pub compare: fn(&PrimaryKey, &PrimaryKey, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct ForeignKeyProperty {
    pub name: &'static str,
    pub compare: fn(&ForeignKey, &ForeignKey, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct IndexProperty {
    pub name: &'static str,
    pub compare: fn(&Index, &Index, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct ConstraintProperty {
    pub name: &'static str,
    pub compare: fn(&Constraint, &Constraint, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct TriggerProperty {
    pub name: &'static str,
    pub compare: fn(&TriggerDef, &TriggerDef, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct FunctionProperty {
    pub name: &'static str,
    pub compare: fn(&FunctionDef, &FunctionDef, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct ViewProperty {
    pub name: &'static str,
    pub compare: fn(&ViewDef, &ViewDef, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct EnumProperty {
    pub name: &'static str,
    pub compare: fn(&EnumDef, &EnumDef, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct ExtensionProperty {
    pub name: &'static str,
    pub compare: fn(&ExtensionDef, &ExtensionDef, DriftContext<'_>) -> PropertyMatch,
}

pub(crate) struct DriftRegistry {
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

/// Returns the user-facing semantic drift contract for a dialect.
pub fn contract_for(dialect: Dialect) -> &'static [DriftPropertyDoc] {
    static POSTGRES: OnceLock<Vec<DriftPropertyDoc>> = OnceLock::new();
    static SQLITE: OnceLock<Vec<DriftPropertyDoc>> = OnceLock::new();
    static MYSQL: OnceLock<Vec<DriftPropertyDoc>> = OnceLock::new();
    static MARIADB: OnceLock<Vec<DriftPropertyDoc>> = OnceLock::new();

    let contract = match dialect {
        Dialect::Postgres => &POSTGRES,
        Dialect::Sqlite => &SQLITE,
        Dialect::Mysql => &MYSQL,
        Dialect::Mariadb => &MARIADB,
    };
    contract.get_or_init(|| registry_for(dialect).contract())
}

impl DriftRegistry {
    pub(crate) fn contract(&self) -> Vec<DriftPropertyDoc> {
        let mut docs = Vec::new();
        push_docs(
            &mut docs,
            EntityKind::Table,
            self.tables.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Column,
            self.columns.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Table,
            self.primary_keys.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::ForeignKey,
            self.foreign_keys.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Index,
            self.indexes.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Constraint,
            self.constraints.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Trigger,
            self.triggers.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Function,
            self.functions.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::View,
            self.views.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Enum,
            self.enums.iter().map(|p| p.name),
        );
        push_docs(
            &mut docs,
            EntityKind::Extension,
            self.extensions.iter().map(|p| p.name),
        );
        docs
    }
}

fn push_docs<I>(docs: &mut Vec<DriftPropertyDoc>, entity_kind: EntityKind, properties: I)
where
    I: IntoIterator<Item = &'static str>,
{
    docs.extend(
        properties
            .into_iter()
            .map(|property| contract::property_doc(entity_kind, property)),
    );
}

/// Verifies replayed migration state against live inspected state.
pub fn diff(replay: Schema, live: Schema, schema: &str, dialect: Dialect) -> VerificationReport {
    diff_schemas(replay, live, &[schema], dialect)
}

/// Verifies replayed state across multiple inspected schemas.
///
/// Unqualified migration objects belong to the first schema. Objects explicitly
/// qualified with later schemas retain their qualification during comparison.
pub fn diff_schemas(
    mut replay: Schema,
    mut live: Schema,
    schemas: &[&str],
    dialect: Dialect,
) -> VerificationReport {
    scope_schemas(&mut replay, schemas, dialect);
    scope_schemas(&mut live, schemas, dialect);

    let registry = registry_for(dialect);
    let mut report = VerificationReport::default();
    let context = DriftContext {
        dialect,
        table_name: None,
    };

    verify_top_level_objects(&replay, &live, registry, context, &mut report);
    verify_tables(&replay, &live, registry, context, &mut report);
    report
}

pub(crate) fn registry_for(dialect: Dialect) -> &'static DriftRegistry {
    dialect.drift_registry()
}

fn verify_tables(
    replay: &Schema,
    live: &Schema,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    for (name, expected) in &replay.tables {
        let Some(observed) = live.tables.get(name) else {
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

        compare_table_properties(expected, observed, registry, context, report);
        verify_table_children(expected, observed, registry, context, report);
    }
}

fn verify_table_children(
    expected: &Table,
    observed: &Table,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    let table_name = expected.qualified_name();
    let context = DriftContext {
        table_name: Some(&table_name),
        ..context
    };
    verify_columns(expected, observed, registry, context, report);
    verify_primary_key(expected, observed, registry, context, report);
    verify_named_items(
        &expected.foreign_keys,
        &observed.foreign_keys,
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
        &observed.indexes,
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
        &observed.constraints,
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
        &observed.triggers,
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
    observed: &Table,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    for column in &expected.columns {
        let Some(actual_column) = observed
            .columns
            .iter()
            .find(|item| item.name == column.name)
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
            let old = column_for_operation(actual_column, context.dialect);
            let new = context.dialect.column_for_repair(
                &column_for_operation(column, context.dialect),
                actual_column,
            );
            report.operations.push(Operation::AlterColumn {
                table_name: expected.qualified_name(),
                cast_expr: context.dialect.repair_cast_expr(&old, &new),
                old,
                new,
            });
            report.findings.extend(findings);
        }
    }
}

fn verify_primary_key(
    expected: &Table,
    observed: &Table,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    let Some(expected_pk) = &expected.primary_key else {
        return;
    };
    let entity_name = format!("{}.{}", expected.qualified_name(), expected_pk.name);
    let Some(actual_pk) = &observed.primary_key else {
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
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    verify_sequence_presence(&replay.sequences, &live.sequences, report);
    verify_top_map(
        &replay.functions,
        &live.functions,
        function_verify_key,
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

fn verify_sequence_presence(
    expected: &std::collections::BTreeMap<String, SequenceDef>,
    observed: &std::collections::BTreeMap<String, SequenceDef>,
    report: &mut VerificationReport,
) {
    for sequence in expected.values() {
        let identity = sequence.qualified_name();
        if observed
            .values()
            .any(|item| item.qualified_name() == identity)
        {
            continue;
        }
        report.operations.push(Operation::CreateSequence {
            sequence: sequence.clone(),
        });
        report.findings.push(missing_finding(
            "create_sequence",
            EntityKind::Sequence,
            &identity,
            "presence",
        ));
    }
}

#[allow(clippy::too_many_arguments)]
fn verify_top_map<T, P>(
    expected: &std::collections::BTreeMap<String, T>,
    observed: &std::collections::BTreeMap<String, T>,
    key: fn(&T) -> String,
    create: impl Fn(&T) -> Operation,
    alter: impl Fn(&T, &T) -> Vec<Operation>,
    kind: EntityKind,
    create_op: &'static str,
    alter_op: &'static str,
    properties: &'static [P],
    compare: fn(&T, &T, &'static [P], DriftContext<'_>, &str) -> Vec<DriftFinding>,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) where
    P: RegisteredProperty<T> + 'static,
    T: DriftOpaque,
{
    for expected_value in expected.values() {
        let expected_key = key(expected_value);
        let Some(actual_value) = observed.values().find(|item| key(item) == expected_key) else {
            report.operations.push(create(expected_value));
            report
                .findings
                .push(missing_finding(create_op, kind, &expected_key, "presence"));
            continue;
        };
        if expected_value.is_drift_opaque() {
            continue;
        }
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

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn verify_named_items<T, P>(
    expected: &[T],
    observed: &[T],
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
        DriftContext<'_>,
        &str,
        &'static str,
        EntityKind,
    ) -> Vec<DriftFinding>,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) where
    T: DriftOpaque,
{
    for expected_value in expected {
        let expected_name = name(expected_value);
        let entity_name = scoped_child_name(context.table_name, expected_name);
        let Some(actual_value) = observed.iter().find(|item| name(item) == expected_name) else {
            report.operations.push(create(expected_value));
            report
                .findings
                .push(missing_finding(create_op, kind, &entity_name, "presence"));
            continue;
        };
        if expected_value.is_drift_opaque() {
            continue;
        }
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

trait DriftOpaque {
    fn is_drift_opaque(&self) -> bool {
        false
    }
}

impl DriftOpaque for ForeignKey {}
impl DriftOpaque for Constraint {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}
impl DriftOpaque for EnumDef {}

impl DriftOpaque for Index {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}

impl DriftOpaque for TriggerDef {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}

impl DriftOpaque for FunctionDef {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}

impl DriftOpaque for ViewDef {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}

impl DriftOpaque for ExtensionDef {
    fn is_drift_opaque(&self) -> bool {
        self.is_opaque()
    }
}

fn compare_table_properties(
    expected: &Table,
    observed: &Table,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
    report: &mut VerificationReport,
) {
    for property in registry.tables {
        if let PropertyMatch::Drift {
            expected: expected_value,
            observed: actual_value,
            note,
        } = (property.compare)(expected, observed, context)
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
    observed: &Column,
    registry: &DriftRegistry,
    context: DriftContext<'_>,
) -> Vec<DriftFinding> {
    let entity_name = scoped_child_name(context.table_name, &observed.name);
    registry
        .columns
        .iter()
        .filter_map(
            |property| match (property.compare)(expected, observed, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    observed,
                    note,
                } => Some(finding(
                    "alter_column",
                    EntityKind::Column,
                    &entity_name,
                    property.name,
                    expected,
                    observed,
                    note,
                )),
            },
        )
        .collect()
}

fn compare_primary_key_properties(
    expected: &PrimaryKey,
    observed: &PrimaryKey,
    properties: &'static [PrimaryKeyProperty],
    context: DriftContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    properties
        .iter()
        .filter_map(
            |property| match (property.compare)(expected, observed, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    observed,
                    note,
                } => Some(finding(
                    "alter_primary_key",
                    EntityKind::Constraint,
                    entity_name,
                    property.name,
                    expected,
                    observed,
                    note,
                )),
            },
        )
        .collect()
}

fn compare_foreign_key_properties(
    expected: &ForeignKey,
    observed: &ForeignKey,
    properties: &'static [ForeignKeyProperty],
    context: DriftContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_index_properties(
    expected: &Index,
    observed: &Index,
    properties: &'static [IndexProperty],
    context: DriftContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_constraint_properties(
    expected: &Constraint,
    observed: &Constraint,
    properties: &'static [ConstraintProperty],
    context: DriftContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_trigger_properties(
    expected: &TriggerDef,
    observed: &TriggerDef,
    properties: &'static [TriggerProperty],
    context: DriftContext<'_>,
    entity_name: &str,
    operation: &'static str,
    kind: EntityKind,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        operation,
        kind,
    )
}

fn compare_function_properties(
    expected: &FunctionDef,
    observed: &FunctionDef,
    properties: &'static [FunctionProperty],
    context: DriftContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        "alter_function",
        EntityKind::Function,
    )
}

fn compare_view_properties(
    expected: &ViewDef,
    observed: &ViewDef,
    properties: &'static [ViewProperty],
    context: DriftContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        "replace_view",
        EntityKind::View,
    )
}

fn compare_enum_properties(
    expected: &EnumDef,
    observed: &EnumDef,
    properties: &'static [EnumProperty],
    context: DriftContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        "alter_enum",
        EntityKind::Enum,
    )
}

fn compare_extension_properties(
    expected: &ExtensionDef,
    observed: &ExtensionDef,
    properties: &'static [ExtensionProperty],
    context: DriftContext<'_>,
    entity_name: &str,
) -> Vec<DriftFinding> {
    compare_properties(
        expected,
        observed,
        properties,
        context,
        entity_name,
        "alter_extension",
        EntityKind::Extension,
    )
}

trait RegisteredProperty<T> {
    fn name(&self) -> &'static str;
    fn compare(&self) -> fn(&T, &T, DriftContext<'_>) -> PropertyMatch;
}

macro_rules! impl_registered_property {
    ($ty:ty, $target:ty) => {
        impl RegisteredProperty<$target> for $ty {
            fn name(&self) -> &'static str {
                self.name
            }
            fn compare(&self) -> fn(&$target, &$target, DriftContext<'_>) -> PropertyMatch {
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
    observed: &T,
    properties: &'static [P],
    context: DriftContext<'_>,
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
            |property| match (property.compare())(expected, observed, context) {
                PropertyMatch::Match => None,
                PropertyMatch::Drift {
                    expected,
                    observed,
                    note,
                } => Some(finding(
                    operation,
                    kind,
                    entity_name,
                    property.name(),
                    expected,
                    observed,
                    note,
                )),
            },
        )
        .collect()
}

fn scope_schemas(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    scope_tables(state, schemas, dialect);
    scope_views(state, schemas, dialect);
    scope_functions(state, schemas, dialect);
    scope_extensions(state, schemas, dialect);
    scope_sequences(state, schemas, dialect);
    scope_enums(state, schemas, dialect);
}

fn scope_sequences(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let sequences = std::mem::take(&mut state.sequences);
    state.sequences = sequences
        .into_values()
        .filter_map(|mut sequence| {
            sequence.schema = scoped_schema(
                dialect,
                EntityKind::Sequence,
                sequence.schema.as_deref(),
                schemas,
            )?;
            Some(sequence)
        })
        .map(|sequence| (sequence.qualified_name(), sequence))
        .collect();
}

fn scope_tables(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let tables = std::mem::take(&mut state.tables);
    state.tables = tables
        .into_values()
        .filter_map(|mut table| {
            table.schema =
                scoped_schema(dialect, EntityKind::Table, table.schema.as_deref(), schemas)?;
            if let Some(default_schema) = schemas.first() {
                scope_table_references(&mut table, default_schema);
            }
            Some(table)
        })
        .map(|table| (table.qualified_name(), table))
        .collect();
}

fn scope_views(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let views = std::mem::take(&mut state.views);
    state.views = views
        .into_values()
        .filter_map(|mut view| {
            view.schema =
                scoped_schema(dialect, EntityKind::View, view.schema.as_deref(), schemas)?;
            Some(view)
        })
        .map(|view| (view.qualified_name(), view))
        .collect();
}

fn scope_functions(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let functions = std::mem::take(&mut state.functions);
    state.functions = functions
        .into_values()
        .filter_map(|mut function| {
            function.schema = scoped_schema(
                dialect,
                EntityKind::Function,
                function.schema.as_deref(),
                schemas,
            )?;
            Some(function)
        })
        .map(|function| {
            let key = function_verify_key(&function);
            (key, function)
        })
        .collect();
}

fn scope_extensions(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let extensions = std::mem::take(&mut state.extensions);
    state.extensions = extensions
        .into_values()
        .filter_map(|mut extension| {
            extension.schema = scoped_schema(
                dialect,
                EntityKind::Extension,
                extension.schema.as_deref(),
                schemas,
            )?;
            Some(extension)
        })
        .map(|extension| (extension.qualified_name(), extension))
        .collect();
}

fn scope_enums(state: &mut Schema, schemas: &[&str], dialect: Dialect) {
    let enums = std::mem::take(&mut state.enums);
    state.enums = enums
        .into_values()
        .filter_map(|mut enum_def| {
            enum_def.schema = scoped_schema(
                dialect,
                EntityKind::Enum,
                enum_def.schema.as_deref(),
                schemas,
            )?;
            Some(enum_def)
        })
        .map(|enum_def| (enum_def.qualified_name(), enum_def))
        .collect();
}

fn scoped_schema(
    dialect: Dialect,
    kind: EntityKind,
    current: Option<&str>,
    schemas: &[&str],
) -> Option<Option<String>> {
    let Some(current) = current else {
        return Some(None);
    };
    if schemas.is_empty() {
        return Some(dialect.canonicalize_schema_name(kind, Some(current)));
    }

    let current = dialect.canonicalize_schema_name(kind, Some(current));
    schemas.iter().enumerate().find_map(|(index, requested)| {
        let requested = dialect.canonicalize_schema_name(kind, Some(requested));
        if current == requested {
            Some(if index == 0 { None } else { current.clone() })
        } else {
            None
        }
    })
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
    function.identity_key()
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
                            "    {}: expected {}, observed {}",
                            finding.property, finding.expected, finding.observed
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
    observed: String,
    note: Option<String>,
) -> DriftFinding {
    DriftFinding {
        operation,
        entity_kind,
        entity_name: entity_name.to_string(),
        property,
        expected,
        observed,
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

pub(crate) fn exact_string(expected: &str, observed: &str) -> PropertyMatch {
    if expected == observed {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_str(expected),
            observed: display_str(observed),
            note: None,
        }
    }
}

pub(crate) fn exact_bool(expected: bool, observed: bool) -> PropertyMatch {
    if expected == observed {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected.to_string(),
            observed: observed.to_string(),
            note: None,
        }
    }
}

pub(crate) fn exact_option(expected: &Option<String>, observed: &Option<String>) -> PropertyMatch {
    if expected == observed {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_option(expected),
            observed: display_option(observed),
            note: None,
        }
    }
}

pub(crate) fn exact_vec(expected: &[String], observed: &[String]) -> PropertyMatch {
    if expected == observed {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: display_vec(expected),
            observed: display_vec(observed),
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
    use crate::dialects::Dialect;
    use crate::states::{Column, FunctionDef, Schema, Table, Volatility};

    use super::{diff, diff_schemas, format_report};

    /// Verifies PostgreSQL literal casts do not create default drift.
    #[test]
    fn postgres_literal_text_cast_default_matches() {
        let replay = schema_with_column(defaulted_column("theme", Some("'light'")));
        let live = schema_with_column(defaulted_column("theme", Some("'light'::text")));

        let report = diff(replay, live, "public", Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {:?}", report);
    }

    /// Verifies real default drift includes the table, column, and property name.
    #[test]
    fn column_default_drift_reports_entity_property_and_values() {
        let replay = schema_with_column(defaulted_column("posts_per_page", None));
        let live = schema_with_column(defaulted_column("posts_per_page", Some("10")));

        let report = diff(replay, live, "public", Dialect::Postgres);
        let lines = format_report(&report);

        assert!(
            lines
                .iter()
                .any(|line| { line.contains("drift: alter_column preferences.posts_per_page") })
        );
        assert!(
            lines
                .iter()
                .any(|line| { line.contains("default: expected <none>, observed 10") })
        );
    }

    /// Verifies unquoted PostgreSQL default-expression case does not create drift.
    #[test]
    fn postgres_default_function_case_is_not_drift() {
        let replay = schema_with_column(defaulted_column("uploaded_at", Some("NOW()")));
        let live = schema_with_column(defaulted_column("uploaded_at", Some("now()")));

        let report = diff(replay, live, "public", Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {report:?}");
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

        let report = diff(replay, live, "public", Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {:?}", report);
    }

    /// Verifies additional explicitly qualified schemas participate in one drift comparison.
    #[test]
    fn multiple_schema_diff_compares_qualified_objects() {
        let replay = schema_with_namespaced_tables();
        let live = schema_with_namespaced_tables();

        let report = diff_schemas(replay, live, &["public", "billing"], Dialect::Postgres);

        assert!(report.findings.is_empty(), "unexpected drift: {report:?}");
    }

    /// Verifies a missing object in an additional schema is reported as drift.
    #[test]
    fn multiple_schema_diff_reports_missing_qualified_object() {
        let replay = schema_with_namespaced_tables();
        let mut live = schema_with_namespaced_tables();
        live.tables.remove("billing.invoices");

        let report = diff_schemas(replay, live, &["public", "billing"], Dialect::Postgres);

        assert!(
            report
                .findings
                .iter()
                .any(|finding| finding.entity_name == "billing.invoices")
        );
    }

    /// Verifies the drift contract documents registered comparator properties.
    #[test]
    fn drift_contract_lists_registered_column_default() {
        let docs = Dialect::Postgres.drift_contract();

        assert!(docs.iter().any(|doc| {
            doc.entity_kind == crate::states::types::EntityKind::Column
                && doc.property == "default"
                && !doc.compared.is_empty()
                && !doc.ignored.is_empty()
        }));
    }

    /// Verifies inspected-schema normalization is owned by the dialect lifecycle.
    #[test]
    fn normalize_inspected_schema_prepares_reflected_schema() {
        let inspected = schema_with_column(defaulted_column("theme", Some("'light'::text")));

        let normalized = Dialect::Postgres
            .normalize_inspected_schema(inspected)
            .expect("inspected schema should normalize");

        assert_eq!(
            normalized.tables["preferences"].columns[0]
                .default
                .as_deref(),
            Some("'light'::text")
        );
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
                options: Default::default(),
            },
        );
        schema
    }

    fn schema_with_namespaced_tables() -> Schema {
        let mut schema = Schema::default();
        let public = Table {
            name: "users".to_string(),
            schema: None,
            primary_key: None,
            columns: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
            triggers: Vec::new(),
            options: Default::default(),
        };
        let billing = Table {
            name: "invoices".to_string(),
            schema: Some("billing".to_string()),
            primary_key: None,
            columns: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
            triggers: Vec::new(),
            options: Default::default(),
        };
        schema.tables.insert("users".to_string(), public);
        schema
            .tables
            .insert("billing.invoices".to_string(), billing);
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
            generated_storage: None,
            dialect_options: Default::default(),
        }
    }

    fn function(body: &str) -> FunctionDef {
        FunctionDef {
            name: "audit_users".to_string(),
            schema: None,
            arguments: String::new(),
            parameters: Vec::new(),
            depends_on: Vec::new(),
            returns: "trigger".to_string(),
            language: "plpgsql".to_string(),
            body: body.to_string(),
            volatility: Volatility::Volatile,
            security_definer: false,
            opaque: Default::default(),
        }
    }
}
