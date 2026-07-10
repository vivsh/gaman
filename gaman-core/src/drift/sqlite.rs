use crate::states::{
    Column, Constraint, ForeignKey, Index, PrimaryKey, Table, TriggerDef, ViewDef,
};

use super::{
    ColumnProperty, ConstraintProperty, DriftContext, DriftRegistry, ForeignKeyProperty,
    IndexProperty, PrimaryKeyProperty, PropertyMatch, TableProperty, TriggerProperty, ViewProperty,
    exact_bool, exact_option, exact_string, exact_vec,
};

pub(crate) fn registry() -> &'static DriftRegistry {
    &REGISTRY
}

static REGISTRY: DriftRegistry = DriftRegistry {
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
    ForeignKeyProperty {
        name: "on_update",
        compare: foreign_key_on_update,
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

fn table_name(expected: &Table, observed: &Table, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn column_name(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn sqlite_column_type(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(
        &expected.col_type.to_ascii_lowercase(),
        &observed.col_type.to_ascii_lowercase(),
    )
}

fn column_nullable(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_bool(expected.nullable, observed.nullable)
}

fn column_default(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.default, &observed.default)
}

fn column_generated(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.generated, &observed.generated)
}

fn primary_key_columns(
    expected: &PrimaryKey,
    observed: &PrimaryKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.columns, &observed.columns)
}

fn foreign_key_name(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn foreign_key_columns(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.columns, &observed.columns)
}

fn foreign_key_table(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.to_table, &observed.to_table)
}

fn foreign_key_to_columns(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_vec(&expected.to_columns, &observed.to_columns)
}

fn foreign_key_on_delete(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.on_delete, &observed.on_delete)
}

fn foreign_key_on_update(
    expected: &ForeignKey,
    observed: &ForeignKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.on_update, &observed.on_update)
}

fn index_name(expected: &Index, observed: &Index, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn index_columns(expected: &Index, observed: &Index, _: DriftContext<'_>) -> PropertyMatch {
    exact_vec(&expected.columns, &observed.columns)
}

fn index_unique(expected: &Index, observed: &Index, _: DriftContext<'_>) -> PropertyMatch {
    exact_bool(expected.unique, observed.unique)
}

fn index_predicate(expected: &Index, observed: &Index, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.predicate, &observed.predicate)
}

fn constraint_definition(
    expected: &Constraint,
    observed: &Constraint,
    _: DriftContext<'_>,
) -> PropertyMatch {
    match (expected, observed) {
        (Constraint::Unique { columns: a, .. }, Constraint::Unique { columns: b, .. }) => {
            exact_vec(a, b)
        }
        (Constraint::Check { expression: a, .. }, Constraint::Check { expression: b, .. }) => {
            exact_string(a, b)
        }
        (Constraint::Opaque { .. }, Constraint::Opaque { .. }) => PropertyMatch::Match,
        _ => PropertyMatch::Drift {
            expected: format!("{expected:?}"),
            observed: format!("{observed:?}"),
            note: None,
        },
    }
}

fn trigger_name(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.name, &observed.name)
}

fn trigger_timing(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.timing),
        &format!("{:?}", observed.timing),
    )
}

fn trigger_events(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    if expected.events == observed.events {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: format!("{:?}", expected.events),
            observed: format!("{:?}", observed.events),
            note: None,
        }
    }
}

fn trigger_scope(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.scope),
        &format!("{:?}", observed.scope),
    )
}

fn view_name(expected: &ViewDef, observed: &ViewDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}
