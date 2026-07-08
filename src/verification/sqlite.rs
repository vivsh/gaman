use gaman_core::states::{
    Column, Constraint, ForeignKey, Index, PrimaryKey, Table, TriggerDef, ViewDef,
};

use super::{
    ColumnProperty, ConstraintProperty, ForeignKeyProperty, IndexProperty, PrimaryKeyProperty,
    PropertyMatch, TableProperty, TriggerProperty, VerificationContext, VerificationRegistry,
    ViewProperty, exact_bool, exact_option, exact_string, exact_vec,
};

pub(crate) fn registry() -> &'static VerificationRegistry {
    &REGISTRY
}

static REGISTRY: VerificationRegistry = VerificationRegistry {
    tables: TABLES,
    columns: COLUMNS,
    primary_keys: PRIMARY_KEYS,
    foreign_keys: FOREIGN_KEYS,
    indexes: INDEXES,
    constraints: CONSTRAINTS,
    triggers: TRIGGERS,
    functions: &[],
    views: VIEWS,
    enums: &[],
    extensions: &[],
};

static TABLES: &[TableProperty] = &[TableProperty {
    name: "name",
    compare: table_name,
}];

static COLUMNS: &[ColumnProperty] = &[
    ColumnProperty {
        name: "name",
        compare: column_name,
    },
    ColumnProperty {
        name: "type",
        compare: sqlite_column_type,
    },
    ColumnProperty {
        name: "nullable",
        compare: column_nullable,
    },
    ColumnProperty {
        name: "default",
        compare: column_default,
    },
    ColumnProperty {
        name: "generated",
        compare: column_generated,
    },
];

static PRIMARY_KEYS: &[PrimaryKeyProperty] = &[PrimaryKeyProperty {
    name: "columns",
    compare: primary_key_columns,
}];

static FOREIGN_KEYS: &[ForeignKeyProperty] = &[
    ForeignKeyProperty {
        name: "name",
        compare: foreign_key_name,
    },
    ForeignKeyProperty {
        name: "columns",
        compare: foreign_key_columns,
    },
    ForeignKeyProperty {
        name: "to_table",
        compare: foreign_key_table,
    },
    ForeignKeyProperty {
        name: "to_columns",
        compare: foreign_key_to_columns,
    },
    ForeignKeyProperty {
        name: "on_delete",
        compare: foreign_key_on_delete,
    },
];

static INDEXES: &[IndexProperty] = &[
    IndexProperty {
        name: "name",
        compare: index_name,
    },
    IndexProperty {
        name: "columns",
        compare: index_columns,
    },
    IndexProperty {
        name: "unique",
        compare: index_unique,
    },
    IndexProperty {
        name: "predicate",
        compare: index_predicate,
    },
];

static CONSTRAINTS: &[ConstraintProperty] = &[ConstraintProperty {
    name: "definition",
    compare: constraint_definition,
}];

static TRIGGERS: &[TriggerProperty] = &[
    TriggerProperty {
        name: "name",
        compare: trigger_name,
    },
    TriggerProperty {
        name: "timing",
        compare: trigger_timing,
    },
    TriggerProperty {
        name: "events",
        compare: trigger_events,
    },
    TriggerProperty {
        name: "scope",
        compare: trigger_scope,
    },
];

static VIEWS: &[ViewProperty] = &[ViewProperty {
    name: "name",
    compare: view_name,
}];

fn table_name(expected: &Table, actual: &Table, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn column_name(expected: &Column, actual: &Column, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn sqlite_column_type(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(
        &expected.col_type.to_ascii_lowercase(),
        &actual.col_type.to_ascii_lowercase(),
    )
}

fn column_nullable(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_bool(expected.nullable, actual.nullable)
}

fn column_default(expected: &Column, actual: &Column, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.default, &actual.default)
}

fn column_generated(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.generated, &actual.generated)
}

fn primary_key_columns(
    expected: &PrimaryKey,
    actual: &PrimaryKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.columns, &actual.columns)
}

fn foreign_key_name(
    expected: &ForeignKey,
    actual: &ForeignKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn foreign_key_columns(
    expected: &ForeignKey,
    actual: &ForeignKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.columns, &actual.columns)
}

fn foreign_key_table(
    expected: &ForeignKey,
    actual: &ForeignKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.to_table, &actual.to_table)
}

fn foreign_key_to_columns(
    expected: &ForeignKey,
    actual: &ForeignKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.to_columns, &actual.to_columns)
}

fn foreign_key_on_delete(
    expected: &ForeignKey,
    actual: &ForeignKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.on_delete, &actual.on_delete)
}

fn index_name(expected: &Index, actual: &Index, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn index_columns(expected: &Index, actual: &Index, _: VerificationContext<'_>) -> PropertyMatch {
    exact_vec(&expected.columns, &actual.columns)
}

fn index_unique(expected: &Index, actual: &Index, _: VerificationContext<'_>) -> PropertyMatch {
    exact_bool(expected.unique, actual.unique)
}

fn index_predicate(expected: &Index, actual: &Index, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.predicate, &actual.predicate)
}

fn constraint_definition(
    expected: &Constraint,
    actual: &Constraint,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    match (expected, actual) {
        (Constraint::Unique { columns: a, .. }, Constraint::Unique { columns: b, .. }) => {
            exact_vec(a, b)
        }
        (Constraint::Check { expression: a, .. }, Constraint::Check { expression: b, .. }) => {
            exact_string(a, b)
        }
        _ => PropertyMatch::Drift {
            expected: format!("{expected:?}"),
            actual: format!("{actual:?}"),
            note: None,
        },
    }
}

fn trigger_name(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.name, &actual.name)
}

fn trigger_timing(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.timing),
        &format!("{:?}", actual.timing),
    )
}

fn trigger_events(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    if expected.events == actual.events {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: format!("{:?}", expected.events),
            actual: format!("{:?}", actual.events),
            note: None,
        }
    }
}

fn trigger_scope(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.scope),
        &format!("{:?}", actual.scope),
    )
}

fn view_name(expected: &ViewDef, actual: &ViewDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}
