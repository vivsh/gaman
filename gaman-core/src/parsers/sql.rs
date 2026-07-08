use sqlparser::ast::Statement;
use sqlparser::dialect::{PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use super::error::ParseError;
use super::segments::segment_sql;
use super::{postgres, sqlite};
use crate::dialects::Dialect;
use crate::states::{Index, Schema, Table};

pub(crate) struct ParseContext {
    pub(crate) schema: Schema,
    pending_indexes: Vec<(String, Index)>,
}

impl ParseContext {
    pub(crate) fn new() -> Self {
        Self {
            schema: Schema::default(),
            pending_indexes: Vec::new(),
        }
    }

    pub(crate) fn insert_table(&mut self, table: (String, Table)) -> Result<(), ParseError> {
        let (key, table) = table;
        if self.schema.tables.contains_key(&key) {
            return Err(ParseError::DuplicateTable(key));
        }
        self.schema.tables.insert(key, table);
        Ok(())
    }

    pub(crate) fn push_index(&mut self, index: (String, Index)) {
        self.pending_indexes.push(index);
    }

    fn finish(mut self) -> Result<Schema, ParseError> {
        for (table_name, index) in self.pending_indexes {
            match self.schema.tables.get_mut(&table_name) {
                Some(table) => table.indexes.push(index),
                None => return Err(ParseError::UnknownTable { table: table_name }),
            }
        }
        self.schema.normalize();
        Ok(self.schema)
    }
}

/// Parses PostgreSQL SQL DDL into a Gaman schema.
pub fn parse_sql(sql: &str) -> Result<Schema, ParseError> {
    parse_sql_for_dialect(sql, Dialect::Postgres)
}

/// Parses SQL DDL for a supported dialect into a Gaman schema.
pub fn parse_sql_for_dialect(sql: &str, dialect: Dialect) -> Result<Schema, ParseError> {
    let segments = segment_sql(sql, dialect)?;
    if matches!(dialect, Dialect::Mysql) {
        return Err(ParseError::UnsupportedDialect("mysql".to_string()));
    }

    let mut ctx = ParseContext::new();
    for segment in segments {
        let statements = match dialect {
            Dialect::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, &segment.sql),
            Dialect::Sqlite => Parser::parse_sql(&SQLiteDialect {}, &segment.sql),
            Dialect::Mysql => unreachable!("mysql returned above"),
        }
        .map_err(|error| ParseError::parse(dialect, error.to_string()))?;
        for stmt in &statements {
            ensure_modeled_create_statement(stmt, dialect)?;
            match dialect {
                Dialect::Postgres => postgres::lower_statement(stmt, &mut ctx)?,
                Dialect::Sqlite => sqlite::lower_statement(stmt, &mut ctx)?,
                Dialect::Mysql => unreachable!("mysql returned above"),
            }
        }
    }
    ctx.finish()
}

fn ensure_modeled_create_statement(stmt: &Statement, dialect: Dialect) -> Result<(), ParseError> {
    if is_modeled_create_statement(stmt) {
        return Ok(());
    }
    Err(ParseError::unsupported(
        dialect,
        stmt.to_string(),
        "only CREATE statements for modeled schema entities are parsed",
    ))
}

fn is_modeled_create_statement(stmt: &Statement) -> bool {
    matches!(
        stmt,
        Statement::CreateExtension(_)
            | Statement::CreateFunction(_)
            | Statement::CreateIndex(_)
            | Statement::CreateTable(_)
            | Statement::CreateTrigger(_)
            | Statement::CreateType { .. }
            | Statement::CreateView(_)
    )
}
