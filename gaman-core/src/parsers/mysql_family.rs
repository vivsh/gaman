//! Shared AST extraction for column options common to MySQL-family syntax.

use sqlparser::ast::{ColumnOption, CreateTable};

use crate::dialects::Dialect;
use crate::states::{ColumnDialectOptions, MariadbColumnOptions, MysqlColumnOptions, Table};

/// Applies product-owned column options after common table structure is lowered.
pub(super) fn apply_column_options(table: &CreateTable, lowered: &mut Table, dialect: Dialect) {
    for (definition, column) in table.columns.iter().zip(&mut lowered.columns) {
        let values = extracted_options(&definition.options);
        column.generated_storage = values.generated_storage;
        column.dialect_options = if values.is_empty() {
            ColumnDialectOptions::default()
        } else {
            match dialect {
                Dialect::Mysql => ColumnDialectOptions {
                    mysql: Some(values.into_mysql()),
                    mariadb: None,
                },
                Dialect::Mariadb => ColumnDialectOptions {
                    mysql: None,
                    mariadb: Some(values.into_mariadb()),
                },
                _ => ColumnDialectOptions::default(),
            }
        };
    }
}

#[derive(Default)]
struct ExtractedOptions {
    auto_increment: bool,
    on_update_expression: Option<String>,
    character_set: Option<String>,
    collation: Option<String>,
    invisible: bool,
    comment: Option<String>,
    generated_storage: Option<crate::states::GeneratedStorage>,
}

impl ExtractedOptions {
    fn is_empty(&self) -> bool {
        !self.auto_increment
            && self.on_update_expression.is_none()
            && self.character_set.is_none()
            && self.collation.is_none()
            && !self.invisible
            && self.comment.is_none()
    }
    fn into_mysql(self) -> MysqlColumnOptions {
        MysqlColumnOptions {
            auto_increment: self.auto_increment,
            on_update_expression: self.on_update_expression,
            character_set: self.character_set,
            collation: self.collation,
            invisible: self.invisible,
            comment: self.comment,
        }
    }

    fn into_mariadb(self) -> MariadbColumnOptions {
        MariadbColumnOptions {
            auto_increment: self.auto_increment,
            on_update_expression: self.on_update_expression,
            character_set: self.character_set,
            collation: self.collation,
            invisible: self.invisible,
            comment: self.comment,
        }
    }
}

/// Extracts exact option tokens without substring matching.
fn extracted_options(options: &[sqlparser::ast::ColumnOptionDef]) -> ExtractedOptions {
    let mut result = ExtractedOptions::default();
    for definition in options {
        match &definition.option {
            ColumnOption::CharacterSet(name) => result.character_set = Some(name.to_string()),
            ColumnOption::Collation(name) => result.collation = Some(name.to_string()),
            ColumnOption::Comment(value) => result.comment = Some(value.clone()),
            ColumnOption::OnUpdate(value) => result.on_update_expression = Some(value.to_string()),
            ColumnOption::Generated {
                generation_expr_mode,
                ..
            } => {
                result.generated_storage = generation_expr_mode.as_ref().map(|mode| match mode {
                    sqlparser::ast::GeneratedExpressionMode::Stored => {
                        crate::states::GeneratedStorage::Stored
                    }
                    sqlparser::ast::GeneratedExpressionMode::Virtual => {
                        crate::states::GeneratedStorage::Virtual
                    }
                });
            }
            ColumnOption::Invisible => result.invisible = true,
            ColumnOption::DialectSpecific(tokens) => {
                let words = tokens.iter().map(ToString::to_string).collect::<Vec<_>>();
                result.auto_increment |= words
                    .iter()
                    .any(|word| word.eq_ignore_ascii_case("AUTO_INCREMENT"));
                result.invisible |= words
                    .iter()
                    .any(|word| word.eq_ignore_ascii_case("INVISIBLE"));
            }
            _ => {}
        }
    }
    result
}

/// Restores exact native type text that the AST normalizes or cannot represent.
pub(super) fn preserve_native_types(sql: &str, table: &mut Table, dialect: Dialect) {
    let Ok(tokens) = dialect.tokenizer().tokenize(sql) else {
        return;
    };
    let Some(open) = tokens
        .iter()
        .position(|token| matches!(token.kind, crate::parsers::tokens::SqlTokenKind::LeftParen))
    else {
        return;
    };
    let mut depth = 0usize;
    let mut clause_start = open + 1;
    let mut clauses = Vec::new();
    for (index, token) in tokens.iter().enumerate().skip(open + 1) {
        match token.kind {
            crate::parsers::tokens::SqlTokenKind::LeftParen => depth += 1,
            crate::parsers::tokens::SqlTokenKind::RightParen if depth == 0 => {
                clauses.push((clause_start, index));
                break;
            }
            crate::parsers::tokens::SqlTokenKind::RightParen => depth = depth.saturating_sub(1),
            crate::parsers::tokens::SqlTokenKind::Comma if depth == 0 => {
                clauses.push((clause_start, index));
                clause_start = index + 1;
            }
            _ => {}
        }
    }
    let mut column_index = 0usize;
    for (start, end) in clauses {
        let significant = tokens[start..end]
            .iter()
            .enumerate()
            .filter(|(_, token)| !token.is_trivia())
            .map(|(offset, _)| start + offset)
            .collect::<Vec<_>>();
        if significant.len() < 2 || is_table_clause(&tokens[significant[0]]) {
            continue;
        }
        let type_start = significant[1];
        let type_end = significant
            .iter()
            .copied()
            .skip(1)
            .find(|index| is_column_option(&tokens[*index]))
            .unwrap_or(end);
        if type_start < type_end && column_index < table.columns.len() {
            table.columns[column_index].col_type = sql
                [tokens[type_start].span.start..tokens[type_end - 1].span.end]
                .trim()
                .to_string();
        }
        column_index += 1;
    }
}

fn is_table_clause(token: &crate::parsers::tokens::SqlToken) -> bool {
    matches!(
        token.canonical_word(),
        Some(
            "CONSTRAINT"
                | "PRIMARY"
                | "FOREIGN"
                | "UNIQUE"
                | "CHECK"
                | "KEY"
                | "INDEX"
                | "FULLTEXT"
                | "SPATIAL"
        )
    )
}

fn is_column_option(token: &crate::parsers::tokens::SqlToken) -> bool {
    matches!(
        token.canonical_word(),
        Some(
            "NULL"
                | "NOT"
                | "DEFAULT"
                | "PRIMARY"
                | "UNIQUE"
                | "REFERENCES"
                | "CHECK"
                | "GENERATED"
                | "AS"
                | "AUTO_INCREMENT"
                | "ON"
                | "CHARACTER"
                | "COLLATE"
                | "COMMENT"
                | "INVISIBLE"
                | "VISIBLE"
        )
    )
}
