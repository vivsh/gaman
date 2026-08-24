//! Shared semantic comparison for MySQL-family catalog expressions.

use crate::dialects::Dialect;
use crate::parsers::tokens::SqlTokenKind;
use crate::states::{Column, Constraint, GeneratedStorage};

use super::{DriftContext, PropertyMatch};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ExpressionToken {
    LeftParen,
    RightParen,
    Value(String),
}

#[derive(Debug, PartialEq, Eq)]
enum CanonicalExpression {
    Or(Vec<Self>),
    And(Vec<Self>),
    Tokens(Vec<ExpressionToken>),
}

/// Compares optional expressions after conservative MySQL-family token normalization.
pub(super) fn optional_expression(
    expected: &Option<String>,
    observed: &Option<String>,
    context: DriftContext<'_>,
) -> PropertyMatch {
    match (expected, observed) {
        (None, None) => PropertyMatch::Match,
        (Some(expected), Some(observed))
            if expressions_equal(&context.dialect, expected, observed) =>
        {
            PropertyMatch::Match
        }
        _ => PropertyMatch::Drift {
            expected: expected.as_deref().unwrap_or("<none>").to_string(),
            observed: observed.as_deref().unwrap_or("<none>").to_string(),
            note: None,
        },
    }
}

/// Compares generated expressions while tolerating server-added identifier quoting.
pub(super) fn generated(
    expected: &Column,
    observed: &Column,
    context: DriftContext<'_>,
) -> PropertyMatch {
    optional_expression(&expected.generated, &observed.generated, context)
}

/// Treats omitted generated storage as the family default of virtual storage.
pub(super) fn generated_storage(
    expected: &Column,
    observed: &Column,
    _: DriftContext<'_>,
) -> PropertyMatch {
    let expected = effective_storage(expected);
    let observed = effective_storage(observed);
    if expected == observed {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: format!("{expected:?}"),
            observed: format!("{observed:?}"),
            note: None,
        }
    }
}

/// Treats omitted and explicit `RESTRICT` actions as the same family default.
pub(super) fn foreign_key_action(
    expected: &Option<String>,
    observed: &Option<String>,
    _: DriftContext<'_>,
) -> PropertyMatch {
    if effective_foreign_key_action(expected) == effective_foreign_key_action(observed) {
        PropertyMatch::Match
    } else {
        PropertyMatch::Drift {
            expected: expected.as_deref().unwrap_or("<none>").to_string(),
            observed: observed.as_deref().unwrap_or("<none>").to_string(),
            note: None,
        }
    }
}

fn effective_foreign_key_action(value: &Option<String>) -> Option<&str> {
    match value.as_deref() {
        None | Some("restrict") => None,
        action => action,
    }
}

/// Compares modeled unique and check constraints using their stable semantics.
pub(super) fn constraint(
    expected: &Constraint,
    observed: &Constraint,
    context: DriftContext<'_>,
) -> PropertyMatch {
    match (expected, observed) {
        (Constraint::Unique { columns: a, .. }, Constraint::Unique { columns: b, .. }) => {
            if a == b {
                PropertyMatch::Match
            } else {
                PropertyMatch::Drift {
                    expected: format!("{a:?}"),
                    observed: format!("{b:?}"),
                    note: None,
                }
            }
        }
        (Constraint::Check { expression: a, .. }, Constraint::Check { expression: b, .. }) => {
            optional_expression(&Some(a.clone()), &Some(b.clone()), context)
        }
        (Constraint::Opaque { .. }, Constraint::Opaque { .. }) => PropertyMatch::Match,
        _ => PropertyMatch::Drift {
            expected: format!("{expected:?}"),
            observed: format!("{observed:?}"),
            note: None,
        },
    }
}

fn effective_storage(column: &Column) -> Option<GeneratedStorage> {
    match (&column.generated, column.generated_storage) {
        (Some(_), None) => Some(GeneratedStorage::Virtual),
        (_, storage) => storage,
    }
}

fn expressions_equal(dialect: &Dialect, left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (
        expression_tokens(dialect, left),
        expression_tokens(dialect, right),
    ) {
        (Ok(left), Ok(right)) => canonical_expression(&left) == canonical_expression(&right),
        _ => left == right,
    }
}

/// Preserves logical precedence while discarding redundant boolean grouping.
fn canonical_expression(tokens: &[ExpressionToken]) -> CanonicalExpression {
    let tokens = strip_outer_parentheses(tokens);
    if let Some(parts) = split_logical(tokens, "word:or") {
        return CanonicalExpression::Or(parts.into_iter().map(canonical_expression).collect());
    }
    if let Some(parts) = split_logical(tokens, "word:and") {
        return CanonicalExpression::And(parts.into_iter().map(canonical_expression).collect());
    }
    CanonicalExpression::Tokens(tokens.to_vec())
}

/// Splits one expression only at top-level logical operators.
fn split_logical<'a>(
    tokens: &'a [ExpressionToken],
    operator: &str,
) -> Option<Vec<&'a [ExpressionToken]>> {
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut parts = Vec::new();
    let mut between = false;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            ExpressionToken::LeftParen => depth += 1,
            ExpressionToken::RightParen => depth = depth.saturating_sub(1),
            ExpressionToken::Value(value) if depth == 0 && value == "word:between" => {
                between = true
            }
            ExpressionToken::Value(value) if depth == 0 && value == "word:and" && between => {
                between = false;
            }
            ExpressionToken::Value(value) if depth == 0 && value == operator => {
                parts.push(&tokens[start..index]);
                start = index + 1;
            }
            ExpressionToken::Value(_) => {}
        }
    }
    if parts.is_empty() {
        None
    } else {
        parts.push(&tokens[start..]);
        Some(parts)
    }
}

