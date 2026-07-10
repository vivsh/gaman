use sha2::{Digest, Sha256};

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
    let mut i = 0;

    while i < source.len() {
        let rest = &source[i..];
        let ch = rest.chars().next()?;

        if ch.is_whitespace() {
            i += ch.len_utf8();
            continue;
        }

        if rest.starts_with("--") {
            if let Some(end) = rest.find('\n') {
                i += end + 1;
            } else {
                i = source.len();
            }
            continue;
        }

        if rest.starts_with("/*") {
            i += block_comment_len(rest)?;
            continue;
        }

        if let Some(delim_len) = dollar_quote_delimiter_len(rest) {
            let delimiter = &rest[..delim_len];
            let body = &rest[delim_len..];
            let end = body.find(delimiter)?;
            let protected_len = delim_len + end + delim_len;
            push_token(&mut out, &rest[..protected_len]);
            i += protected_len;
            continue;
        }

        if matches!(ch, '\'' | '"' | '`') {
            let protected_len = quoted_region_len(rest, ch)?;
            push_token(&mut out, &rest[..protected_len]);
            i += protected_len;
            continue;
        }

        if ch == '[' {
            let protected_len = bracket_region_len(rest)?;
            push_token(&mut out, &rest[..protected_len]);
            i += protected_len;
            continue;
        }

        let token_len = normal_token_len(rest);
        push_token(&mut out, &rest[..token_len]);
        i += token_len;
    }

    Some(out)
}

fn push_token(out: &mut String, token: &str) {
    use std::fmt::Write;
    let _ = write!(out, "{}:", token.len());
    out.push_str(token);
}

fn normal_token_len(source: &str) -> usize {
    let Some(first) = source.chars().next() else {
        return 0;
    };
    let class = token_class(first);
    if class == 3 {
        return first.len_utf8();
    }
    source
        .char_indices()
        .skip(1)
        .find_map(|(idx, ch)| (ch.is_whitespace() || token_class(ch) != class).then_some(idx))
        .unwrap_or(source.len())
}

fn token_class(ch: char) -> u8 {
    if ch.is_alphanumeric() || matches!(ch, '_' | '$') {
        1
    } else if matches!(
        ch,
        '+' | '-' | '*' | '/' | '%' | '<' | '>' | '=' | '!' | '~' | '^' | '|' | '&' | '#'
    ) {
        2
    } else {
        3
    }
}

fn block_comment_len(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < source.len() {
        let rest = &source[i..];
        if rest.starts_with("/*") {
            depth += 1;
            i += 2;
        } else if rest.starts_with("*/") {
            depth = depth.checked_sub(1)?;
            i += 2;
            if depth == 0 {
                return Some(i);
            }
        } else {
            i += rest.chars().next()?.len_utf8();
        }
    }
    None
}

fn quoted_region_len(source: &str, quote: char) -> Option<usize> {
    let mut iter = source.char_indices();
    let (_, first) = iter.next()?;
    if first != quote {
        return None;
    }

    while let Some((idx, ch)) = iter.next() {
        if ch != quote {
            continue;
        }
        let next_idx = idx + ch.len_utf8();
        if source[next_idx..].starts_with(quote) {
            iter.next();
            continue;
        }
        return Some(next_idx);
    }
    None
}

fn bracket_region_len(source: &str) -> Option<usize> {
    let mut iter = source.char_indices();
    let (_, first) = iter.next()?;
    if first != '[' {
        return None;
    }

    while let Some((idx, ch)) = iter.next() {
        if ch != ']' {
            continue;
        }
        let next_idx = idx + ch.len_utf8();
        if source[next_idx..].starts_with(']') {
            iter.next();
            continue;
        }
        return Some(next_idx);
    }
    None
}

fn dollar_quote_delimiter_len(source: &str) -> Option<usize> {
    let bytes = source.as_bytes();
    if bytes.first() != Some(&b'$') {
        return None;
    }
    if bytes.get(1) == Some(&b'$') {
        return Some(2);
    }

    let mut i = 1;
    while let Some(&byte) = bytes.get(i) {
        if byte == b'$' {
            return Some(i + 1);
        }
        if !(byte == b'_' || byte.is_ascii_alphanumeric()) {
            return None;
        }
        i += 1;
    }
    None
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
