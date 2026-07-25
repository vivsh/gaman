//! Bounded SQL context used when a database rejects one statement.

use std::fmt::{self, Display, Formatter};

/// Maximum number of characters included in one statement signature.
const MAX_SIGNATURE_CHARS: usize = 160;
/// Maximum number of characters included in one highlighted source line.
const MAX_EXCERPT_CHARS: usize = 200;

/// Stable database error information supplied by an executor.
///
/// Executors preserve only fields emitted directly by their database driver. They never attach
/// the submitted SQL text, which can be arbitrarily large or sensitive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DatabaseFailure {
    /// Concise database-provided failure message.
    pub message: String,
    /// Optional database error code, such as a PostgreSQL SQLSTATE.
    pub code: Option<String>,
    /// Optional driver-provided character position.
    pub position: Option<DatabasePosition>,
}

impl DatabaseFailure {
    /// Creates a database failure without a database-specific code or position.
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            code: None,
            position: None,
        }
    }

    /// Adds a stable database error code.
    pub fn with_code(mut self, code: impl Into<String>) -> Self {
        self.code = Some(code.into());
        self
    }

    /// Adds a driver-provided character position.
    pub fn with_position(mut self, position: DatabasePosition) -> Self {
        self.position = Some(position);
        self
    }
}

impl Display for DatabaseFailure {
    /// Renders only stable message and code, never an internal query body.
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match &self.code {
            Some(code) => write!(formatter, "[{code}] {}", self.message),
            None => formatter.write_str(&self.message),
        }
    }
}

/// A character position reported directly by a database driver.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DatabasePosition {
    /// One-based character position in the submitted statement.
    Statement(usize),
    /// One-based character position in a database-provided internal query.
    Internal {
        /// One-based character position in [`Self::query`].
        position: usize,
        /// Query text supplied by the database driver.
        query: String,
    },
}

/// Compact, display-safe context for one rendered SQL statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementDiagnostic {
    /// Bounded first-line identity of the statement.
    pub signature: String,
    /// Optional source location supplied by the database.
    pub location: Option<StatementLocation>,
}

/// One bounded source location rendered from a database-provided position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatementLocation {
    /// Whether the location refers to submitted or database-internal SQL.
    pub source: StatementLocationSource,
    /// One-based line number in the database-provided source text.
    pub line: usize,
    /// One-based column number in the database-provided source text.
    pub column: usize,
    /// Bounded line excerpt containing the reported position.
    pub excerpt: String,
    /// Zero-based display offset for a caret below [`Self::excerpt`].
    pub caret_offset: usize,
}

/// The source text to which a database-reported position applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StatementLocationSource {
    /// The SQL statement submitted by Gaman.
    Statement,
    /// An internal query returned by the database, such as a SQL-function body.
    Internal,
}

impl StatementLocationSource {
    /// Returns the concise label used in host diagnostics.
    pub fn label(self) -> &'static str {
        match self {
            Self::Statement => "SQL",
            Self::Internal => "internal SQL",
        }
    }
}

/// Builds bounded statement context from one rendered statement and stable driver information.
pub(crate) fn statement_diagnostic(
    statement: &str,
    position: Option<&DatabasePosition>,
) -> StatementDiagnostic {
    let location = match position {
        Some(DatabasePosition::Statement(position)) => {
            statement_location(statement, *position, StatementLocationSource::Statement)
        }
        Some(DatabasePosition::Internal { position, query }) => {
            statement_location(query, *position, StatementLocationSource::Internal)
        }
        None => None,
    };
    StatementDiagnostic {
        signature: statement_signature(statement),
        location,
    }
}

/// Returns the first non-empty SQL line in a bounded, single-line form.
fn statement_signature(statement: &str) -> String {
    let line = statement
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("");
    let compact = line.split_whitespace().collect::<Vec<_>>().join(" ");
    truncate_chars(&compact, MAX_SIGNATURE_CHARS)
}

