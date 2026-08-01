//! Token-based MySQL 8.4 native type comparison.

use crate::dialects::Dialect;

/// Produces a stable comparison key while preserving quoted enum and set labels.
pub(in crate::dialects) fn key(value: &str) -> String {
    token_key(value, Dialect::Mysql, true)
}

/// Preserves authored type spelling while removing insignificant outer whitespace.
pub(super) fn canonical(value: &str) -> String {
    value.trim().to_string()
}

pub(crate) fn token_key(value: &str, dialect: Dialect, mysql_json: bool) -> String {
    let Ok(tokens) = dialect.tokenizer().tokenize(value) else {
        return value.trim().to_ascii_lowercase();
    };
    let mut parts = tokens
        .iter()
        .filter(|token| !token.is_trivia())
        .map(|token| {
            token
                .canonical_word()
                .map(str::to_ascii_lowercase)
                .unwrap_or_else(|| value[token.span.clone()].to_string())
        })
        .collect::<Vec<_>>();
    if let Some(first) = parts.first_mut() {
        *first = match first.as_str() {
            "integer" => "int".to_string(),
            "bool" | "boolean" => "tinyint".to_string(),
            "numeric" => "decimal".to_string(),
            "serial" => "bigint".to_string(),
            "json" if mysql_json => "json".to_string(),
            other => other.to_string(),
        };
    }
    normalize_double_precision(&mut parts);
    strip_integer_display_width(&mut parts);
    if value.trim().to_ascii_lowercase().starts_with("serial")
        && !parts.iter().any(|part| part == "unsigned")
    {
        parts.insert(1, "unsigned".to_string());
    }
    if parts.iter().any(|part| part == "zerofill") && !parts.iter().any(|part| part == "unsigned") {
        let position = parts
            .iter()
            .position(|part| part == "zerofill")
            .unwrap_or(parts.len());
        parts.insert(position, "unsigned".to_string());
    }
    parts.join("\u{1f}")
}

/// Treats the documented `DOUBLE PRECISION` spelling as the `DOUBLE` catalog type.
fn normalize_double_precision(parts: &mut Vec<String>) {
    if parts.first().is_some_and(|part| part == "double")
        && parts.get(1).is_some_and(|part| part == "precision")
    {
        parts.remove(1);
    }
}

/// Removes deprecated integer display widths while preserving precision elsewhere.
fn strip_integer_display_width(parts: &mut Vec<String>) {
    let integer = parts.first().is_some_and(|part| {
        matches!(
            part.as_str(),
            "tinyint" | "smallint" | "mediumint" | "int" | "bigint"
        )
    });
    if integer
        && parts.len() >= 4
        && parts[1] == "("
        && parts[3] == ")"
        && parts[2].chars().all(|ch| ch.is_ascii_digit())
    {
        parts.drain(1..4);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MySQL's `DOUBLE PRECISION` alias matches its reflected `DOUBLE` type.
    #[test]
    fn double_precision_matches_double() {
        assert_eq!(key("DOUBLE PRECISION"), key("double"));
    }
}
