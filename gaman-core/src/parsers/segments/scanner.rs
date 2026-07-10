use super::classifier::classify_segment;
use super::types::{Location, SqlSegment, SqlSegmentationExt};
use crate::dialects::Dialect;
use crate::parsers::ParseError;
use crate::parsers::tokens::{SqlToken, SqlTokenKind};

/// Segments SQL text into raw statements for the selected dialect.
pub fn segment_sql(sql: &str, dialect: Dialect) -> Result<Vec<SqlSegment>, ParseError> {
    let tokens = dialect
        .tokenizer()
        .tokenize(sql)
        .map_err(|error| ParseError::segment(dialect, error.line, error.column, error.message))?;
    Scanner::new(sql, dialect, tokens).scan()
}

struct Scanner<'a> {
    sql: &'a str,
    dialect: Dialect,
    tokens: Vec<SqlToken>,
    token_index: usize,
    segment_start: usize,
    meaningful: bool,
    ordinal: usize,
    delimiter: String,
    paren_depth: usize,
    bracket_depth: usize,
    brace_depth: usize,
    body_depth: usize,
    first_word: Option<String>,
    create_kind: Option<String>,
    recent_words: Vec<String>,
    segments: Vec<SqlSegment>,
}

impl<'a> Scanner<'a> {
    fn new(sql: &'a str, dialect: Dialect, tokens: Vec<SqlToken>) -> Self {
        Self {
            sql,
            dialect,
            tokens,
            token_index: 0,
            segment_start: 0,
            meaningful: false,
            ordinal: 1,
            delimiter: ";".to_string(),
            paren_depth: 0,
            bracket_depth: 0,
            brace_depth: 0,
            body_depth: 0,
            first_word: None,
            create_kind: None,
            recent_words: Vec::new(),
            segments: Vec::new(),
        }
    }

    /// Runs statement-boundary state over the shared dialect token stream.
    fn scan(mut self) -> Result<Vec<SqlSegment>, ParseError> {
        while self.token_index < self.tokens.len() {
            if self.consume_delimiter_directive()? || self.consume_active_delimiter()? {
                continue;
            }
            let token = self.tokens[self.token_index].clone();
            self.observe_token(&token)?;
            self.token_index += 1;
        }
        self.finish_at_eof()?;
        Ok(self.segments)
    }

    /// Updates statement-boundary state for one non-owning lexical token.
    fn observe_token(&mut self, token: &SqlToken) -> Result<(), ParseError> {
        if token.is_trivia() {
            return Ok(());
        }
        if let Some(word) = token.canonical_word() {
            if self.should_split_before_word(token, word) {
                self.emit_segment(token.span.start)?;
                self.reset_segment_start(token.span.start);
            }
            self.meaningful = true;
            self.observe_word(word);
            return Ok(());
        }
        match token.kind {
            SqlTokenKind::LeftParen => self.paren_depth += 1,
            SqlTokenKind::RightParen => self.paren_depth = self.paren_depth.saturating_sub(1),
            SqlTokenKind::LeftBracket => self.bracket_depth += 1,
            SqlTokenKind::RightBracket => self.bracket_depth = self.bracket_depth.saturating_sub(1),
            SqlTokenKind::LeftBrace => self.brace_depth += 1,
            SqlTokenKind::RightBrace => self.brace_depth = self.brace_depth.saturating_sub(1),
            SqlTokenKind::Semicolon if self.delimiter == ";" && self.can_split() => {
                self.emit_segment(token.span.start)?;
                self.reset_segment_start(token.span.end);
                return Ok(());
            }
            _ => {}
        }
        self.meaningful = true;
        Ok(())
    }

    fn should_split_before_word(&self, token: &SqlToken, word: &str) -> bool {
        self.meaningful
            && self.can_split()
            && self.dialect.starts_statement(word)
            && self.at_line_statement_start(token.span.start)
            && self.allows_new_statement_boundary(word)
    }

    /// Rejects apparent line boundaries that are nested in known statement forms.
    fn allows_new_statement_boundary(&self, word: &str) -> bool {
        if self.first_word.as_deref() == Some("WITH") {
            return false;
        }
        if word == "REPLACE" && self.recent_words.ends_with(&["CREATE".into(), "OR".into()]) {
            return false;
        }
        !matches!(
            (
                self.first_word.as_deref(),
                self.create_kind.as_deref(),
                word
            ),
            (
                Some("CREATE"),
                Some("VIEW" | "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT"),
                _
            ) | (Some("INSERT"), _, "SELECT" | "WITH")
        )
    }

    /// Tracks statement kind and procedural body depth from canonical words.
    fn observe_word(&mut self, word: &str) {
        if self.first_word.is_none() {
            self.first_word = Some(word.to_string());
        }
        self.observe_create_kind(word);
        if ((self.dialect.tracks_sqlite_trigger_body()
            && self.create_kind.as_deref() == Some("TRIGGER"))
            || (self.dialect.tracks_mysql_body_blocks()
                && matches!(
                    self.create_kind.as_deref(),
                    Some("FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT")
                )))
            && self.paren_depth == 0
        {
            match word {
                "BEGIN" => self.body_depth += 1,
                "END" if self.body_depth > 0 => self.body_depth -= 1,
                _ => {}
            }
        }
        self.recent_words.push(word.to_string());
        if self.recent_words.len() > 6 {
            self.recent_words.remove(0);
        }
    }