/// Converts a one-based character position into a bounded source line and caret offset.
fn statement_location(
    source: &str,
    position: usize,
    source_kind: StatementLocationSource,
) -> Option<StatementLocation> {
    let byte_index = character_byte_index(source, position)?;
    let before = &source[..byte_index];
    let line_start = before.rfind('\n').map_or(0, |index| index + 1);
    let line_end = source[byte_index..]
        .find('\n')
        .map_or(source.len(), |index| byte_index + index);
    let line = source[line_start..line_end].replace('\t', " ");
    let column = source[line_start..byte_index].chars().count() + 1;
    let line_number = source[..line_start]
        .chars()
        .filter(|character| *character == '\n')
        .count()
        + 1;
    let (excerpt, caret_offset) = bounded_excerpt(&line, column)?;
    Some(StatementLocation {
        source: source_kind,
        line: line_number,
        column,
        excerpt,
        caret_offset,
    })
}

/// Resolves a one-based character position without assuming UTF-8 bytes equal characters.
fn character_byte_index(source: &str, position: usize) -> Option<usize> {
    position
        .checked_sub(1)
        .and_then(|index| source.char_indices().nth(index).map(|(byte, _)| byte))
}

/// Trims a source line around a one-based column while preserving a caret-safe display offset.
fn bounded_excerpt(line: &str, column: usize) -> Option<(String, usize)> {
    let characters: Vec<char> = line.chars().collect();
    let cursor = column.checked_sub(1)?;
    if cursor >= characters.len() {
        return None;
    }
    if characters.len() <= MAX_EXCERPT_CHARS {
        return Some((line.to_string(), cursor));
    }
    let content_limit = MAX_EXCERPT_CHARS.saturating_sub(4);
    let mut start = cursor.saturating_sub(content_limit / 2);
    let end = (start + content_limit).min(characters.len());
    if end == characters.len() {
        start = end.saturating_sub(content_limit);
    }
    let prefix = usize::from(start > 0);
    let suffix = usize::from(end < characters.len());
    let mut excerpt = String::new();
    if prefix > 0 {
        excerpt.push('…');
    }
    excerpt.extend(characters[start..end].iter());
    if suffix > 0 {
        excerpt.push('…');
    }
    Some((excerpt, prefix + cursor - start))
}

/// Shortens text on character boundaries and marks omitted content with one ellipsis.
fn truncate_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_string();
    }
    let mut result: String = value.chars().take(limit.saturating_sub(1)).collect();
    result.push('…');
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies statement positions preserve UTF-8 character columns and source lines.
    #[test]
    fn statement_position_uses_character_columns() {
        let statement = "SELECT café\nFROM reports";
        let diagnostic = statement_diagnostic(statement, Some(&DatabasePosition::Statement(11)));
        let location = diagnostic.location.expect("location");
        assert_eq!(location.line, 1);
        assert_eq!(location.column, 11);
        assert_eq!(location.excerpt, "SELECT café");
        assert_eq!(location.caret_offset, 10);
    }

    /// Verifies internal-query positions use database-provided source rather than submitted DDL.
    #[test]
    fn internal_position_uses_internal_query() {
        let position = DatabasePosition::Internal {
            position: 8,
            query: "SELECT ambiguous_column FROM one JOIN two ON true".to_string(),
        };
        let diagnostic = statement_diagnostic("CREATE FUNCTION ...", Some(&position));
        let location = diagnostic.location.expect("location");
        assert_eq!(location.source, StatementLocationSource::Internal);
        assert_eq!(location.column, 8);
        assert!(location.excerpt.starts_with("SELECT"));
    }

    /// Verifies long statements and source lines never exceed their diagnostic bounds.
    #[test]
    fn diagnostics_bound_statement_and_excerpt_sizes() {
        let statement = format!("{}\n{}", "CREATE ".repeat(80), "x".repeat(400));
        let position = DatabasePosition::Statement(statement.chars().count() - 50);
        let diagnostic = statement_diagnostic(&statement, Some(&position));
        assert!(diagnostic.signature.chars().count() <= MAX_SIGNATURE_CHARS);
        assert!(
            diagnostic
                .location
                .expect("location")
                .excerpt
                .chars()
                .count()
                <= MAX_EXCERPT_CHARS
        );
    }

    /// Verifies unavailable database positions do not create guessed locations.
    #[test]
    fn missing_position_has_no_location() {
        assert!(
            statement_diagnostic("CREATE TABLE reports", None)
                .location
                .is_none()
        );
    }
}
