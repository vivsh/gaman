use crate::parsers::tokens::{SqlToken, SqlTokenKind};
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
    let expected_type = ctx.dialect.type_comparison_key(&expected.col_type);
    let actual_type = ctx.dialect.type_comparison_key(&observed.col_type);
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

fn column_default(expected: &Column, observed: &Column, ctx: DriftContext<'_>) -> PropertyMatch {
    let expected_default = canonical_default(expected.default.as_deref());
    let actual_default = canonical_default(observed.default.as_deref());
    let equal = match (&expected_default, &actual_default) {
        (Some(expected), Some(actual)) => ctx.dialect.default_expressions_equal(expected, actual),
        (None, None) => true,
        _ => false,
    };
    if equal {
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

fn column_generated(
    expected: &Column,
    observed: &Column,
    context: DriftContext<'_>,
) -> PropertyMatch {
    match (&expected.generated, &observed.generated) {
        (Some(expected), Some(observed))
            if postgres_generated_expressions_equal(expected, observed, context) =>
        {
            PropertyMatch::Match
        }
        _ => exact_option(&expected.generated, &observed.generated),
    }
}

/// Compares generated expressions while ignoring PostgreSQL's implicit varchar-to-text casts.
fn postgres_generated_expressions_equal(
    expected: &str,
    observed: &str,
    context: DriftContext<'_>,
) -> bool {
    if context
        .dialect
        .default_expressions_equal(expected, observed)
    {
        return true;
    }

    let tokenizer = context.dialect.tokenizer();
    let Ok(expected) = tokenizer.tokenize(expected) else {
        return false;
    };
    let Ok(observed) = tokenizer.tokenize(observed) else {
        return false;
    };
    generated_expression_tokens(&expected) == generated_expression_tokens(&observed)
}

/// Produces comparison tokens after removing only catalog-added casts around identifiers.
fn generated_expression_tokens(tokens: &[SqlToken]) -> Vec<String> {
    let meaningful = tokens
        .iter()
        .filter(|token| !token.is_trivia())
        .collect::<Vec<_>>();
    let mut result = Vec::with_capacity(meaningful.len());
    let mut index = 0;
    while index < meaningful.len() {
        if let Some(identifier) = implicit_text_cast_identifier(&meaningful, index) {
            result.push(comparison_token(identifier));
            index += 5;
        } else {
            result.push(comparison_token(meaningful[index]));
            index += 1;
        }
    }
    result
}

/// Recognizes PostgreSQL's deparsed `(<identifier>)::text` coercion shape.
fn implicit_text_cast_identifier<'a>(
    tokens: &[&'a SqlToken],
    index: usize,
) -> Option<&'a SqlToken> {
    let candidate = tokens.get(index..index + 5)?;
    let identifier = candidate[1];
    let is_identifier = matches!(
        identifier.kind,
        SqlTokenKind::Word { .. } | SqlTokenKind::QuotedIdentifier { .. }
    );
    let is_text = candidate[4].canonical_word() == Some("TEXT");
    if matches!(candidate[0].kind, SqlTokenKind::LeftParen)
        && is_identifier
        && matches!(candidate[2].kind, SqlTokenKind::RightParen)
        && candidate[3].raw == "::"
        && is_text
    {
        Some(identifier)
    } else {
        None
    }
}

/// Converts one token into a stable comparison value without weakening protected literals.
fn comparison_token(token: &SqlToken) -> String {
    match &token.kind {
        SqlTokenKind::Word { canonical, .. } => format!("word:{canonical}"),
        SqlTokenKind::QuotedIdentifier { value, .. } => format!("identifier:{value}"),
        SqlTokenKind::String => format!("string:{}", token.raw),
        _ => format!("exact:{}", token.raw),
    }
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
    context: DriftContext<'_>,
) -> PropertyMatch {
    match (expected, observed) {
        (Constraint::Unique { columns: a, .. }, Constraint::Unique { columns: b, .. }) => {
            exact_vec(a, b)
        }
        (Constraint::Check { expression: a, .. }, Constraint::Check { expression: b, .. })
            if postgres_check_expressions_equal(a, b, context) =>
        {
            PropertyMatch::Match
        }
        (Constraint::Check { expression: a, .. }, Constraint::Check { expression: b, .. }) => {
            exact_string(a, b)
        }
        _ => PropertyMatch::Match,
    }
}

/// Compares checks while removing only catalog-added casts on numeric literals.
fn postgres_check_expressions_equal(
    expected: &str,
    observed: &str,
    context: DriftContext<'_>,
) -> bool {
    let tokenizer = context.dialect.tokenizer();
    let Ok(expected) = tokenizer.tokenize(expected) else {
        return false;
    };
    let Ok(observed) = tokenizer.tokenize(observed) else {
        return false;
    };
    check_expression_tokens(&expected) == check_expression_tokens(&observed)
}

/// Produces conservative check-expression tokens without numeric literal casts.
fn check_expression_tokens(tokens: &[SqlToken]) -> Vec<String> {
    let meaningful = tokens
        .iter()
        .filter(|token| !token.is_trivia())
        .collect::<Vec<_>>();
    let mut result = Vec::with_capacity(meaningful.len());
    let mut index = 0;
    while index < meaningful.len() {
        result.push(comparison_token(meaningful[index]));
        index += 1;
        if matches!(meaningful[index - 1].kind, SqlTokenKind::Number) {
            index += numeric_cast_token_count(&meaningful, index);
        }
    }
    result
}

/// Returns the number of safe numeric cast tokens following one numeric literal.
fn numeric_cast_token_count(tokens: &[&SqlToken], index: usize) -> usize {
    if tokens.get(index).is_none_or(|token| token.raw != "::") {
        return 0;
    }
    let first = tokens
        .get(index + 1)
        .and_then(|token| token.canonical_word());
    let second = tokens
        .get(index + 2)
        .and_then(|token| token.canonical_word());
    match (first, second) {
        (Some("DOUBLE"), Some("PRECISION")) => 3,
        (Some("NUMERIC" | "DECIMAL" | "SMALLINT" | "INTEGER" | "BIGINT" | "REAL"), _) => 2,
        _ => 0,
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

#[cfg(test)]
mod generated_expression_tests {
    use crate::parsers::tokens::{POSTGRES_TOKENIZER, SqlTokenizer};

    use super::{check_expression_tokens, generated_expression_tokens};

    /// PostgreSQL's catalog-added text coercion does not create generated-column drift.
    #[test]
    fn implicit_identifier_text_cast_is_ignored() {
        let expected = POSTGRES_TOKENIZER
            .tokenize("length(email)")
            .expect("expected expression should tokenize");
        let observed = POSTGRES_TOKENIZER
            .tokenize("length((email)::text)")
            .expect("observed expression should tokenize");

        assert_eq!(
            generated_expression_tokens(&expected),
            generated_expression_tokens(&observed)
        );
    }

    /// Explicit casts over compound expressions remain semantically significant.
    #[test]
    fn compound_expression_text_cast_is_preserved() {
        let expected = POSTGRES_TOKENIZER
            .tokenize("length(email || suffix)")
            .expect("expected expression should tokenize");
        let observed = POSTGRES_TOKENIZER
            .tokenize("length((email || suffix)::text)")
            .expect("observed expression should tokenize");

        assert_ne!(
            generated_expression_tokens(&expected),
            generated_expression_tokens(&observed)
        );
    }

    /// PostgreSQL numeric casts added while deparsing checks do not create drift.
    #[test]
    fn numeric_literal_casts_in_checks_are_ignored() {
        let expected = POSTGRES_TOKENIZER
            .tokenize("confidence >= 0 AND confidence <= 1")
            .expect("expected check should tokenize");
        let observed = POSTGRES_TOKENIZER
            .tokenize("confidence >= 0::double precision AND confidence <= 1::double precision")
            .expect("observed check should tokenize");
        assert_eq!(
            check_expression_tokens(&expected),
            check_expression_tokens(&observed)
        );
    }

    /// Cast removal remains limited to numeric literals and numeric target types.
    #[test]
    fn non_numeric_check_casts_remain_significant() {
        let plain = POSTGRES_TOKENIZER
            .tokenize("status = 1")
            .expect("plain check should tokenize");
        let text_cast = POSTGRES_TOKENIZER
            .tokenize("status = 1::text")
            .expect("cast check should tokenize");
        assert_ne!(
            check_expression_tokens(&plain),
            check_expression_tokens(&text_cast)
        );
    }
}
