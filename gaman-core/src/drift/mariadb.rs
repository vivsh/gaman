//! MariaDB semantic drift contract.

use super::{
    ColumnProperty, ConstraintProperty, DriftContext, DriftRegistry, ForeignKeyProperty,
    IndexProperty, PrimaryKeyProperty, PropertyMatch, TableProperty, exact_bool, exact_option,
    exact_string, exact_vec,
};
use crate::states::{Column, Constraint};

use super::mysql_family;

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
    triggers: &[],
    functions: &[],
    views: &[],
    enums: &[],
    extensions: &[],
};
static TABLES: &[TableProperty] = &[TableProperty {
    name: "name",
    compare: |a, b, _| exact_string(&a.name, &b.name),
}];
static COLUMNS: &[ColumnProperty] = &[
    ColumnProperty {
        name: "name",
        compare: |a, b, _| exact_string(&a.name, &b.name),
    },
    ColumnProperty {
        name: "type",
        compare: column_type,
    },
    ColumnProperty {
        name: "nullable",
        compare: |a, b, _| exact_bool(a.nullable, b.nullable),
    },
    ColumnProperty {
        name: "default",
        compare: column_default,
    },
    ColumnProperty {
        name: "generated",
        compare: mysql_family::generated,
    },
    ColumnProperty {
        name: "generated_storage",
        compare: mysql_family::generated_storage,
    },
    ColumnProperty {
        name: "auto_increment",
        compare: |a, b, _| exact_bool(mariadb(a).auto_increment, mariadb(b).auto_increment),
    },
    ColumnProperty {
        name: "on_update_expression",
        compare: |a, b, context| {
            mysql_family::optional_expression(
                &mariadb(a).on_update_expression,
                &mariadb(b).on_update_expression,
                context,
            )
        },
    },
    ColumnProperty {
        name: "character_set",
        compare: optional_pin_charset,
    },
    ColumnProperty {
        name: "collation",
        compare: optional_pin_collation,
    },
    ColumnProperty {
        name: "invisible",
        compare: |a, b, _| exact_bool(mariadb(a).invisible, mariadb(b).invisible),
    },
    ColumnProperty {
        name: "comment",
        compare: comment,
    },
];
static PRIMARY_KEYS: &[PrimaryKeyProperty] = &[PrimaryKeyProperty {
    name: "columns",
    compare: |a, b, _| exact_vec(&a.columns, &b.columns),
}];
static FOREIGN_KEYS: &[ForeignKeyProperty] = &[
    ForeignKeyProperty {
        name: "name",
        compare: |a, b, _| exact_string(&a.name, &b.name),
    },
    ForeignKeyProperty {
        name: "columns",
        compare: |a, b, _| exact_vec(&a.columns, &b.columns),
    },
    ForeignKeyProperty {
        name: "to_table",
        compare: |a, b, _| exact_string(&a.to_table, &b.to_table),
    },
    ForeignKeyProperty {
        name: "to_columns",
        compare: |a, b, _| exact_vec(&a.to_columns, &b.to_columns),
    },
    ForeignKeyProperty {
        name: "on_delete",
        compare: |a, b, _| exact_option(&a.on_delete, &b.on_delete),
    },
    ForeignKeyProperty {
        name: "on_update",
        compare: |a, b, _| exact_option(&a.on_update, &b.on_update),
    },
];
static INDEXES: &[IndexProperty] = &[
    IndexProperty {
        name: "name",
        compare: |a, b, _| exact_string(&a.name, &b.name),
    },
    IndexProperty {
        name: "columns",
        compare: |a, b, _| exact_vec(&a.columns, &b.columns),
    },
    IndexProperty {
        name: "unique",
        compare: |a, b, _| exact_bool(a.unique, b.unique),
    },
];
static CONSTRAINTS: &[ConstraintProperty] = &[ConstraintProperty {
    name: "definition",
    compare: constraint,
}];

fn column_type(a: &Column, b: &Column, ctx: DriftContext<'_>) -> PropertyMatch {
    exact_string(
        &ctx.dialect.type_comparison_key(&a.col_type),
        &ctx.dialect.type_comparison_key(&b.col_type),
    )
}
fn column_default(a: &Column, b: &Column, ctx: DriftContext<'_>) -> PropertyMatch {
    match (&a.default, &b.default) {
        (Some(a), Some(b)) if ctx.dialect.default_expressions_equal(a, b) => PropertyMatch::Match,
        _ => exact_option(&a.default, &b.default),
    }
}
fn optional_pin_charset(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    if mariadb(a).character_set.is_none() {
        PropertyMatch::Match
    } else {
        exact_option(&mariadb(a).character_set, &mariadb(b).character_set)
    }
}
fn optional_pin_collation(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    if mariadb(a).collation.is_none() {
        PropertyMatch::Match
    } else {
        exact_option(&mariadb(a).collation, &mariadb(b).collation)
    }
}
fn comment(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(
        mariadb(a).comment.as_deref().unwrap_or(""),
        mariadb(b).comment.as_deref().unwrap_or(""),
    )
}

/// Returns explicit MariaDB options or an immutable empty option set.
fn mariadb(column: &Column) -> &crate::states::MariadbColumnOptions {
    static EMPTY: crate::states::MariadbColumnOptions = crate::states::MariadbColumnOptions {
        auto_increment: false,
        on_update_expression: None,
        character_set: None,
        collation: None,
        invisible: false,
        comment: None,
    };
    column.dialect_options.mariadb().unwrap_or(&EMPTY)
}
fn constraint(a: &Constraint, b: &Constraint, context: DriftContext<'_>) -> PropertyMatch {
    mysql_family::constraint(a, b, context)
}
