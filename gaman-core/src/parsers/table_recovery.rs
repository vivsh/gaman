//! Conservative recovery of modeled table cores with unmanaged outer options.

use crate::dialects::Dialect;
use crate::parsers::tokens::SqlTokenKind;

/// A cleaned table statement and the syntax removed around its modeled body.
pub(super) struct RecoveredTableSql {
    pub core_sql: String,
    pub header_options: Vec<String>,
    pub tail_options: Vec<String>,
}

/// Removes supported outer table options while preserving the complete table body.
pub(super) fn recover_table_sql(sql: &str, dialect: Dialect) -> Option<RecoveredTableSql> {
    let tokens = dialect.tokenizer().tokenize(sql).ok()?;
    let open_index = tokens
        .iter()
        .position(|token| matches!(token.kind, SqlTokenKind::LeftParen))?;
    let open = tokens[open_index].span.start;
    let close = find_body_close(&tokens, open_index)?;
    let (prefix, header_options) = recover_header(&sql[..open], dialect)?;
    let tail = sql[close + 1..].trim();
    let mut core_sql = format!("{prefix}{}", &sql[open..=close]);
    if matches!(dialect, Dialect::Mysql | Dialect::Mariadb) {
        core_sql = remove_zerofill(&core_sql, dialect)?;
    }
    if dialect == Dialect::Mariadb {
        core_sql = replace_persistent_with_stored(&core_sql, dialect)?;
    }
    if header_options.is_empty() && tail.is_empty() && core_sql == sql.trim() {
        return None;
    }
    let tail_options = (!tail.is_empty())
        .then(|| tail.to_string())
        .into_iter()
        .collect();
    Some(RecoveredTableSql {
        core_sql,
        header_options,
        tail_options,
    })
}

/// Canonicalizes MariaDB's generated-column storage alias for the shared AST parser.
fn replace_persistent_with_stored(sql: &str, dialect: Dialect) -> Option<String> {
    let tokens = dialect.tokenizer().tokenize(sql).ok()?;
    let mut output = sql.to_string();
    for (index, token) in tokens.iter().enumerate().rev() {
        if token.canonical_word() == Some("PERSISTENT")
            && is_generated_storage_position(&tokens, index)
        {
            output.replace_range(token.span.clone(), "STORED");
        }
    }
    Some(output)
}

fn remove_zerofill(sql: &str, dialect: Dialect) -> Option<String> {
    let tokens = dialect.tokenizer().tokenize(sql).ok()?;
    let mut output = sql.to_string();
    for token in tokens
        .iter()
        .rev()
        .filter(|token| token.canonical_word() == Some("ZEROFILL"))
    {
        output.replace_range(token.span.clone(), "");
    }
    Some(output)
}

/// Accepts `PERSISTENT` only after a balanced generated `AS (...)` expression.
fn is_generated_storage_position(
    tokens: &[crate::parsers::tokens::SqlToken],
    index: usize,
) -> bool {
    (0..index)
        .rev()
        .find(|candidate| !tokens[*candidate].is_trivia())
        .is_some_and(|candidate| matches!(tokens[candidate].kind, SqlTokenKind::RightParen))
}

/// Finds the opening delimiter of the top-level table definition.
/// Finds the matching end of the top-level table definition.
fn find_body_close(
    tokens: &[crate::parsers::tokens::SqlToken],
    open_index: usize,
) -> Option<usize> {
    let mut depth = 0usize;
    for token in &tokens[open_index..] {
        match token.kind {
            SqlTokenKind::LeftParen => depth += 1,
            SqlTokenKind::RightParen if depth == 1 => return Some(token.span.start),
            SqlTokenKind::RightParen => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    None
}

/// Removes dialect-supported header modifiers that sqlparser cannot lower.
fn recover_header(prefix: &str, dialect: Dialect) -> Option<(String, Vec<String>)> {
    if !matches!(dialect, Dialect::Postgres) {
        return Some((prefix.to_string(), Vec::new()));
    }
    let tokens = dialect.tokenizer().tokenize(prefix).ok()?;
    let Some(range) = tokens
        .iter()
        .find(|token| token.canonical_word() == Some("UNLOGGED"))
        .map(|token| token.span.clone())
    else {
        return Some((prefix.to_string(), Vec::new()));
    };
    let mut cleaned = String::with_capacity(prefix.len());
    cleaned.push_str(&prefix[..range.start]);
    cleaned.push_str(&prefix[range.end..]);
    Some((cleaned, vec![prefix[range].to_string()]))
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
