//! Conservative recovery of modeled table cores with unmanaged outer options.

use crate::dialects::Dialect;

/// A cleaned table statement and the syntax removed around its modeled body.
pub(super) struct RecoveredTableSql {
    pub core_sql: String,
    pub header_options: Vec<String>,
    pub tail_options: Vec<String>,
}

/// Removes supported outer table options while preserving the complete table body.
pub(super) fn recover_table_sql(sql: &str, dialect: Dialect) -> Option<RecoveredTableSql> {
    let open = find_body_open(sql)?;
    let close = find_body_close(sql, open)?;
    let (prefix, header_options) = recover_header(&sql[..open], dialect)?;
    let tail = sql[close + 1..].trim();
    if header_options.is_empty() && tail.is_empty() {
        return None;
    }
    let tail_options = (!tail.is_empty())
        .then(|| tail.to_string())
        .into_iter()
        .collect();
    Some(RecoveredTableSql {
        core_sql: format!("{prefix}{}", &sql[open..=close]),
        header_options,
        tail_options,
    })
}

/// Finds the opening delimiter of the top-level table definition.
fn find_body_open(sql: &str) -> Option<usize> {
    scan_normal(sql, 0, |idx, ch, depth| {
        (ch == '(' && depth == 0).then_some(idx)
    })
}

/// Finds the matching end of the top-level table definition.
fn find_body_close(sql: &str, open: usize) -> Option<usize> {
    scan_normal(sql, open, |idx, ch, depth| {
        (ch == ')' && depth == 1).then_some(idx)
    })
}

/// Removes dialect-supported header modifiers that sqlparser cannot lower.
fn recover_header(prefix: &str, dialect: Dialect) -> Option<(String, Vec<String>)> {
    if dialect != Dialect::Postgres {
        return Some((prefix.to_string(), Vec::new()));
    }
    let Some(range) = find_ascii_word(prefix, "UNLOGGED") else {
        return Some((prefix.to_string(), Vec::new()));
    };
    let mut cleaned = String::with_capacity(prefix.len());
    cleaned.push_str(&prefix[..range.0]);
    cleaned.push_str(&prefix[range.1..]);
    Some((cleaned, vec![prefix[range.0..range.1].to_string()]))
}

/// Finds an ASCII keyword at a lexical word boundary.
fn find_ascii_word(source: &str, word: &str) -> Option<(usize, usize)> {
    let upper = source.to_ascii_uppercase();
    let mut start = 0;
    while let Some(offset) = upper[start..].find(word) {
        let found = start + offset;
        let end = found + word.len();
        let before = source[..found].chars().next_back();
        let after = source[end..].chars().next();
        if before.is_none_or(|ch| !is_word(ch)) && after.is_none_or(|ch| !is_word(ch)) {
            return Some((found, end));
        }
        start = end;
    }
    None
}

fn is_word(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '_'
}

/// Scans normal SQL text while skipping protected lexical regions.
fn scan_normal<T>(
    sql: &str,
    start: usize,
    mut found: impl FnMut(usize, char, usize) -> Option<T>,
) -> Option<T> {
    let mut i = start;
    let mut depth = 0usize;
    while i < sql.len() {
        let rest = &sql[i..];
        if let Some(len) = protected_len(rest) {
            i += len;
            continue;
        }
        let ch = rest.chars().next()?;
        if let Some(value) = found(i, ch, depth) {
            return Some(value);
        }
        match ch {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ => {}
        }
        i += ch.len_utf8();
    }
    None
}

/// Returns the byte length of a quoted or commented lexical region.
fn protected_len(source: &str) -> Option<usize> {
    if source.starts_with("--") {
        return Some(source.find('\n').unwrap_or(source.len()));
    }
    if source.starts_with("/*") {
        return nested_comment_len(source);
    }
    let ch = source.chars().next()?;
    if matches!(ch, '\'' | '"' | '`') {
        return quoted_len(source, ch);
    }
    (ch == '$').then(|| dollar_len(source)).flatten()
}

fn quoted_len(source: &str, quote: char) -> Option<usize> {
    let mut chars = source.char_indices().skip(1);
    while let Some((idx, ch)) = chars.next() {
        if ch != quote {
            continue;
        }
        let end = idx + ch.len_utf8();
        if source[end..].starts_with(quote) {
            chars.next();
        } else {
            return Some(end);
        }
    }
    None
}

fn dollar_len(source: &str) -> Option<usize> {
    let tag_end = source[1..].find('$')? + 2;
    let tag = &source[..tag_end];
    let body_end = source[tag_end..].find(tag)?;
    Some(tag_end + body_end + tag.len())
}

fn nested_comment_len(source: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = 0usize;
    while i < source.len() {
        if source[i..].starts_with("/*") {
            depth += 1;
            i += 2;
        } else if source[i..].starts_with("*/") {
            depth = depth.checked_sub(1)?;
            i += 2;
            if depth == 0 {
                return Some(i);
            }
        } else {
            i += source[i..].chars().next()?.len_utf8();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies PostgreSQL UNLOGGED is preserved while the table core remains parseable.
    #[test]
    fn recovers_unlogged_header() {
        let recovered = recover_table_sql(
            "CREATE UNLOGGED TABLE events (id integer)",
            Dialect::Postgres,
        )
        .expect("recover table");
        assert_eq!(recovered.header_options, ["UNLOGGED"]);
        assert_eq!(recovered.core_sql, "CREATE  TABLE events (id integer)");
    }

    /// Verifies nested expressions do not terminate table-body recovery early.
    #[test]
    fn preserves_nested_table_body() {
        let recovered = recover_table_sql(
            "CREATE TABLE metrics (value integer DEFAULT (1 + (2))) TABLESPACE fast",
            Dialect::Postgres,
        )
        .expect("recover table");
        assert!(recovered.core_sql.ends_with("DEFAULT (1 + (2)))"));
        assert_eq!(recovered.tail_options, ["TABLESPACE fast"]);
    }

    /// Verifies SQLite table tails are retained as unmanaged options.
    #[test]
    fn recovers_sqlite_tail() {
        let recovered =
            recover_table_sql("CREATE TABLE records (id integer) STRICT", Dialect::Sqlite)
                .expect("recover table");
        assert_eq!(recovered.tail_options, ["STRICT"]);
    }
}
