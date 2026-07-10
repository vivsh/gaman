//! Lexical and non validating statement classification for already-segmented SQL.
//!
//! The classifier is intentionally broad-stroke. It reads dialect tokens at
//! top-level tokens to identify the primary statement kind and object name, but
//! it does not validate SQL grammar, dialect support, or Gaman lowering support.
//! Dialect-specific boundary handling belongs to the segment scanner.
//!
//! CREATE classification logic: CREATE [unknown modifiers]* DETERMINANT [known modifiers]* name
//!

use std::ops::Range;

use super::types::{DdlStatementKind, DmlStatementKind, SqlObjectName, SqlStatementKind};
use crate::dialects::Dialect;
use crate::parsers::tokens::SqlTokenKind;
use crate::states::types::EntityKind;

/// Classifies a segment using only scanner-safe top-level lexical tokens.
pub(super) fn classify_segment(dialect: Dialect, sql: &str) -> Option<SqlStatementKind> {
    let tokens = TokenStream::from_dialect(dialect, sql)?;
    match tokens.first_word()? {
        "CREATE" => classify_create(&tokens),
        "SELECT" => Some(dml(DmlStatementKind::Select)),
        "INSERT" => Some(dml(DmlStatementKind::Insert)),
        "UPDATE" => Some(dml(DmlStatementKind::Update)),
        "DELETE" => Some(dml(DmlStatementKind::Delete)),
        "WITH" => classify_with(&tokens).map(dml),
        _ => None,
    }
}

fn classify_create(tokens: &TokenStream<'_>) -> Option<SqlStatementKind> {
    let determinant = find_entity_determinant(tokens)?;
    match determinant.kind {
        CreateDeterminantKind::Entity(entity) => {
            if entity == EntityKind::View
                && (0..determinant.index).any(|idx| tokens.word(idx) == Some("MATERIALIZED"))
            {
                return None;
            }
            let name_idx = skip_name_modifiers(tokens, entity, determinant.index + 1);
            let (name, _) = tokens.object_name_at(name_idx)?;
            let owner = owner_for_entity(tokens, entity, name_idx + 1);
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity,
                name: Some(name),
                owner,
            }))
        }
        CreateDeterminantKind::Type => {
            let (name, next_idx) = tokens.object_name_at(determinant.index + 1)?;
            if !has_as_enum_after_name(tokens, next_idx) {
                return None;
            }
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Enum,
                name: Some(name),
                owner: None,
            }))
        }
    }
}

fn owner_for_entity(
    tokens: &TokenStream<'_>,
    entity: EntityKind,
    start_idx: usize,
) -> Option<SqlObjectName> {
    match entity {
        EntityKind::Index | EntityKind::Trigger => object_name_after_word(tokens, start_idx, "ON"),
        _ => None,
    }
}

fn object_name_after_word(
    tokens: &TokenStream<'_>,
    start_idx: usize,
    target: &str,
) -> Option<SqlObjectName> {
    let mut idx = start_idx;
    while let Some(token) = tokens.item(idx) {
        if matches!(token, TopLevelToken::Word { upper, .. } if upper == target) {
            return tokens.object_name_at(idx + 1).map(|(name, _)| name);
        }
        idx += 1;
    }
    None
}

fn find_entity_determinant(tokens: &TokenStream<'_>) -> Option<EntityDeterminant> {
    let mut idx = 1usize;
    while let Some(word) = tokens.word(idx) {
        if word == "EVENT" && tokens.word(idx + 1) == Some("TRIGGER") {
            return None;
        }
        if let Some(kind) = determinant_for_word(word) {
            return Some(EntityDeterminant::new(kind, idx));
        }
        idx += 1;
    }
    None
}

fn determinant_for_word(word: &str) -> Option<CreateDeterminantKind> {
    match word {
        "TABLE" => Some(CreateDeterminantKind::Entity(EntityKind::Table)),
        "INDEX" => Some(CreateDeterminantKind::Entity(EntityKind::Index)),
        "VIEW" => Some(CreateDeterminantKind::Entity(EntityKind::View)),
        "FUNCTION" => Some(CreateDeterminantKind::Entity(EntityKind::Function)),
        "TRIGGER" => Some(CreateDeterminantKind::Entity(EntityKind::Trigger)),
        "EXTENSION" => Some(CreateDeterminantKind::Entity(EntityKind::Extension)),
        "TYPE" => Some(CreateDeterminantKind::Type),
        _ => None,
    }
}

fn skip_name_modifiers(tokens: &TokenStream<'_>, entity: EntityKind, mut idx: usize) -> usize {
    loop {
        match (entity, tokens.word(idx)) {
            (EntityKind::Index, Some("CONCURRENTLY")) => idx += 1,
            (_, Some("IF"))
                if tokens.word(idx + 1) == Some("NOT")
                    && tokens.word(idx + 2) == Some("EXISTS") =>
            {
                idx += 3;
            }
            _ => return idx,
        }
    }
}

fn has_as_enum_after_name(tokens: &TokenStream<'_>, idx: usize) -> bool {
    tokens.word(idx) == Some("AS") && tokens.word(idx + 1) == Some("ENUM")
}

