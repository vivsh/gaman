use super::classifier::classify_segment;
use super::types::{Location, SqlSegment, SqlSegmentationExt};
use crate::dialects::Dialect;
use crate::parsers::ParseError;

/// Segments SQL text into raw statements for the selected dialect.
pub fn segment_sql(sql: &str, dialect: Dialect) -> Result<Vec<SqlSegment>, ParseError> {
    Scanner::new(sql, dialect).scan()
}

struct Scanner<'a> {
    sql: &'a str,
    dialect: Dialect,
    pos: usize,
    line: usize,
    column: usize,
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
    fn new(sql: &'a str, dialect: Dialect) -> Self {
        Self {
            sql,
            dialect,
            pos: 0,
            line: 1,
            column: 1,
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

    fn scan(mut self) -> Result<Vec<SqlSegment>, ParseError> {
        while let Some(ch) = self.current_char() {
            if self.consume_delimiter_directive()? || self.consume_active_delimiter()? {
                continue;
            }
            match ch {
                c if c.is_whitespace() => self.advance_char(),
                '-' if self.starts_with("--") => self.consume_line_comment(),
                '#' if self.dialect.supports_hash_comments() => self.consume_line_comment(),
                '/' if self.starts_with("/*") => self.consume_block_comment()?,
                '\'' => self.consume_quoted('\'', "single-quoted string")?,
                '"' => self.consume_quoted('"', "double-quoted identifier")?,
                '`' if self.dialect.supports_backtick_quotes() => {
                    self.consume_quoted('`', "backtick identifier")?
                }
                '$' if self.dialect.supports_dollar_quotes() && self.dollar_tag().is_some() => {
                    self.consume_dollar_quote()?
                }
                '(' => self.bump_depth(DepthKind::Paren),
                ')' => self.drop_depth(DepthKind::Paren),
                '[' => self.bump_depth(DepthKind::Bracket),
                ']' => self.drop_depth(DepthKind::Bracket),
                '{' => self.bump_depth(DepthKind::Brace),
                '}' => self.drop_depth(DepthKind::Brace),
                ';' if self.delimiter == ";" && self.can_split_at_terminator() => {
                    self.emit_segment(self.pos)?;
                    self.advance_char();
                    self.reset_segment_start();
                }
                c if is_ident_start(c) => self.consume_word_or_boundary()?,
                _ => {
                    self.mark_meaningful();
                    self.advance_char();
                }
            }
        }
        self.finish_at_eof()?;
        Ok(self.segments)
    }

    fn consume_word_or_boundary(&mut self) -> Result<(), ParseError> {
        let word = self.peek_word().to_ascii_uppercase();
        if self.should_split_before_word(&word) {
            self.emit_segment(self.pos)?;
            self.reset_segment_start();
            return Ok(());
        }
        self.mark_meaningful();
        self.advance_word();
        self.observe_word(&word);
        Ok(())
    }

    fn should_split_before_word(&self, word: &str) -> bool {
        self.meaningful
            && self.can_split_at_terminator()
            && self.dialect.is_statement_start(word)
            && self.at_line_statement_start()
            && self.allows_new_statement_boundary(word)
    }

    fn allows_new_statement_boundary(&self, word: &str) -> bool {
        if self.first_word.as_deref() == Some("WITH") {
            return false;
        }
        if word == "REPLACE" && self.recent_words.ends_with(&["CREATE".into(), "OR".into()]) {
            return false;
        }
        match (
            self.first_word.as_deref(),
            self.create_kind.as_deref(),
            word,
        ) {
            (Some("CREATE"), Some("VIEW" | "FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT"), _) => {
                false
            }
            (Some("INSERT"), _, "SELECT" | "WITH") => false,
            _ => true,
        }
    }

    fn observe_word(&mut self, word: &str) {
        if self.first_word.is_none() {
            self.first_word = Some(word.to_string());
        }
        self.observe_create_kind(word);
        self.observe_body_word(word);
        self.recent_words.push(word.to_string());
        if self.recent_words.len() > 6 {
            self.recent_words.remove(0);
        }
    }

    fn observe_create_kind(&mut self, word: &str) {
        if self.first_word.as_deref() != Some("CREATE") || self.create_kind.is_some() {
            return;
        }
        if matches!(
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
            return;
        }
        self.create_kind = Some(word.to_string());
    }

    fn observe_body_word(&mut self, word: &str) {
        if self.dialect.tracks_sqlite_trigger_body()
            && self.create_kind.as_deref() == Some("TRIGGER")
        {
            self.update_begin_end_depth(word);
        }
        if self.dialect.tracks_mysql_body_blocks()
            && matches!(
                self.create_kind.as_deref(),
                Some("FUNCTION" | "PROCEDURE" | "TRIGGER" | "EVENT")
            )
        {
            self.update_begin_end_depth(word);
        }
    }

    fn update_begin_end_depth(&mut self, word: &str) {
        match word {
            "BEGIN" => self.body_depth += 1,
            "END" if self.body_depth > 0 => self.body_depth -= 1,
            _ => {}
        }
    }

    fn consume_delimiter_directive(&mut self) -> Result<bool, ParseError> {
        if !self.dialect.supports_delimiter_directive() || !self.at_line_statement_start() {
            return Ok(false);
        }
        let Some(line) = self.remaining_line() else {
            return Ok(false);
        };
        let trimmed = line.trim_start();
        if !starts_with_word_ci(trimmed, "DELIMITER") {
            return Ok(false);
        }
        if self.meaningful {
            self.emit_segment(self.pos)?;
        }
        let delimiter = trimmed["DELIMITER".len()..].trim();
        if delimiter.is_empty() {
            return Err(self.error_here("DELIMITER directive requires a delimiter"));
        }
        self.delimiter = delimiter.to_string();
        self.advance_to_line_end();
        self.reset_segment_start();
        Ok(true)
    }

    fn consume_active_delimiter(&mut self) -> Result<bool, ParseError> {
        if self.delimiter == ";" || !self.can_split_at_terminator() {
            return Ok(false);
        }
        if !self.sql[self.pos..].starts_with(&self.delimiter) {
            return Ok(false);
        }
        self.emit_segment(self.pos)?;
        let end = self.pos + self.delimiter.len();
        self.advance_to(end);
        self.reset_segment_start();
        Ok(true)
    }

    fn consume_line_comment(&mut self) {
        while let Some(ch) = self.current_char() {
            self.advance_char();
            if ch == '\n' {
                break;
            }
        }
    }

    fn consume_block_comment(&mut self) -> Result<(), ParseError> {
        let start = self.location();
        self.advance_to(self.pos + 2);
        let mut depth = 1usize;
        while self.current_char().is_some() {
            if self.starts_with("/*") && self.dialect.supports_nested_block_comments() {
                depth += 1;
                self.advance_to(self.pos + 2);
            } else if self.starts_with("*/") {
                depth -= 1;
                self.advance_to(self.pos + 2);
                if depth == 0 {
                    return Ok(());
                }
            } else {
                self.advance_char();
            }
        }
        Err(self.error_at(start, "unterminated block comment"))
    }

    fn consume_quoted(&mut self, quote: char, label: &str) -> Result<(), ParseError> {
        let start = self.location();
        self.mark_meaningful();
        self.advance_char();
        while let Some(ch) = self.current_char() {
            self.advance_char();
            if ch == quote {
                if self.current_char() == Some(quote) {
                    self.advance_char();
                } else {
                    return Ok(());
                }
            } else if ch == '\\' && self.dialect.supports_backtick_quotes() {
                self.advance_char();
            }
        }
        Err(self.error_at(start, format!("unterminated {label}")))
    }

    fn consume_dollar_quote(&mut self) -> Result<(), ParseError> {
        let start = self.location();
        let tag = self.dollar_tag().expect("checked by caller");
        self.mark_meaningful();
        self.advance_to(self.pos + tag.len());
        if let Some(offset) = self.sql[self.pos..].find(&tag) {
            self.advance_to(self.pos + offset + tag.len());
            Ok(())
        } else {
            Err(self.error_at(start, "unterminated dollar-quoted string"))
        }
    }

    fn emit_segment(&mut self, end: usize) -> Result<(), ParseError> {
        if !self.meaningful {
            return Ok(());
        }
        let Some(end) = trim_trailing_whitespace(self.sql, self.segment_start, end) else {
            return Ok(());
        };
        let start = self.segment_start;
        let start_loc = self.location_at(start);
        let end_loc = self.location_at(end.saturating_sub(1));
        let segment_sql = self.sql[start..end].to_string();
        self.segments.push(SqlSegment {
            ordinal: self.ordinal,
            kind: classify_segment(&segment_sql),
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

    fn finish_at_eof(&mut self) -> Result<(), ParseError> {
        if self.paren_depth > 0 || self.bracket_depth > 0 || self.brace_depth > 0 {
            return Err(self.error_here("unbalanced bracket depth at end of SQL"));
        }
        if self.body_depth > 0 {
            return Err(self.error_here("unterminated statement body at end of SQL"));
        }
        self.emit_segment(self.sql.len())
    }

    fn can_split_at_terminator(&self) -> bool {
        self.paren_depth == 0
            && self.bracket_depth == 0
            && self.brace_depth == 0
            && self.body_depth == 0
    }

    fn bump_depth(&mut self, kind: DepthKind) {
        self.mark_meaningful();
        match kind {
            DepthKind::Paren => self.paren_depth += 1,
            DepthKind::Bracket => self.bracket_depth += 1,
            DepthKind::Brace => self.brace_depth += 1,
        }
        self.advance_char();
    }

    fn drop_depth(&mut self, kind: DepthKind) {
        self.mark_meaningful();
        match kind {
            DepthKind::Paren if self.paren_depth > 0 => self.paren_depth -= 1,
            DepthKind::Bracket if self.bracket_depth > 0 => self.bracket_depth -= 1,
            DepthKind::Brace if self.brace_depth > 0 => self.brace_depth -= 1,
            _ => {}
        }
        self.advance_char();
    }

    fn mark_meaningful(&mut self) {
        self.meaningful = true;
    }

    fn reset_segment_start(&mut self) {
        self.segment_start = self.pos;
        self.meaningful = false;
        self.first_word = None;
        self.create_kind = None;
        self.recent_words.clear();
        self.body_depth = 0;
    }

    fn at_line_statement_start(&self) -> bool {
        let line_start = self.sql[..self.pos]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0);
        self.sql[line_start..self.pos]
            .chars()
            .all(char::is_whitespace)
    }

    fn remaining_line(&self) -> Option<&'a str> {
        self.sql[self.pos..]
            .split_once('\n')
            .map(|(line, _)| line)
            .or_else(|| Some(&self.sql[self.pos..]))
    }

    fn advance_to_line_end(&mut self) {
        while let Some(ch) = self.current_char() {
            self.advance_char();
            if ch == '\n' {
                break;
            }
        }
    }

    fn peek_word(&self) -> &'a str {
        let mut end = self.pos;
        for (offset, ch) in self.sql[self.pos..].char_indices() {
            if offset == 0 {
                end = self.pos + ch.len_utf8();
                continue;
            }
            if is_ident_continue(ch) {
                end = self.pos + offset + ch.len_utf8();
            } else {
                break;
            }
        }
        &self.sql[self.pos..end]
    }

    fn advance_word(&mut self) {
        while let Some(ch) = self.current_char() {
            if is_ident_continue(ch) {
                self.advance_char();
            } else {
                break;
            }
        }
    }

    fn dollar_tag(&self) -> Option<String> {
        let rest = &self.sql[self.pos..];
        if !rest.starts_with('$') {
            return None;
        }
        let mut chars = rest.char_indices().skip(1);
        for (idx, ch) in &mut chars {
            if ch == '$' {
                return Some(rest[..idx + 1].to_string());
            }
            if !(ch == '_' || ch.is_ascii_alphanumeric()) {
                return None;
            }
        }
        None
    }

    fn starts_with(&self, value: &str) -> bool {
        self.sql[self.pos..].starts_with(value)
    }

    fn current_char(&self) -> Option<char> {
        self.sql[self.pos..].chars().next()
    }

    fn advance_char(&mut self) {
        let Some(ch) = self.current_char() else {
            return;
        };
        self.pos += ch.len_utf8();
        if ch == '\n' {
            self.line += 1;
            self.column = 1;
        } else {
            self.column += 1;
        }
    }

    fn advance_to(&mut self, end: usize) {
        while self.pos < end {
            self.advance_char();
        }
    }

    fn location(&self) -> Location {
        Location::new(self.line, self.column)
    }

    fn location_at(&self, byte: usize) -> Location {
        let mut line = 1usize;
        let mut column = 1usize;
        for (idx, ch) in self.sql.char_indices() {
            if idx >= byte {
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

    fn error_here(&self, reason: impl Into<String>) -> ParseError {
        self.error_at(self.location(), reason)
    }

    fn error_at(&self, location: Location, reason: impl Into<String>) -> ParseError {
        ParseError::segment(self.dialect, location.line, location.column, reason)
    }
}

#[derive(Debug, Clone, Copy)]
enum DepthKind {
    Paren,
    Bracket,
    Brace,
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

fn starts_with_word_ci(value: &str, word: &str) -> bool {
    value.len() >= word.len()
        && value[..word.len()].eq_ignore_ascii_case(word)
        && value[word.len()..]
            .chars()
            .next()
            .is_none_or(|ch| ch.is_whitespace())
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