    fn observe_create_kind(&mut self, word: &str) {
        if self.first_word.as_deref() != Some("CREATE") || self.create_kind.is_some() {
            return;
        }
        if !matches!(
            word,
            "CREATE"
                | "OR"
                | "REPLACE"
                | "TEMP"
                | "TEMPORARY"
                | "UNLOGGED"
                | "UNIQUE"
                | "MATERIALIZED"
                | "IF"
                | "NOT"
                | "EXISTS"
        ) {
            self.create_kind = Some(word.to_string());
        }
    }

    /// Consumes a MySQL DELIMITER directive without returning it as SQL.
    fn consume_delimiter_directive(&mut self) -> Result<bool, ParseError> {
        if !self.dialect.supports_delimiter_directive() {
            return Ok(false);
        }
        let token = self.tokens[self.token_index].clone();
        if token.canonical_word() != Some("DELIMITER")
            || !self.at_line_statement_start(token.span.start)
        {
            return Ok(false);
        }
        if self.meaningful {
            self.emit_segment(token.span.start)?;
        }
        let line_end = self.sql[token.span.start..]
            .find('\n')
            .map_or(self.sql.len(), |offset| token.span.start + offset);
        let delimiter = self.sql[token.span.end..line_end].trim();
        if delimiter.is_empty() {
            return Err(ParseError::segment(
                self.dialect,
                token.line,
                token.column,
                "DELIMITER directive requires a delimiter",
            ));
        }
        self.delimiter = delimiter.to_string();
        while self.token_index < self.tokens.len()
            && self.tokens[self.token_index].span.start < line_end
        {
            self.token_index += 1;
        }
        let next = if line_end < self.sql.len() {
            line_end + '\n'.len_utf8()
        } else {
            line_end
        };
        self.reset_segment_start(next);
        Ok(true)
    }

    fn consume_active_delimiter(&mut self) -> Result<bool, ParseError> {
        if self.delimiter == ";" || !self.can_split() {
            return Ok(false);
        }
        let start = self.tokens[self.token_index].span.start;
        if !self.sql[start..].starts_with(&self.delimiter) {
            return Ok(false);
        }
        self.emit_segment(start)?;
        let end = start + self.delimiter.len();
        while self.token_index < self.tokens.len() && self.tokens[self.token_index].span.start < end
        {
            self.token_index += 1;
        }
        self.reset_segment_start(end);
        Ok(true)
    }

    /// Emits an exact source segment and classifies it with the same tokenizer.
    fn emit_segment(&mut self, end: usize) -> Result<(), ParseError> {
        if !self.meaningful {
            return Ok(());
        }
        let Some(end) = trim_trailing_whitespace(self.sql, self.segment_start, end) else {
            return Ok(());
        };
        let start = self.segment_start;
        let start_loc = location_at(self.sql, start);
        let end_loc = location_at(self.sql, end.saturating_sub(1));
        let segment_sql = self.sql[start..end].to_string();
        self.segments.push(SqlSegment {
            ordinal: self.ordinal,
            kind: classify_segment(self.dialect, &segment_sql),
            sql: segment_sql,
            start_byte: start,
            end_byte: end,
            start_line: start_loc.line,
            start_column: start_loc.column,
            end_line: end_loc.line,
            end_column: end_loc.column,
        });
        self.ordinal += 1;
        Ok(())
    }

    /// Validates open lexical structure before emitting the final unterminated segment.
    fn finish_at_eof(&mut self) -> Result<(), ParseError> {
        if self.paren_depth > 0 || self.bracket_depth > 0 || self.brace_depth > 0 {
            let location = location_at(self.sql, self.sql.len());
            return Err(ParseError::segment(
                self.dialect,
                location.line,
                location.column,
                "unbalanced bracket depth at end of SQL",
            ));
        }
        if self.body_depth > 0 {
            let location = location_at(self.sql, self.sql.len());
            return Err(ParseError::segment(
                self.dialect,
                location.line,
                location.column,
                "unterminated statement body at end of SQL",
            ));
        }
        self.emit_segment(self.sql.len())
    }

    fn can_split(&self) -> bool {
        self.paren_depth == 0
            && self.bracket_depth == 0
            && self.brace_depth == 0
            && self.body_depth == 0
    }

    fn reset_segment_start(&mut self, start: usize) {
        self.segment_start = start;
        self.meaningful = false;
        self.first_word = None;
        self.create_kind = None;
        self.recent_words.clear();
        self.body_depth = 0;
    }

    fn at_line_statement_start(&self, byte: usize) -> bool {
        let line_start = self.sql[..byte].rfind('\n').map_or(0, |index| index + 1);
        self.sql[line_start..byte].chars().all(char::is_whitespace)
    }
}

fn location_at(sql: &str, byte: usize) -> Location {
    let mut line = 1usize;
    let mut column = 1usize;
    for (index, ch) in sql.char_indices() {
        if index >= byte {
            break;
        }
        if ch == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    Location::new(line, column)
}

fn trim_trailing_whitespace(sql: &str, start: usize, end: usize) -> Option<usize> {
    let mut trimmed_end = end;
    while trimmed_end > start {
        let ch = sql[start..trimmed_end].chars().next_back()?;
        if !ch.is_whitespace() {
            break;
        }
        trimmed_end -= ch.len_utf8();
    }
    (start < trimmed_end).then_some(trimmed_end)
}
