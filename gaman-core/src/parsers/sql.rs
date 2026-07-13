use sqlparser::ast::Statement;
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use super::error::ParseError;
use super::segments::{SqlObjectName, SqlSegment, SqlStatementKind, segment_sql};
use super::table_recovery::recover_table_sql;
use super::{postgres, sqlite};
use crate::dialects::Dialect;
use crate::states::types::EntityKind;
use crate::states::{
    ExtensionDef, FunctionDef, Index, Schema, Table, TriggerDef, ViewDef, schema_qualified_key,
};

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

    /// Preserves the authored source for an index that lowering classified as opaque.
    fn preserve_opaque_index_source(&mut self, raw: &str) {
        let Some((_, index)) = self.pending_indexes.last_mut() else {
            return;
        };
        if index.is_opaque() {
            *index = Index::from_raw(index.name.clone(), raw.to_string());
        }
    }

    fn finish(self) -> Result<Schema, ParseError> {
        self.finish_raw()
    }

    fn finish_raw(mut self) -> Result<Schema, ParseError> {
        for (table_name, index) in self.pending_indexes {
            match self.schema.tables.get_mut(&table_name) {
                Some(table) => table.indexes.push(index),
                None => return Err(ParseError::UnknownTable { table: table_name }),
            }
        }
        Ok(self.schema)
    }
}

/// Parses SQL DDL for a supported dialect into a Gaman schema.
pub fn parse_sql(sql: &str, dialect: Dialect) -> Result<Schema, ParseError> {
    Ok(parse_sql_raw(sql, dialect)?.prepare(dialect)?)
}

pub(crate) fn parse_sql_raw(sql: &str, dialect: Dialect) -> Result<Schema, ParseError> {
    let segments = segment_sql(sql, dialect)?;
    let mut ctx = ParseContext::new();
    for segment in segments {
        ensure_schema_segment(&segment, dialect)?;
        let statements = match parse_segment(&segment.sql, dialect) {
            Ok(statements) => statements,
            Err(error) => {
                if recover_modeled_table(&segment, &mut ctx, dialect)? {
                    continue;
                }
                if lower_raw_segment(&segment, &mut ctx, dialect).is_ok() {
                    continue;
                }
                return Err(ParseError::parse_in_segment(dialect, &segment, error));
            }
        };
        for stmt in &statements {
            ensure_modeled_create_statement(stmt, dialect)?;
            let lowered = match dialect {
                Dialect::Postgres => postgres::lower_statement(stmt, &mut ctx),
                Dialect::Sqlite => sqlite::lower_statement(stmt, &mut ctx),
                Dialect::Mysql => super::mysql::lower_statement(stmt, &mut ctx, dialect),
                Dialect::Mariadb => super::mariadb::lower_statement(stmt, &mut ctx),
            };
            if let Err(error) = lowered {
                if matches!(segment.kind, Some(SqlStatementKind::Ddl(ref ddl)) if ddl.entity == EntityKind::Table)
                {
                    return Err(error);
                }
                lower_raw_segment(&segment, &mut ctx, dialect)?;
            } else {
                preserve_family_table_types(&segment, &mut ctx, dialect);
            }
            if matches!(
                segment.kind,
                Some(SqlStatementKind::Ddl(ref ddl)) if ddl.entity == EntityKind::Index
            ) {
                ctx.preserve_opaque_index_source(&segment.sql);
            }
        }
    }
    ctx.finish()
}

/// Rejects statements outside the closed CREATE EntityKind boundary before AST parsing.
fn ensure_schema_segment(segment: &SqlSegment, dialect: Dialect) -> Result<(), ParseError> {
    if matches!(segment.kind, Some(SqlStatementKind::Ddl(_))) {
        return Ok(());
    }
    Err(ParseError::unsupported(
        dialect,
        segment.sql.clone(),
        "schema SQL must be a CREATE statement for a known Gaman entity kind",
    ))
}

