use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Table,
    TriggerDef, ViewDef,
};

use super::{
    ColumnProperty, ConstraintProperty, DriftContext, DriftRegistry, EnumProperty,
    ExtensionProperty, ForeignKeyProperty, FunctionProperty, IndexProperty, PrimaryKeyProperty,
    PropertyMatch, TableProperty, TriggerProperty, ViewProperty, exact_bool, exact_option,
    exact_string, exact_vec,
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

fn table_name(expected: &Table, observed: &Table, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn table_schema(expected: &Table, observed: &Table, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &observed.schema)
}

fn column_name(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn column_type(expected: &Column, observed: &Column, ctx: DriftContext<'_>) -> PropertyMatch {
    let expected_type = ctx.dialect.normalize_type(&expected.col_type).to_string();
    let actual_type = ctx.dialect.normalize_type(&observed.col_type).to_string();
    if expected_type == actual_type || serial_type_matches(&expected_type, &actual_type) {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected_type,
            observed: actual_type,
            note: None,
        }
    }
}

fn column_nullable(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_bool(expected.nullable, observed.nullable)
}

fn column_default(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    let expected_default = canonical_default(expected.default.as_deref());
    let actual_default = canonical_default(observed.default.as_deref());
    if expected_default == actual_default {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected_default.unwrap_or_else(|| "<none>".to_string()),
            observed: actual_default.unwrap_or_else(|| "<none>".to_string()),
            note: None,
        }
    }
}

fn column_reference(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    if expected.references == observed.references {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: format!("{:?}", expected.references),
            observed: format!("{:?}", observed.references),
            note: None,
        }
    }
}

fn column_check(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.check, &observed.check)
}

fn column_generated(expected: &Column, observed: &Column, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.generated, &observed.generated)
}

fn primary_key_name(
    expected: &PrimaryKey,
    observed: &PrimaryKey,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
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

fn constraint_kind(
    expected: &Constraint,
    observed: &Constraint,
    _: DriftContext<'_>,
) -> PropertyMatch {
    if std::mem::discriminant(expected) == std::mem::discriminant(observed) {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: constraint_kind_name(expected).to_string(),
            observed: constraint_kind_name(observed).to_string(),
            note: None,
        }
    }
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
        _ => PropertyMatch::Match,
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

fn trigger_function(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.function_name, &observed.function_name)
}

fn trigger_language(
    expected: &TriggerDef,
    observed: &TriggerDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    match expected.language.as_deref() {
        None => PropertyMatch::Match,
        Some(_) => exact_option(&expected.language, &observed.language),
    }
}

fn function_name(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn function_schema(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.schema, &observed.schema)
}

fn function_arguments(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.arguments, &observed.arguments)
}

fn function_returns(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.returns, &observed.returns)
}

fn function_language(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.language, &observed.language)
}

fn function_volatility(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(
        &format!("{:?}", expected.volatility),
        &format!("{:?}", observed.volatility),
    )
}

fn function_security(
    expected: &FunctionDef,
    observed: &FunctionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_bool(expected.security_definer, observed.security_definer)
}

fn view_name(expected: &ViewDef, observed: &ViewDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn view_schema(expected: &ViewDef, observed: &ViewDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &observed.schema)
}

fn enum_name(expected: &EnumDef, observed: &EnumDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn enum_schema(expected: &EnumDef, observed: &EnumDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_option(&expected.schema, &observed.schema)
}

fn enum_values(expected: &EnumDef, observed: &EnumDef, _: DriftContext<'_>) -> PropertyMatch {
    exact_vec(&expected.values, &observed.values)
}

fn extension_name(
    expected: &ExtensionDef,
    observed: &ExtensionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_string(&expected.name, &observed.name)
}

fn extension_schema(
    expected: &ExtensionDef,
    observed: &ExtensionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    exact_option(&expected.schema, &observed.schema)
}

fn extension_version(
    expected: &ExtensionDef,
    observed: &ExtensionDef,
    _: DriftContext<'_>,
) -> PropertyMatch {
    match expected.version.as_deref() {
        None => PropertyMatch::Match,
        Some(_) => exact_option(&expected.version, &observed.version),
    }
}

fn serial_type_matches(expected: &str, observed: &str) -> bool {
    matches!(
        (expected, observed),
        ("serial", "integer") | ("bigserial", "bigint") | ("smallserial", "smallint")
    )
}

fn canonical_default(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let value = strip_wrapping_parens(value);
    if let Some(numeric) = numeric_literal_cast(value) {
        return Some(numeric.to_string());
    }
    let value = strip_literal_cast(value);
    if value.eq_ignore_ascii_case("true") {
        return Some("true".to_string());
    }
    if value.eq_ignore_ascii_case("false") {
        return Some("false".to_string());
    }
    Some(value.to_string())
}

/// Extracts a catalog-quoted numeric literal only when both its cast and token shape are safe.
fn numeric_literal_cast(value: &str) -> Option<&str> {
    let (literal, cast) = value.rsplit_once("::")?;
    let cast = cast.trim_matches('"').to_ascii_lowercase();
    if !matches!(
        cast.as_str(),
        "numeric" | "decimal" | "smallint" | "integer" | "bigint" | "real" | "double precision"
    ) {
        return None;
    }
    let literal = literal.strip_prefix('\'')?.strip_suffix('\'')?;
    is_numeric_literal(literal).then_some(literal)
}

/// Recognizes a decimal or exponent-form SQL numeric token without evaluating its value.
fn is_numeric_literal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let mut digits = 0usize;
    while matches!(bytes.get(index), Some(b'0'..=b'9')) {
        digits += 1;
        index += 1;
    }
    if matches!(bytes.get(index), Some(b'.')) {
        index += 1;
        while matches!(bytes.get(index), Some(b'0'..=b'9')) {
            digits += 1;
            index += 1;
        }
    }
    if digits == 0 {
        return false;
    }
    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while matches!(bytes.get(index), Some(b'0'..=b'9')) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }
    index == bytes.len()
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
        Constraint::Opaque { .. } => "opaque",
    }
}
