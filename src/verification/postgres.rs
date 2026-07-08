use gaman_core::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Table,
    TriggerDef, ViewDef,
};

use super::{
    ColumnProperty, ConstraintProperty, EnumProperty, ExtensionProperty, ForeignKeyProperty,
    FunctionProperty, IndexProperty, PrimaryKeyProperty, PropertyMatch, TableProperty,
    TriggerProperty, VerificationContext, VerificationRegistry, ViewProperty, exact_bool,
    exact_option, exact_string, exact_vec,
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
    functions: FUNCTIONS,
    views: VIEWS,
    enums: ENUMS,
    extensions: EXTENSIONS,
};

static TABLES: &[TableProperty] = &[
    TableProperty {
        name: "name",
        compare: table_name,
    },
    TableProperty {
        name: "schema",
        compare: table_schema,
    },
];

static COLUMNS: &[ColumnProperty] = &[
    ColumnProperty {
        name: "name",
        compare: column_name,
    },
    ColumnProperty {
        name: "type",
        compare: column_type,
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
        name: "references",
        compare: column_reference,
    },
    ColumnProperty {
        name: "check",
        compare: column_check,
    },
    ColumnProperty {
        name: "generated",
        compare: column_generated,
    },
];

static PRIMARY_KEYS: &[PrimaryKeyProperty] = &[
    PrimaryKeyProperty {
        name: "name",
        compare: primary_key_name,
    },
    PrimaryKeyProperty {
        name: "columns",
        compare: primary_key_columns,
    },
];

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

static CONSTRAINTS: &[ConstraintProperty] = &[
    ConstraintProperty {
        name: "kind",
        compare: constraint_kind,
    },
    ConstraintProperty {
        name: "definition",
        compare: constraint_definition,
    },
];

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
    TriggerProperty {
        name: "function_name",
        compare: trigger_function,
    },
    TriggerProperty {
        name: "language",
        compare: trigger_language,
    },
];

static FUNCTIONS: &[FunctionProperty] = &[
    FunctionProperty {
        name: "name",
        compare: function_name,
    },
    FunctionProperty {
        name: "schema",
        compare: function_schema,
    },
    FunctionProperty {
        name: "arguments",
        compare: function_arguments,
    },
    FunctionProperty {
        name: "returns",
        compare: function_returns,
    },
    FunctionProperty {
        name: "language",
        compare: function_language,
    },
    FunctionProperty {
        name: "volatility",
        compare: function_volatility,
    },
    FunctionProperty {
        name: "security_definer",
        compare: function_security,
    },
];

static VIEWS: &[ViewProperty] = &[
    ViewProperty {
        name: "name",
        compare: view_name,
    },
    ViewProperty {
        name: "schema",
        compare: view_schema,
    },
];

static ENUMS: &[EnumProperty] = &[
    EnumProperty {
        name: "name",
        compare: enum_name,
    },
    EnumProperty {
        name: "schema",
        compare: enum_schema,
    },
    EnumProperty {
        name: "values",
        compare: enum_values,
    },
];

static EXTENSIONS: &[ExtensionProperty] = &[
    ExtensionProperty {
        name: "name",
        compare: extension_name,
    },
    ExtensionProperty {
        name: "schema",
        compare: extension_schema,
    },
    ExtensionProperty {
        name: "version",
        compare: extension_version,
    },
];

fn table_name(expected: &Table, actual: &Table, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn table_schema(expected: &Table, actual: &Table, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &actual.schema)
}

fn column_name(expected: &Column, actual: &Column, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn column_type(expected: &Column, actual: &Column, ctx: VerificationContext<'_>) -> PropertyMatch {
    let expected_type = ctx.dialect.normalize_type(&expected.col_type).to_string();
    let actual_type = ctx.dialect.normalize_type(&actual.col_type).to_string();
    if expected_type == actual_type || serial_type_matches(&expected_type, &actual_type) {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected_type,
            actual: actual_type,
            note: None,
        }
    }
}

fn column_nullable(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_bool(expected.nullable, actual.nullable)
}

fn column_default(expected: &Column, actual: &Column, _: VerificationContext<'_>) -> PropertyMatch {
    let expected_default = canonical_default(expected.default.as_deref());
    let actual_default = canonical_default(actual.default.as_deref());
    if expected_default == actual_default {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected_default.unwrap_or_else(|| "<none>".to_string()),
            actual: actual_default.unwrap_or_else(|| "<none>".to_string()),
            note: None,
        }
    }
}