/// Parses one segment with the selected private SQL parser dialect.
fn parse_segment(
    sql: &str,
    dialect: Dialect,
) -> Result<Vec<Statement>, sqlparser::parser::ParserError> {
    match dialect {
        Dialect::Postgres => Parser::parse_sql(&PostgreSqlDialect {}, sql),
        Dialect::Sqlite => Parser::parse_sql(&SQLiteDialect {}, sql),
        Dialect::Mariadb => Parser::parse_sql(&MySqlDialect {}, sql),
        Dialect::Mysql => Parser::parse_sql(&MySqlDialect {}, sql),
    }
}

/// Recovers a modeled table while preserving unsupported outer syntax as metadata.
fn recover_modeled_table(
    segment: &SqlSegment,
    ctx: &mut ParseContext,
    dialect: Dialect,
) -> Result<bool, ParseError> {
    let Some(SqlStatementKind::Ddl(ddl)) = &segment.kind else {
        return Ok(false);
    };
    if ddl.entity != EntityKind::Table {
        return Ok(false);
    }
    let Some(recovered) = recover_table_sql(&segment.sql, dialect) else {
        return Ok(false);
    };
    let Ok(statements) = parse_segment(&recovered.core_sql, dialect) else {
        return Ok(false);
    };
    for statement in &statements {
        ensure_modeled_create_statement(statement, dialect)?;
        match dialect {
            Dialect::Postgres => postgres::lower_statement(statement, ctx)?,
            Dialect::Sqlite => sqlite::lower_statement(statement, ctx)?,
            Dialect::Mysql => super::mysql::lower_statement(statement, ctx, dialect)?,
            Dialect::Mariadb => super::mariadb::lower_statement(statement, ctx)?,
        }
    }
    attach_recovered_options(ddl.name.as_ref(), recovered, ctx);
    preserve_family_table_types(segment, ctx, dialect);
    Ok(true)
}

fn preserve_family_table_types(segment: &SqlSegment, ctx: &mut ParseContext, dialect: Dialect) {
    if !matches!(dialect, Dialect::Mysql | Dialect::Mariadb) {
        return;
    }
    let Some(SqlStatementKind::Ddl(ddl)) = &segment.kind else {
        return;
    };
    if ddl.entity != EntityKind::Table {
        return;
    }
    let Some(name) = &ddl.name else {
        return;
    };
    let (name, schema) = object_name_parts(name);
    let key = crate::states::schema_qualified_key(&name, schema.as_deref());
    if let Some(table) = ctx.schema.tables.get_mut(&key) {
        super::mysql_family::preserve_native_types(&segment.sql, table, dialect);
    }
}

/// Attaches recovered outer syntax to the table produced from the cleaned core.
fn attach_recovered_options(
    name: Option<&SqlObjectName>,
    recovered: super::table_recovery::RecoveredTableSql,
    ctx: &mut ParseContext,
) {
    let Some(name) = name else {
        return;
    };
    let (name, schema) = object_name_parts(name);
    let key = schema_qualified_key(&name, schema.as_deref());
    let Some(table) = ctx.schema.tables.get_mut(&key) else {
        return;
    };
    let mut header = table.options.header_raw.clone();
    let mut tail = table.options.tail_raw.clone();
    header.extend(recovered.header_options);
    tail.extend(recovered.tail_options);
    table.options = crate::states::TableOptionsMeta::from_parts(header, tail);
}