fn expression_tokens(
    dialect: &Dialect,
    source: &str,
) -> Result<Vec<ExpressionToken>, crate::parsers::tokens::TokenizeError> {
    let tokens = dialect
        .tokenizer()
        .tokenize(source)?
        .into_iter()
        .filter(|token| !token.is_trivia())
        .map(|token| {
            let value = match token.kind {
                SqlTokenKind::LeftParen => ExpressionToken::LeftParen,
                SqlTokenKind::RightParen => ExpressionToken::RightParen,
                SqlTokenKind::Word { canonical, .. } => {
                    let mut canonical = canonical.to_ascii_lowercase();
                    if matches!(dialect, Dialect::Mariadb) && canonical == "length" {
                        canonical = "octet_length".to_string();
                    }
                    ExpressionToken::Value(format!("word:{canonical}"))
                }
                SqlTokenKind::QuotedIdentifier { value, .. } => {
                    ExpressionToken::Value(format!("word:{}", value.to_ascii_lowercase()))
                }
                SqlTokenKind::String => ExpressionToken::Value(format!("string:{}", token.raw)),
                _ => ExpressionToken::Value(format!("exact:{}", token.raw)),
            };
            Ok(value)
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(normalize_empty_temporal_calls(tokens))
}

/// Removes optional empty call syntax from SQL temporal keywords.
fn normalize_empty_temporal_calls(tokens: Vec<ExpressionToken>) -> Vec<ExpressionToken> {
    let mut normalized = Vec::with_capacity(tokens.len());
    let mut index = 0;
    while index < tokens.len() {
        let temporal = matches!(tokens.get(index), Some(ExpressionToken::Value(value)) if matches!(value.as_str(), "word:current_timestamp" | "word:current_date" | "word:current_time" | "word:localtime" | "word:localtimestamp"));
        if temporal
            && tokens.get(index + 1) == Some(&ExpressionToken::LeftParen)
            && tokens.get(index + 2) == Some(&ExpressionToken::RightParen)
        {
            normalized.push(tokens[index].clone());
            index += 3;
        } else {
            normalized.push(tokens[index].clone());
            index += 1;
        }
    }
    normalized
}

fn strip_outer_parentheses(mut tokens: &[ExpressionToken]) -> &[ExpressionToken] {
    while wraps_expression(tokens) {
        tokens = &tokens[1..tokens.len() - 1];
    }
    tokens
}

fn wraps_expression(tokens: &[ExpressionToken]) -> bool {
    if tokens.len() < 2
        || tokens.first() != Some(&ExpressionToken::LeftParen)
        || tokens.last() != Some(&ExpressionToken::RightParen)
    {
        return false;
    }
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate() {
        match token {
            ExpressionToken::LeftParen => depth += 1,
            ExpressionToken::RightParen => {
                depth = depth.saturating_sub(1);
                if depth == 0 && index + 1 != tokens.len() {
                    return false;
                }
            }
            ExpressionToken::Value(_) => {}
        }
    }
    depth == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialects::Dialect;

    /// Verifies catalog quoting, case, trivia, and outer parentheses are semantically ignored.
    #[test]
    fn compares_family_catalog_expressions() {
        assert!(expressions_equal(
            &Dialect::Mysql,
            "length(email)",
            "( LENGTH(`email`) )"
        ));
        assert!(expressions_equal(
            &Dialect::Mysql,
            "CURRENT_TIMESTAMP(6)",
            "current_timestamp(6)"
        ));
        assert!(expressions_equal(
            &Dialect::Mysql,
            "confidence >= 0 AND confidence <= 1",
            "((`confidence` >= 0) and (`confidence` <= 1))"
        ));
    }

    /// Verifies removing catalog grouping never erases logical precedence.
    #[test]
    fn preserves_logical_grouping() {
        assert!(!expressions_equal(
            &Dialect::Mysql,
            "a AND (b OR c)",
            "(a AND b) OR c"
        ));
        assert!(expressions_equal(
            &Dialect::Mysql,
            "score BETWEEN 0 AND 1",
            "(score between 0 and 1)"
        ));
    }

    /// Verifies protected string values remain case- and content-sensitive.
    #[test]
    fn preserves_string_literal_semantics() {
        assert!(!expressions_equal(
            &Dialect::Mysql,
            "status = 'Open'",
            "status = 'open'"
        ));
    }

    /// Verifies MariaDB catalog aliases retain the same generated and temporal semantics.
    #[test]
    fn compares_mariadb_catalog_aliases() {
        assert!(expressions_equal(
            &Dialect::Mariadb,
            "length(email)",
            "octet_length(`email`)"
        ));
        assert!(expressions_equal(
            &Dialect::Mariadb,
            "CURRENT_TIMESTAMP",
            "current_timestamp()"
        ));
    }

    /// Verifies an omitted foreign-key action equals the family default of `RESTRICT`.
    #[test]
    fn compares_implicit_restrict_actions() {
        assert_eq!(effective_foreign_key_action(&None), None);
        assert_eq!(
            effective_foreign_key_action(&Some("restrict".to_string())),
            None
        );
        assert_eq!(
            effective_foreign_key_action(&Some("cascade".to_string())),
            Some("cascade")
        );
    }
}