fn classify_with(tokens: &TokenStream<'_>) -> Option<DmlStatementKind> {
    let mut idx = 1usize;
    if tokens.word(idx) == Some("RECURSIVE") {
        idx += 1;
    }

    loop {
        tokens.name_atom(idx)?;
        idx += 1;

        if matches!(tokens.item(idx), Some(TopLevelToken::Group)) {
            idx += 1;
        }

        if tokens.word(idx) != Some("AS") {
            return None;
        }
        idx += 1;

        if tokens.word(idx) == Some("NOT") && tokens.word(idx + 1) == Some("MATERIALIZED") {
            idx += 2;
        } else if tokens.word(idx) == Some("MATERIALIZED") {
            idx += 1;
        }

        match tokens.item(idx) {
            Some(TopLevelToken::Group) => idx += 1,
            _ => return None,
        }

        match tokens.item(idx) {
            Some(TopLevelToken::Comma) => idx += 1,
            Some(TopLevelToken::Word { upper, .. }) => {
                return dml_kind_word(upper).map(|kind| DmlStatementKind::With(Box::new(kind)));
            }
            _ => return None,
        }
    }
}

fn dml_kind_word(word: &str) -> Option<DmlStatementKind> {
    match word {
        "SELECT" => Some(DmlStatementKind::Select),
        "INSERT" => Some(DmlStatementKind::Insert),
        "UPDATE" => Some(DmlStatementKind::Update),
        "DELETE" => Some(DmlStatementKind::Delete),
        _ => None,
    }
}

fn dml(kind: DmlStatementKind) -> SqlStatementKind {
    SqlStatementKind::Dml(kind)
}

#[derive(Debug, Clone, Copy)]
struct EntityDeterminant {
    kind: CreateDeterminantKind,
    index: usize,
}

impl EntityDeterminant {
    fn new(kind: CreateDeterminantKind, index: usize) -> Self {
        Self { kind, index }
    }
}

#[derive(Debug, Clone, Copy)]
enum CreateDeterminantKind {
    Entity(EntityKind),
    Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TopLevelToken {
    Word {
        upper: String,
        span: Range<usize>,
    },
    QuotedName {
        unquoted: String,
        span: Range<usize>,
    },
    Dot,
    Comma,
    Group,
}

struct TokenStream<'a> {
    sql: &'a str,
    tokens: Vec<TopLevelToken>,
}

impl<'a> TokenStream<'a> {
    fn first_word(&self) -> Option<&str> {
        self.tokens.iter().find_map(|token| match token {
            TopLevelToken::Word { upper, .. } => Some(upper.as_str()),
            _ => None,
        })
    }

    fn item(&self, idx: usize) -> Option<&TopLevelToken> {
        self.tokens.get(idx)
    }

    fn word(&self, idx: usize) -> Option<&str> {
        match self.tokens.get(idx) {
            Some(TopLevelToken::Word { upper, .. }) => Some(upper.as_str()),
            _ => None,
        }
    }

    fn name_atom(&self, idx: usize) -> Option<NameAtom<'_>> {
        match self.tokens.get(idx) {
            Some(TopLevelToken::Word { span, .. }) => Some(NameAtom {
                raw: &self.sql[span.clone()],
                part: self.sql[span.clone()].to_string(),
            }),
            Some(TopLevelToken::QuotedName { unquoted, span }) => Some(NameAtom {
                raw: &self.sql[span.clone()],
                part: unquoted.clone(),
            }),
            _ => None,
        }
    }

    fn object_name_at(&self, idx: usize) -> Option<(SqlObjectName, usize)> {
        let first = self.name_atom(idx)?;
        let mut raw_parts = vec![first.raw.to_string()];
        let mut parts = vec![first.part];
        let mut next = idx + 1;

        while matches!(self.item(next), Some(TopLevelToken::Dot)) {
            let part = self.name_atom(next + 1)?;
            raw_parts.push(part.raw.to_string());
            parts.push(part.part);
            next += 2;
        }

        Some((
            SqlObjectName {
                raw: raw_parts.join("."),
                parts,
            },
            next,
        ))
    }
}

struct NameAtom<'a> {
    raw: &'a str,
    part: String,
}

impl<'a> TokenStream<'a> {
    /// Projects shared dialect tokens into the classifier's top-level view.
    fn from_dialect(dialect: Dialect, sql: &'a str) -> Option<Self> {
        let source = dialect.tokenizer().tokenize(sql).ok()?;
        let mut tokens = Vec::new();
        let mut depth = 0usize;
        for token in source {
            match token.kind {
                SqlTokenKind::LeftParen | SqlTokenKind::LeftBracket | SqlTokenKind::LeftBrace => {
                    if depth == 0 {
                        tokens.push(TopLevelToken::Group);
                    }
                    depth += 1;
                }
                SqlTokenKind::RightParen
                | SqlTokenKind::RightBracket
                | SqlTokenKind::RightBrace => depth = depth.saturating_sub(1),
                _ if depth > 0 || token.is_trivia() => {}
                SqlTokenKind::Word { canonical, .. } => tokens.push(TopLevelToken::Word {
                    upper: canonical,
                    span: token.span,
                }),
                SqlTokenKind::QuotedIdentifier { value, .. } => {
                    tokens.push(TopLevelToken::QuotedName {
                        unquoted: value,
                        span: token.span,
                    });
                }
                SqlTokenKind::Dot => tokens.push(TopLevelToken::Dot),
                SqlTokenKind::Comma => tokens.push(TopLevelToken::Comma),
                _ => {}
            }
        }
        Some(Self { sql, tokens })
    }
}