fn lower_raw_segment(
    segment: &SqlSegment,
    ctx: &mut ParseContext,
    dialect: Dialect,
) -> Result<(), ParseError> {
    let Some(SqlStatementKind::Ddl(ddl)) = &segment.kind else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "schema SQL must be CREATE statements for known Gaman entity kinds",
        ));
    };
    let Some(name) = &ddl.name else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "raw fallback requires a recoverable object name",
        ));
    };
    match ddl.entity {
        EntityKind::Table => Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "CREATE TABLE must parse into a modeled table; opaque tables are not supported",
        )),
        EntityKind::Index => lower_raw_index(segment, ctx, dialect, name, ddl.owner.as_ref()),
        EntityKind::Trigger => lower_raw_trigger(segment, ctx, dialect, name, ddl.owner.as_ref()),
        EntityKind::Function => {
            let (name, schema) = object_name_parts(name);
            let key = schema_qualified_key(&name, schema.as_deref());
            let mut function = FunctionDef::from_raw(name, segment.sql.clone());
            function.schema = schema;
            ctx.schema.functions.insert(key, function);
            Ok(())
        }
        EntityKind::View => {
            let (name, schema) = object_name_parts(name);
            let key = schema_qualified_key(&name, schema.as_deref());
            let mut view = ViewDef::from_raw(name, segment.sql.clone());
            view.schema = schema;
            ctx.schema.views.insert(key, view);
            Ok(())
        }
        EntityKind::Extension => {
            let (name, schema) = object_name_parts(name);
            let key = schema_qualified_key(&name, schema.as_deref());
            let mut extension = ExtensionDef::from_raw(name, segment.sql.clone());
            extension.schema = schema;
            ctx.schema.extensions.insert(key, extension);
            Ok(())
        }
        EntityKind::Enum | EntityKind::Column | EntityKind::ForeignKey | EntityKind::Constraint => {
            Err(ParseError::unsupported(
                dialect,
                segment.sql.clone(),
                "this entity kind must be recovered from modeled SQL, not raw fallback",
            ))
        }
    }
}

fn lower_raw_index(
    segment: &SqlSegment,
    ctx: &mut ParseContext,
    dialect: Dialect,
    name: &SqlObjectName,
    owner: Option<&SqlObjectName>,
) -> Result<(), ParseError> {
    let Some(owner) = owner else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque index fallback requires a recoverable target table after ON",
        ));
    };
    let (table, schema) = object_name_parts(owner);
    let table_name = schema_qualified_key(&table, schema.as_deref());
    let (index_name, _) = object_name_parts(name);
    ctx.push_index((table_name, Index::from_raw(index_name, segment.sql.clone())));
    Ok(())
}

fn lower_raw_trigger(
    segment: &SqlSegment,
    ctx: &mut ParseContext,
    dialect: Dialect,
    name: &SqlObjectName,
    owner: Option<&SqlObjectName>,
) -> Result<(), ParseError> {
    let Some(owner) = owner else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque trigger fallback requires a recoverable target table after ON",
        ));
    };
    let (table, schema) = object_name_parts(owner);
    let table_name = schema_qualified_key(&table, schema.as_deref());
    let table =
        ctx.schema
            .tables
            .get_mut(&table_name)
            .ok_or_else(|| ParseError::UnknownTriggerTable {
                table: table_name.clone(),
            })?;
    let (trigger_name, _) = object_name_parts(name);
    table
        .triggers
        .push(TriggerDef::from_raw(trigger_name, segment.sql.clone()));
    Ok(())
}

fn object_name_parts(name: &SqlObjectName) -> (String, Option<String>) {
    match name.parts.as_slice() {
        [schema, name] if schema != "public" => (name.clone(), Some(schema.clone())),
        [_, name] => (name.clone(), None),
        [name] => (name.clone(), None),
        _ => (name.raw.clone(), None),
    }
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

#[cfg(test)]
mod recovery_tests {
    use super::*;

    /// Verifies a PostgreSQL UNLOGGED table is modeled and retains its unmanaged header.
    #[test]
    fn parse_sql_recovers_unlogged_table() {
        let schema = parse_sql(
            "CREATE UNLOGGED TABLE events (id integer NOT NULL)",
            Dialect::Postgres,
        )
        .expect("parse table");
        let table = schema.tables.get("events").expect("events table");
        assert_eq!(table.columns.len(), 1);
        assert_eq!(table.options.header_raw, ["UNLOGGED"]);
    }

    /// Verifies opaque fallback uses a quoted, schema-qualified owner from classification.
    #[test]
    fn raw_index_uses_classified_owner() {
        let schema = parse_sql(
            "CREATE TABLE app.users (email text); CREATE INDEX users_email_idx ON app.users ((lower(email)))",
            Dialect::Postgres,
        )
        .expect("parse schema");
        let table = schema.tables.get("app.users").expect("qualified table");
        assert!(
            table
                .indexes
                .iter()
                .any(|index| index.name == "users_email_idx")
        );
    }
}
