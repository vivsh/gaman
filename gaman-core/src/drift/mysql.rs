//! MySQL semantic drift contract.

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
        compare: |a, b, _| exact_bool(mysql(a).auto_increment, mysql(b).auto_increment),
    },
    ColumnProperty {
        name: "on_update_expression",
        compare: |a, b, context| {
            mysql_family::optional_expression(
                &mysql(a).on_update_expression,
                &mysql(b).on_update_expression,
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
        compare: |a, b, _| exact_bool(mysql(a).invisible, mysql(b).invisible),
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
        compare: |a, b, context| {
            mysql_family::foreign_key_action(&a.on_delete, &b.on_delete, context)
        },
    },
    ForeignKeyProperty {
        name: "on_update",
        compare: |a, b, context| {
            mysql_family::foreign_key_action(&a.on_update, &b.on_update, context)
        },
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
    mysql_family::optional_expression(&a.default, &b.default, ctx)
}
fn optional_pin_charset(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    if mysql(a).character_set.is_none() {
        PropertyMatch::Match
    } else {
        exact_option(&mysql(a).character_set, &mysql(b).character_set)
    }
}
fn optional_pin_collation(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    if mysql(a).collation.is_none() {
        PropertyMatch::Match
    } else {
        exact_option(&mysql(a).collation, &mysql(b).collation)
    }
}
fn comment(a: &Column, b: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(
        mysql(a).comment.as_deref().unwrap_or(""),
        mysql(b).comment.as_deref().unwrap_or(""),
    )
}

/// Returns explicit MySQL options or an immutable empty option set.
fn mysql(column: &Column) -> &crate::states::MysqlColumnOptions {
    static EMPTY: crate::states::MysqlColumnOptions = crate::states::MysqlColumnOptions {
        auto_increment: false,
        on_update_expression: None,
        character_set: None,
        collation: None,
        invisible: false,
        comment: None,
    };
    column.dialect_options.mysql().unwrap_or(&EMPTY)
}
fn constraint(a: &Constraint, b: &Constraint, context: DriftContext<'_>) -> PropertyMatch {
    mysql_family::constraint(a, b, context)
}
