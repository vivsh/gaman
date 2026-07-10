//! Shared dialect-aware SQL tokenization.
//!
//! Tokens retain exact authored source slices while exposing uppercase canonical
//! values for unquoted words. Consumers choose their own comparison policy;
//! tokenization itself never rewrites stored or rendered SQL.

use std::ops::Range;

use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::tokenizer::{Location, Token, Tokenizer, Whitespace};
use thiserror::Error;

/// Failure to tokenize SQL with source location context.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("SQL tokenization failed at line {line}, column {column}: {message}")]
pub(crate) struct TokenizeError {
    pub(crate) message: String,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

/// Stable token categories consumed by Gaman's lexical algorithms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SqlTokenKind {
    Word { value: String, canonical: String },
    QuotedIdentifier { value: String, quote: char },
    String,
    Number,
    Whitespace,
    Comment,
    LeftParen,
    RightParen,
    LeftBracket,
    RightBracket,
    LeftBrace,
    RightBrace,
    Comma,
    Dot,
    Semicolon,
    Other,
}

/// One dialect token with exact authored text and a half-open byte span.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SqlToken {
    pub(crate) kind: SqlTokenKind,
    pub(crate) raw: String,
    pub(crate) span: Range<usize>,
    pub(crate) line: usize,
    pub(crate) column: usize,
}

impl SqlToken {
    pub(crate) fn canonical_word(&self) -> Option<&str> {
        match &self.kind {
            SqlTokenKind::Word { canonical, .. } => Some(canonical),
            _ => None,
        }
    }

    pub(crate) fn is_trivia(&self) -> bool {
        matches!(self.kind, SqlTokenKind::Whitespace | SqlTokenKind::Comment)
    }
}

/// Stateless dialect tokenizer used by all lexical consumers.
pub(crate) trait SqlTokenizer: Sync {
    fn tokenize(&self, source: &str) -> Result<Vec<SqlToken>, TokenizeError>;
}

/// Compares SQL expressions using dialect tokens without rewriting authored source.
pub(crate) fn expressions_equal(tokenizer: &dyn SqlTokenizer, left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }
    match (
        comparison_tokens(tokenizer, left),
        comparison_tokens(tokenizer, right),
    ) {
        (Ok(left), Ok(right)) => left == right,
        _ => normalize_line_endings(left) == normalize_line_endings(right),
    }
}

