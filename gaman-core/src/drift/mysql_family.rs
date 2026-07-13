//! Shared semantic comparison for MySQL-family catalog expressions.

use crate::parsers::tokens::{SqlTokenKind, SqlTokenizer};
use crate::states::{Column, Constraint, GeneratedStorage};

use super::{DriftContext, PropertyMatch};

#[derive(Debug, PartialEq, Eq)]
enum ExpressionToken {
    LeftParen,
    RightParen,
    Value(String),
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
            if expressions_equal(context.dialect.tokenizer(), expected, observed) =>
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

fn expressions_equal(tokenizer: &dyn SqlTokenizer, left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (
        expression_tokens(tokenizer, left),
        expression_tokens(tokenizer, right),
    ) {
        (Ok(left), Ok(right)) => strip_outer_parentheses(&left) == strip_outer_parentheses(&right),
        _ => left == right,
    }
}

fn expression_tokens(
    tokenizer: &dyn SqlTokenizer,
    source: &str,
) -> Result<Vec<ExpressionToken>, crate::parsers::tokens::TokenizeError> {
    tokenizer
        .tokenize(source)?
        .into_iter()
        .filter(|token| !token.is_trivia())
        .map(|token| {
            let value = match token.kind {
                SqlTokenKind::LeftParen => ExpressionToken::LeftParen,
                SqlTokenKind::RightParen => ExpressionToken::RightParen,
                SqlTokenKind::Word { canonical, .. } => {
                    ExpressionToken::Value(format!("word:{}", canonical.to_ascii_lowercase()))
                }
                SqlTokenKind::QuotedIdentifier { value, .. } => {
                    ExpressionToken::Value(format!("word:{}", value.to_ascii_lowercase()))
                }
                SqlTokenKind::String => ExpressionToken::Value(format!("string:{}", token.raw)),
                _ => ExpressionToken::Value(format!("exact:{}", token.raw)),
            };
            Ok(value)
        })
        .collect()
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
    use crate::parsers::tokens::MYSQL_TOKENIZER;

    /// Verifies catalog quoting, case, trivia, and outer parentheses are semantically ignored.
    #[test]
    fn compares_family_catalog_expressions() {
        assert!(expressions_equal(
            &MYSQL_TOKENIZER,
            "length(email)",
            "( LENGTH(`email`) )"
        ));
        assert!(expressions_equal(
            &MYSQL_TOKENIZER,
            "CURRENT_TIMESTAMP(6)",
            "current_timestamp(6)"
        ));
    }

    /// Verifies protected string values remain case- and content-sensitive.
    #[test]
    fn preserves_string_literal_semantics() {
        assert!(!expressions_equal(
            &MYSQL_TOKENIZER,
            "status = 'Open'",
            "status = 'open'"
        ));
    }
}
