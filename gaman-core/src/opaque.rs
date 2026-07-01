pub(crate) fn opaque_sources_equal(left: &str, right: &str) -> bool {
    left == right || canonicalize_opaque_source(left) == canonicalize_opaque_source(right)
}

pub(crate) fn opaque_option_sources_equal(left: &Option<String>, right: &Option<String>) -> bool {
    match (left, right) {
        (Some(left), Some(right)) => opaque_sources_equal(left, right),
        (None, None) => true,
        _ => false,
    }
}

pub(crate) fn canonicalize_opaque_source(source: &str) -> String {
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
    let mut pending_space = false;

    while i < source.len() {
        let rest = &source[i..];
        let ch = rest.chars().next()?;

        if ch.is_whitespace() {
            pending_space = true;
            i += ch.len_utf8();
            continue;
        }

        if rest.starts_with("--") {
            if let Some(end) = rest.find('\n') {
                i += end + 1;
            } else {
                i = source.len();
            }
            pending_space = true;
            continue;
        }

        if rest.starts_with("/*") {
            let end = rest.find("*/")?;
            i += end + 2;
            pending_space = true;
            continue;
        }

        if let Some(delim_len) = dollar_quote_delimiter_len(rest) {
            flush_space(&mut out, &mut pending_space);
            let delimiter = &rest[..delim_len];
            let body = &rest[delim_len..];
            let end = body.find(delimiter)?;
            let protected_len = delim_len + end + delim_len;
            out.push_str(&rest[..protected_len]);
            i += protected_len;
            continue;
        }

        if matches!(ch, '\'' | '"' | '`') {
            flush_space(&mut out, &mut pending_space);
            let protected_len = quoted_region_len(rest, ch)?;
            out.push_str(&rest[..protected_len]);
            i += protected_len;
            continue;
        }

        if ch == '[' {
            flush_space(&mut out, &mut pending_space);
            let protected_len = bracket_region_len(rest)?;
            out.push_str(&rest[..protected_len]);
            i += protected_len;
            continue;
        }

        flush_space(&mut out, &mut pending_space);
        out.push(ch);
        i += ch.len_utf8();
    }

    Some(out.trim().to_string())
}

fn flush_space(out: &mut String, pending_space: &mut bool) {
    if *pending_space && !out.is_empty() {
        out.push(' ');
    }
    *pending_space = false;
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