fn comparison_tokens(
    tokenizer: &dyn SqlTokenizer,
    source: &str,
) -> Result<Vec<ComparisonToken>, TokenizeError> {
    tokenizer
        .tokenize(source)?
        .into_iter()
        .filter(|token| !token.is_trivia())
        .map(|token| {
            Ok(match token.kind {
                SqlTokenKind::Word { canonical, .. } => ComparisonToken::Word(canonical),
                SqlTokenKind::QuotedIdentifier { .. } | SqlTokenKind::String => {
                    ComparisonToken::Protected(token.raw)
                }
                _ => ComparisonToken::Exact(token.raw),
            })
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
enum ComparisonToken {
    Word(String),
    Protected(String),
    Exact(String),
}

fn normalize_line_endings(source: &str) -> String {
    source.replace("\r\n", "\n").replace('\r', "\n")
}

pub(crate) static POSTGRES_TOKENIZER: PostgresTokenizer = PostgresTokenizer;
pub(crate) static SQLITE_TOKENIZER: SqliteTokenizer = SqliteTokenizer;
pub(crate) static MYSQL_TOKENIZER: MysqlTokenizer = MysqlTokenizer;

pub(crate) struct PostgresTokenizer;
pub(crate) struct SqliteTokenizer;
pub(crate) struct MysqlTokenizer;

impl SqlTokenizer for PostgresTokenizer {
    fn tokenize(&self, source: &str) -> Result<Vec<SqlToken>, TokenizeError> {
        tokenize_with(&PostgreSqlDialect {}, source)
    }
}

impl SqlTokenizer for SqliteTokenizer {
    fn tokenize(&self, source: &str) -> Result<Vec<SqlToken>, TokenizeError> {
        tokenize_with(&SQLiteDialect {}, source)
    }
}

impl SqlTokenizer for MysqlTokenizer {
    fn tokenize(&self, source: &str) -> Result<Vec<SqlToken>, TokenizeError> {
        tokenize_with(&MySqlDialect {}, source)
    }
}

/// Tokenizes SQL and maps sqlparser locations back to exact byte ranges.
fn tokenize_with(
    dialect: &dyn sqlparser::dialect::Dialect,
    source: &str,
) -> Result<Vec<SqlToken>, TokenizeError> {
    let line_starts = line_starts(source);
    let tokens = Tokenizer::new(dialect, source)
        .with_unescape(false)
        .tokenize_with_location()
        .map_err(|error| TokenizeError {
            message: error.message,
            line: error.location.line as usize,
            column: error.location.column as usize,
        })?;

    tokens
        .into_iter()
        .map(|token| {
            let start = byte_offset(source, &line_starts, token.span.start)?;
            let end = byte_offset(source, &line_starts, token.span.end)?;
            let raw = source.get(start..end).ok_or_else(|| TokenizeError {
                message: "token span is outside the SQL source".to_string(),
                line: token.span.start.line as usize,
                column: token.span.start.column as usize,
            })?;
            Ok(SqlToken {
                kind: token_kind(token.token),
                raw: raw.to_string(),
                span: start..end,
                line: token.span.start.line as usize,
                column: token.span.start.column as usize,
            })
        })
        .collect()
}

/// Converts dependency tokens into the stable internal categories used by consumers.
fn token_kind(token: Token) -> SqlTokenKind {
    match token {
        Token::Word(word) => match word.quote_style {
            Some(quote) => SqlTokenKind::QuotedIdentifier {
                value: word.value,
                quote,
            },
            None => SqlTokenKind::Word {
                canonical: word.value.to_ascii_uppercase(),
                value: word.value,
            },
        },
        Token::Whitespace(
            Whitespace::SingleLineComment { .. } | Whitespace::MultiLineComment(_),
        ) => SqlTokenKind::Comment,
        Token::Whitespace(_) => SqlTokenKind::Whitespace,
        Token::Number(_, _) => SqlTokenKind::Number,
        Token::DoubleQuotedString(value) => SqlTokenKind::QuotedIdentifier { value, quote: '"' },
        Token::SingleQuotedString(_)
        | Token::TripleSingleQuotedString(_)
        | Token::TripleDoubleQuotedString(_)
        | Token::DollarQuotedString(_)
        | Token::SingleQuotedByteStringLiteral(_)
        | Token::DoubleQuotedByteStringLiteral(_)
        | Token::TripleSingleQuotedByteStringLiteral(_)
        | Token::TripleDoubleQuotedByteStringLiteral(_)
        | Token::SingleQuotedRawStringLiteral(_)
        | Token::DoubleQuotedRawStringLiteral(_)
        | Token::TripleSingleQuotedRawStringLiteral(_)
        | Token::TripleDoubleQuotedRawStringLiteral(_)
        | Token::NationalStringLiteral(_)
        | Token::QuoteDelimitedStringLiteral(_)
        | Token::NationalQuoteDelimitedStringLiteral(_)
        | Token::EscapedStringLiteral(_)
        | Token::UnicodeStringLiteral(_)
        | Token::HexStringLiteral(_) => SqlTokenKind::String,
        Token::LParen => SqlTokenKind::LeftParen,
        Token::RParen => SqlTokenKind::RightParen,
        Token::LBracket => SqlTokenKind::LeftBracket,
        Token::RBracket => SqlTokenKind::RightBracket,
        Token::LBrace => SqlTokenKind::LeftBrace,
        Token::RBrace => SqlTokenKind::RightBrace,
        Token::Comma => SqlTokenKind::Comma,
        Token::Period => SqlTokenKind::Dot,
        Token::SemiColon => SqlTokenKind::Semicolon,
        _ => SqlTokenKind::Other,
    }
}

fn line_starts(source: &str) -> Vec<usize> {
    std::iter::once(0)
        .chain(source.match_indices('\n').map(|(index, _)| index + 1))
        .collect()
}

/// Converts sqlparser's character-based location into a source byte offset.
fn byte_offset(
    source: &str,
    line_starts: &[usize],
    location: Location,
) -> Result<usize, TokenizeError> {
    let line = location.line as usize;
    let column = location.column as usize;
    let start = line
        .checked_sub(1)
        .and_then(|index| line_starts.get(index))
        .copied()
        .ok_or_else(|| location_error(location))?;
    let line_end = source[start..]
        .find('\n')
        .map_or(source.len(), |offset| start + offset);
    source[start..line_end]
        .char_indices()
        .map(|(offset, _)| start + offset)
        .chain(std::iter::once(line_end))
        .nth(column.saturating_sub(1))
        .ok_or_else(|| location_error(location))
}

fn location_error(location: Location) -> TokenizeError {
    TokenizeError {
        message: "invalid token source location".to_string(),
        line: location.line as usize,
        column: location.column as usize,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies canonical words are uppercase while quoted regions retain authored text.
    #[test]
    fn canonicalizes_only_unquoted_words() {
        let tokens = POSTGRES_TOKENIZER
            .tokenize("select now(), 'Now', \"CreatedAt\", $$Body$$")
            .expect("tokenize");
        assert_eq!(tokens[0].canonical_word(), Some("SELECT"));
        assert!(
            tokens
                .iter()
                .any(|token| token.canonical_word() == Some("NOW"))
        );
        assert!(tokens.iter().any(|token| token.raw == "'Now'"));
        assert!(tokens.iter().any(|token| token.raw == "\"CreatedAt\""));
        assert!(tokens.iter().any(|token| token.raw == "$$Body$$"));
    }

    /// Verifies token byte spans remain exact for Unicode and multiline SQL.
    #[test]
    fn preserves_exact_unicode_multiline_spans() {
        let source = "SELECT 'é';\nSELECT café";
        let tokens = POSTGRES_TOKENIZER.tokenize(source).expect("tokenize");
        for token in tokens {
            assert_eq!(&source[token.span], token.raw);
        }
    }

    /// Verifies dialect-specific quoted identifiers and comments are recognized.
    #[test]
    fn recognizes_mysql_and_sqlite_lexical_forms() {
        let mysql = MYSQL_TOKENIZER
            .tokenize("# note\nSELECT `Mixed`")
            .expect("mysql tokenize");
        assert!(
            mysql
                .iter()
                .any(|token| matches!(token.kind, SqlTokenKind::Comment))
        );
        assert!(
            mysql
                .iter()
                .any(|token| matches!(token.kind, SqlTokenKind::QuotedIdentifier { .. }))
        );

        let sqlite = SQLITE_TOKENIZER
            .tokenize("SELECT [Mixed]")
            .expect("sqlite tokenize");
        assert!(
            sqlite
                .iter()
                .any(|token| matches!(token.kind, SqlTokenKind::QuotedIdentifier { .. }))
        );
    }

    /// Verifies expression comparison folds only unquoted words and trivia.
    #[test]
    fn compares_expression_tokens_conservatively() {
        assert!(expressions_equal(
            &POSTGRES_TOKENIZER,
            "NOW()",
            " now ( ) /*x*/"
        ));
        assert!(expressions_equal(
            &POSTGRES_TOKENIZER,
            "CURRENT_TIMESTAMP",
            "current_timestamp"
        ));
        assert!(!expressions_equal(&POSTGRES_TOKENIZER, "'NOW'", "'now'"));
        assert!(!expressions_equal(
            &POSTGRES_TOKENIZER,
            "\"CreatedAt\"",
            "\"createdat\""
        ));
    }

    /// Verifies malformed expressions use deterministic raw fallback comparison.
    #[test]
    fn malformed_expression_comparison_falls_back_conservatively() {
        assert!(expressions_equal(
            &POSTGRES_TOKENIZER,
            "'unterminated\r\n",
            "'unterminated\n"
        ));
        assert!(!expressions_equal(
            &POSTGRES_TOKENIZER,
            "'unterminated A",
            "'unterminated a"
        ));
    }
}
