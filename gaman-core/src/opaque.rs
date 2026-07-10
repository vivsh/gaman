use sha2::{Digest, Sha256};

use crate::parsers::tokens::{POSTGRES_TOKENIZER, SqlTokenizer};

pub(crate) fn opaque_sources_equal(left: &str, right: &str) -> bool {
    left == right || fingerprint_opaque_source(left) == fingerprint_opaque_source(right)
}

pub(crate) fn opaque_option_sources_equal(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => opaque_sources_equal(left, right),
        (None, None) => true,
        _ => false,
    }
}

/// Compares ordered unmanaged table clauses by lexical SQL content and placement.
pub(crate) fn table_option_sources_equal(
    left_header: &[String],
    left_tail: &[String],
    right_header: &[String],
    right_tail: &[String],
) -> bool {
    source_lists_equal(left_header, right_header) && source_lists_equal(left_tail, right_tail)
}

fn source_lists_equal(left: &[String], right: &[String]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| opaque_sources_equal(left, right))
}

pub(crate) fn fingerprint_opaque_source(source: &str) -> String {
    let canonical = canonicalize_opaque_source(source);
    let digest = Sha256::digest(canonical.as_bytes());
    let mut encoded = String::with_capacity(10 + digest.len() * 2);
    encoded.push_str("v1:sha256:");
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn canonicalize_opaque_source(source: &str) -> String {
    let normalized = normalize_line_endings(source);
    canonicalize_normalized(&normalized).unwrap_or(normalized)
}

fn normalize_line_endings(source: &str) -> String {
    let mut out = String::with_capacity(source.len());
    let mut chars = source.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

fn canonicalize_normalized(source: &str) -> Option<String> {
    let mut out = String::with_capacity(source.len());
    for token in POSTGRES_TOKENIZER.tokenize(source).ok()? {
        if !token.is_trivia() {
            push_token(&mut out, &token.raw);
        }
    }
    Some(out)
}

fn push_token(out: &mut String, token: &str) {
    use std::fmt::Write;
    let _ = write!(out, "{}:", token.len());
    out.push_str(token);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match_is_equal() {
        assert!(opaque_sources_equal("SELECT 1", "SELECT 1"));
    }

    #[test]
    fn whitespace_outside_protected_regions_is_ignored() {
        assert!(opaque_sources_equal(
            "SELECT  a\nFROM users",
            "SELECT a FROM users"
        ));
    }

    /// Verifies line wrapping between SQL clauses does not change the token fingerprint.
    #[test]
    fn clause_line_wrapping_is_ignored() {
        assert!(opaque_sources_equal(
            "INSERT INTO audit_log(user_id) VALUES (NEW.id);",
            "INSERT INTO audit_log(user_id)\nVALUES (NEW.id);\n"
        ));
    }

    #[test]
    fn comments_outside_protected_regions_are_ignored() {
        assert!(opaque_sources_equal(
            "SELECT a -- ignored\nFROM users /* ignored */ WHERE id = 1",
            "SELECT a FROM users WHERE id = 1"
        ));
    }

    #[test]
    fn single_quoted_contents_are_preserved() {
        assert!(!opaque_sources_equal("SELECT 'a b'", "SELECT 'ab'"));
    }

    #[test]
    fn double_quoted_contents_are_preserved() {
        assert!(!opaque_sources_equal(r#"SELECT "a b""#, r#"SELECT "ab""#));
    }

    #[test]
    fn backtick_and_bracket_contents_are_preserved() {
        assert!(!opaque_sources_equal("SELECT `a b`", "SELECT `ab`"));
        assert!(!opaque_sources_equal("SELECT [a b]", "SELECT [ab]"));
    }

    #[test]
    fn dollar_quoted_contents_are_preserved() {
        assert!(!opaque_sources_equal("SELECT $$a b$$", "SELECT $$ab$$"));
        assert!(!opaque_sources_equal(
            "SELECT $tag$a b$tag$",
            "SELECT $tag$ab$tag$"
        ));
    }

    #[test]
    fn unterminated_protected_region_falls_back_to_raw_normalized_text() {
        assert_eq!(
            canonicalize_opaque_source("SELECT 'a  b"),
            "SELECT 'a  b".to_string()
        );
    }
}