fn column_reference(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    if expected.references == actual.references {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: format!("{:?}", expected.references),
            actual: format!("{:?}", actual.references),
            note: None,
        }
    }
}

fn column_check(expected: &Column, actual: &Column, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.check, &actual.check)
}

fn column_generated(
    expected: &Column,
    actual: &Column,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.generated, &actual.generated)
}

fn primary_key_name(
    expected: &PrimaryKey,
    actual: &PrimaryKey,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
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

fn constraint_kind(
    expected: &Constraint,
    actual: &Constraint,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    if std::mem::discriminant(expected) == std::mem::discriminant(actual) {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: constraint_kind_name(expected).to_string(),
            actual: constraint_kind_name(actual).to_string(),
            note: None,
        }
    }
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
        _ => PropertyMatch::Match,
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

fn trigger_function(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.function_name, &actual.function_name)
}

fn trigger_language(
    expected: &TriggerDef,
    actual: &TriggerDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    match expected.language.as_deref() {
        None => PropertyMatch::Match,
        Some(_) => exact_option(&expected.language, &actual.language),
    }
}

fn function_name(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn function_schema(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.schema, &actual.schema)
}

fn function_arguments(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.arguments, &actual.arguments)
}

fn function_returns(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.returns, &actual.returns)
}

fn function_language(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.language, &actual.language)
}

fn function_volatility(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.volatility),
        &format!("{:?}", actual.volatility),
    )
}

fn function_security(
    expected: &FunctionDef,
    actual: &FunctionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_bool(expected.security_definer, actual.security_definer)
}

fn view_name(expected: &ViewDef, actual: &ViewDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn view_schema(expected: &ViewDef, actual: &ViewDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &actual.schema)
}

fn enum_name(expected: &EnumDef, actual: &EnumDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn enum_schema(expected: &EnumDef, actual: &EnumDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &actual.schema)
}

fn enum_values(expected: &EnumDef, actual: &EnumDef, _: VerificationContext<'_>) -> PropertyMatch {
    exact_vec(&expected.values, &actual.values)
}

fn extension_name(
    expected: &ExtensionDef,
    actual: &ExtensionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &actual.name)
}

fn extension_schema(
    expected: &ExtensionDef,
    actual: &ExtensionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.schema, &actual.schema)
}

fn extension_version(
    expected: &ExtensionDef,
    actual: &ExtensionDef,
    _: VerificationContext<'_>,
) -> PropertyMatch {
    match expected.version.as_deref() {
        None => PropertyMatch::Match,
        Some(_) => exact_option(&expected.version, &actual.version),
    }
}

fn serial_type_matches(expected: &str, actual: &str) -> bool {
    matches!(
        (expected, actual),
        ("serial", "integer") | ("bigserial", "bigint") | ("smallserial", "smallint")
    )
}

fn canonical_default(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let value = strip_wrapping_parens(value);
    let value = strip_literal_cast(value);
    if value.eq_ignore_ascii_case("true") {
        return Some("true".to_string());
    }
    if value.eq_ignore_ascii_case("false") {
        return Some("false".to_string());
    }
    Some(value.to_string())
}

pub(crate) fn canonical_column_default(value: Option<&str>) -> Option<String> {
    canonical_default(value)
}

fn strip_wrapping_parens(value: &str) -> &str {
    let mut current = value.trim();
    while current.starts_with('(') && current.ends_with(')') {
        current = current[1..current.len() - 1].trim();
    }
    current
}

fn strip_literal_cast(value: &str) -> &str {
    let Some((literal, cast)) = value.rsplit_once("::") else {
        return value;
    };
    if literal.starts_with('\'') && safe_literal_cast(cast) {
        literal
    } else {
        value
    }
}

fn safe_literal_cast(cast: &str) -> bool {
    matches!(
        cast.trim_matches('"').to_ascii_lowercase().as_str(),
        "text" | "varchar" | "character varying" | "bpchar" | "char" | "character"
    )
}

fn constraint_kind_name(constraint: &Constraint) -> &'static str {
    match constraint {
        Constraint::Unique { .. } => "unique",
        Constraint::Check { .. } => "check",
    }
}
