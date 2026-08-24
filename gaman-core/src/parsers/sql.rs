use sqlparser::ast::Statement;
use sqlparser::dialect::{MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;

use super::error::ParseError;
use super::segments::{SqlObjectName, SqlSegment, SqlStatementKind, segment_sql};
use super::table_recovery::recover_table_sql;
use super::tokens::{SqlToken, SqlTokenKind};
use super::{postgres, sqlite};
use crate::dialects::Dialect;
use crate::states::types::EntityKind;
use crate::states::{
    EnumDef, ExtensionDef, FunctionDef, Index, OpaqueMeta, Schema, SequenceDef, Table, TriggerDef,
    ViewDef, schema_qualified_key,
};
use std::collections::BTreeSet;

/// One classifiable opaque CREATE declaration shared by SQL and Rust ingestion.
#[derive(Debug)]
pub(crate) enum OpaqueDeclaration {
    Index { table: String, value: Index },
    Trigger { table: String, value: TriggerDef },
    Function { key: String, value: FunctionDef },
    View { key: String, value: ViewDef },
    Extension { key: String, value: ExtensionDef },
    Sequence { key: String, value: SequenceDef },
    Enum { key: String, value: EnumDef },
}

impl OpaqueDeclaration {
    /// Returns the closed entity kind proven by SQL classification.
    pub(crate) fn kind(&self) -> EntityKind {
        match self {
            Self::Index { .. } => EntityKind::Index,
            Self::Trigger { .. } => EntityKind::Trigger,
            Self::Function { .. } => EntityKind::Function,
            Self::View { .. } => EntityKind::View,
            Self::Extension { .. } => EntityKind::Extension,
            Self::Sequence { .. } => EntityKind::Sequence,
            Self::Enum { .. } => EntityKind::Enum,
        }
    }

    /// Returns the canonical identity extracted from the CREATE statement.
    pub(crate) fn identity(&self) -> String {
        match self {
            Self::Index { value, .. } => value.name.clone(),
            Self::Trigger { value, .. } => value.name.clone().unwrap_or_default(),
            Self::Function { key, .. }
            | Self::View { key, .. }
            | Self::Extension { key, .. }
            | Self::Sequence { key, .. }
            | Self::Enum { key, .. } => key.clone(),
        }
    }

    /// Returns the canonical owner for a table-owned declaration.
    pub(crate) fn owner(&self) -> Option<&str> {
        match self {
            Self::Index { table, .. } | Self::Trigger { table, .. } => Some(table),
            _ => None,
        }
    }
}

/// Classifies one raw CREATE statement through the SQL parser's opaque fallback path.
pub(crate) fn parse_opaque_create(
    source: &str,
    dialect: Dialect,
) -> Result<OpaqueDeclaration, ParseError> {
    let segments = segment_sql(source, dialect)?;
    if segments.len() != 1 {
        return Err(ParseError::unsupported(
            dialect,
            source,
            "opaque input must contain exactly one CREATE statement",
        ));
    }
    let segment = &segments[0];
    ensure_schema_segment(segment, dialect)?;
    opaque_declaration_from_segment(segment, dialect)
}

/// Validates dialect-less committed opaque metadata against every supported lexer profile.
pub(crate) fn parse_opaque_create_portable(source: &str) -> Result<OpaqueDeclaration, String> {
    let mut errors = Vec::new();
    for dialect in [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ] {
        match parse_opaque_create(source, dialect) {
            Ok(declaration) => return Ok(declaration),
            Err(error) => errors.push(opaque_parse_reason(&error)),
        }
    }
    let reason = errors
        .iter()
        .find(|error| error.contains("Gaman owns"))
        .cloned()
        .or_else(|| errors.into_iter().next())
        .unwrap_or_else(|| "opaque source could not be classified".to_string());
    Err(reason)
}

/// Returns a concise stable reason without repeating the complete source statement.
pub(crate) fn opaque_parse_reason(error: &ParseError) -> String {
    match error {
        ParseError::UnsupportedStatement { reason, .. } => reason.clone(),
        _ => error.to_string(),
    }
}

/// Extracts the authored function argument declaration used for opaque identity.
pub(crate) fn extract_function_arguments(dialect: Dialect, source: &str) -> Option<String> {
    let tokens = significant_tokens(dialect, source).ok()?;
    let function = tokens
        .iter()
        .position(|token| token.canonical_word() == Some("FUNCTION"))?;
    let open = tokens
        .iter()
        .enumerate()
        .skip(function + 1)
        .find_map(|(index, token)| {
            matches!(token.kind, SqlTokenKind::LeftParen).then_some(index)
        })?;
    let close = matching_close(&tokens, open)?;
    let start = tokens.get(open)?.span.end;
    let end = tokens.get(close)?.span.start;
    source.get(start..end).map(str::trim).map(str::to_string)
}

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

    /// Rebuilds an AST-lowered opaque index through the shared declaration boundary.
    fn preserve_opaque_index_source(
        &mut self,
        segment: &SqlSegment,
        dialect: Dialect,
    ) -> Result<(), ParseError> {
        let Some((_, index)) = self.pending_indexes.last() else {
            return Ok(());
        };
        if !index.is_opaque() {
            return Ok(());
        }
        let declaration = opaque_declaration_from_segment(segment, dialect)?;
        self.replace_opaque_index(declaration, dialect, segment)
    }

    /// Replaces the provisional AST index while preserving its classified owner identity.
    fn replace_opaque_index(
        &mut self,
        declaration: OpaqueDeclaration,
        dialect: Dialect,
        segment: &SqlSegment,
    ) -> Result<(), ParseError> {
        let Some((table, index)) = self.pending_indexes.last_mut() else {
            return Ok(());
        };
        match declaration {
            OpaqueDeclaration::Index {
                table: parsed_table,
                value,
            } if parsed_table == *table => {
                *index = value;
                Ok(())
            }
            _ => Err(ParseError::unsupported(
                dialect,
                segment.sql.clone(),
                "opaque index lowering changed its classified owner identity",
            )),
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
        let functions_before = ctx
            .schema
            .functions
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        ensure_schema_segment(&segment, dialect)?;
        if matches!(segment.kind, Some(SqlStatementKind::Ddl(ref ddl)) if ddl.entity == EntityKind::Sequence)
        {
            lower_raw_segment(&segment, &mut ctx, dialect)?;
            apply_function_annotations(&mut ctx, &segment, &functions_before, dialect)?;
            continue;
        }
        let statements = match parse_segment(&segment.semantic_sql, dialect) {
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
                if matches!(&error, ParseError::UnsupportedFunctionParameterMode { .. }) {
                    return Err(error);
                }
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
                ctx.preserve_opaque_index_source(&segment, dialect)?;
            }
        }
        apply_function_annotations(&mut ctx, &segment, &functions_before, dialect)?;
    }
    ctx.finish()
}

/// Attaches segmentation-owned annotations after the following function has been lowered.
fn apply_function_annotations(
    ctx: &mut ParseContext,
    segment: &SqlSegment,
    before: &BTreeSet<String>,
    dialect: Dialect,
) -> Result<(), ParseError> {
    if segment.annotations.is_empty() {
        return Ok(());
    }
    if !matches!(segment.kind, Some(SqlStatementKind::Ddl(ref ddl)) if ddl.entity == EntityKind::Function)
    {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "SQL annotations are supported only for CREATE FUNCTION",
        ));
    }
    let keys = ctx
        .schema
        .functions
        .keys()
        .filter(|key| !before.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    if keys.len() != 1 {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "SQL annotations require exactly one lowered CREATE FUNCTION",
        ));
    }
    let function = ctx.schema.functions.get_mut(&keys[0]).ok_or_else(|| {
        ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "annotated function disappeared during lowering",
        )
    })?;
    for annotation in &segment.annotations {
        match annotation {
            super::segments::SqlAnnotation::DependsOn { dependency, .. } => {
                function.depends_on.push(dependency.clone());
            }
        }
    }
    Ok(())
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
    let declaration = opaque_declaration_from_segment(segment, dialect)?;
    apply_opaque_declaration(ctx, declaration, dialect)
}

/// Produces the sole raw declaration shape used by every opaque ingestion frontend.
fn opaque_declaration_from_segment(
    segment: &SqlSegment,
    dialect: Dialect,
) -> Result<OpaqueDeclaration, ParseError> {
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
    validate_plain_create(segment, dialect, name)?;
    match ddl.entity {
        EntityKind::Table => Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "CREATE TABLE must parse into a modeled table; opaque tables are not supported",
        )),
        EntityKind::Index => opaque_index(segment, dialect, name, ddl.owner.as_ref()),
        EntityKind::Trigger => opaque_trigger(segment, dialect, name, ddl.owner.as_ref()),
        EntityKind::Function => {
            let (name, schema) = opaque_object_parts(segment, dialect, name)?;
            let key = schema_qualified_key(&name, schema.as_deref());
            let mut function = FunctionDef::from_raw(name, segment.semantic_sql.clone());
            function.schema = schema;
            function.arguments =
                extract_function_arguments(dialect, &segment.semantic_sql).unwrap_or_default();
            Ok(OpaqueDeclaration::Function {
                key,
                value: function,
            })
        }
        EntityKind::View => {
            let (name, schema) = opaque_object_parts(segment, dialect, name)?;
            let key = schema_qualified_key(&name, schema.as_deref());
            let mut view = ViewDef::from_raw(name, segment.sql.clone());
            view.schema = schema;
            Ok(OpaqueDeclaration::View { key, value: view })
        }
        EntityKind::Extension => {
            let (name, schema) = opaque_object_parts(segment, dialect, name)?;
            if schema.is_some() {
                return Err(ParseError::unsupported(
                    dialect,
                    segment.sql.clone(),
                    "extension identity cannot be schema-qualified",
                ));
            }
            let extension = ExtensionDef::from_raw(name, segment.sql.clone());
            let key = extension.name.clone();
            Ok(OpaqueDeclaration::Extension {
                key,
                value: extension,
            })
        }
        EntityKind::Sequence => opaque_sequence(segment, dialect, name),
        EntityKind::Enum => {
            let (name, schema) = opaque_object_parts(segment, dialect, name)?;
            let key = schema_qualified_key(&name, schema.as_deref());
            Ok(OpaqueDeclaration::Enum {
                key,
                value: EnumDef {
                    name,
                    schema,
                    values: Vec::new(),
                    opaque: OpaqueMeta::from_raw(segment.sql.clone()),
                },
            })
        }
        EntityKind::Column | EntityKind::ForeignKey | EntityKind::Constraint | EntityKind::Row => {
            Err(ParseError::unsupported(
                dialect,
                segment.sql.clone(),
                "this entity kind must be recovered from modeled SQL, not raw fallback",
            ))
        }
    }
}

fn opaque_sequence(
    segment: &SqlSegment,
    dialect: Dialect,
    object: &SqlObjectName,
) -> Result<OpaqueDeclaration, ParseError> {
    if dialect != Dialect::Postgres {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque sequences are supported only by PostgreSQL",
        ));
    }
    validate_sequence_lifecycle(segment, dialect)?;
    let (name, schema) = opaque_object_parts(segment, dialect, object)?;
    let key = schema_qualified_key(&name, schema.as_deref());
    let mut sequence = SequenceDef::from_raw(name, segment.sql.clone());
    sequence.schema = schema;
    Ok(OpaqueDeclaration::Sequence {
        key,
        value: sequence,
    })
}

fn validate_sequence_lifecycle(segment: &SqlSegment, dialect: Dialect) -> Result<(), ParseError> {
    let words = significant_tokens(dialect, &segment.sql)
        .map_err(|error| ParseError::unsupported(dialect, &segment.sql, error.to_string()))?
        .into_iter()
        .filter_map(|token| token.canonical_word().map(str::to_string))
        .collect::<Vec<_>>();
    if words
        .iter()
        .any(|word| word == "TEMP" || word == "TEMPORARY")
    {
        return Err(ParseError::unsupported(
            dialect,
            &segment.sql,
            "temporary sequences are session-owned and cannot be managed",
        ));
    }
    if words
        .windows(2)
        .any(|pair| pair[0] == "OWNED" && pair[1] == "BY")
    {
        return Err(ParseError::unsupported(
            dialect,
            &segment.sql,
            "sequence OWNED BY creates an unsupported reverse table lifecycle dependency",
        ));
    }
    Ok(())
}

fn opaque_index(
    segment: &SqlSegment,
    dialect: Dialect,
    name: &SqlObjectName,
    owner: Option<&SqlObjectName>,
) -> Result<OpaqueDeclaration, ParseError> {
    let Some(owner) = owner else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque index fallback requires a recoverable target table after ON",
        ));
    };
    let (table, schema) = opaque_object_parts(segment, dialect, owner)?;
    let table_name = schema_qualified_key(&table, schema.as_deref());
    let (index_name, index_schema) = opaque_object_parts(segment, dialect, name)?;
    if index_schema.is_some() && index_schema != schema {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque index schema must match its target table schema",
        ));
    }
    Ok(OpaqueDeclaration::Index {
        table: table_name,
        value: Index::from_raw(index_name, segment.sql.clone()),
    })
}

fn opaque_trigger(
    segment: &SqlSegment,
    dialect: Dialect,
    name: &SqlObjectName,
    owner: Option<&SqlObjectName>,
) -> Result<OpaqueDeclaration, ParseError> {
    let Some(owner) = owner else {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque trigger fallback requires a recoverable target table after ON",
        ));
    };
    let (table, schema) = opaque_object_parts(segment, dialect, owner)?;
    let table_name = schema_qualified_key(&table, schema.as_deref());
    let (trigger_name, trigger_schema) = opaque_object_parts(segment, dialect, name)?;
    if trigger_schema.is_some() {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "trigger identity must be table-scoped and unqualified",
        ));
    }
    Ok(OpaqueDeclaration::Trigger {
        table: table_name,
        value: TriggerDef::from_raw(trigger_name, segment.sql.clone()),
    })
}

/// Applies a classified raw declaration to SQL parser state without reclassification.
fn apply_opaque_declaration(
    ctx: &mut ParseContext,
    declaration: OpaqueDeclaration,
    dialect: Dialect,
) -> Result<(), ParseError> {
    match declaration {
        OpaqueDeclaration::Index { table, value } => ctx.push_index((table, value)),
        OpaqueDeclaration::Trigger { table, value } => ctx
            .schema
            .tables
            .get_mut(&table)
            .ok_or_else(|| ParseError::UnknownTriggerTable {
                table: table.clone(),
            })?
            .triggers
            .push(value),
        OpaqueDeclaration::Function { key, value } => {
            ctx.schema.functions.insert(key, value);
        }
        OpaqueDeclaration::View { key, value } => {
            ctx.schema.views.insert(key, value);
        }
        OpaqueDeclaration::Extension { key, value } => {
            ctx.schema.extensions.insert(key, value);
        }
        OpaqueDeclaration::Sequence { key, value } => {
            if ctx.schema.sequences.contains_key(&key) {
                return Err(ParseError::unsupported(
                    dialect,
                    value.raw_sql().unwrap_or_default(),
                    format!("duplicate sequence '{key}'"),
                ));
            }
            ctx.schema.sequences.insert(key, value);
        }
        OpaqueDeclaration::Enum { key, value } => {
            ctx.schema.enums.insert(key, value);
        }
    }
    Ok(())
}

fn opaque_object_parts(
    segment: &SqlSegment,
    dialect: Dialect,
    name: &SqlObjectName,
) -> Result<(String, Option<String>), ParseError> {
    let parts = match name.parts.as_slice() {
        [schema, name] if schema != "public" => Ok((name.clone(), Some(schema.clone()))),
        [_, name] => Ok((name.clone(), None)),
        [name] => Ok((name.clone(), None)),
        _ => Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque identity must be an unambiguous one- or two-part name",
        )),
    }?;
    if parts.0.contains('.')
        || parts
            .1
            .as_deref()
            .is_some_and(|schema| schema.contains('.'))
    {
        return Err(ParseError::unsupported(
            dialect,
            segment.sql.clone(),
            "opaque identity components containing dots are not supported",
        ));
    }
    Ok(parts)
}

/// Rejects caller-owned lifecycle modifiers while ignoring protected body content.
fn validate_plain_create(
    segment: &SqlSegment,
    dialect: Dialect,
    name: &SqlObjectName,
) -> Result<(), ParseError> {
    let tokens = significant_tokens(dialect, &segment.sql)
        .map_err(|error| ParseError::unsupported(dialect, &segment.sql, error.to_string()))?;
    if tokens.first().and_then(SqlToken::canonical_word) != Some("CREATE") {
        return Err(ParseError::unsupported(
            dialect,
            &segment.sql,
            "opaque definition must begin with CREATE",
        ));
    }
    let name_start = find_object_name(&tokens, &name.parts).ok_or_else(|| {
        ParseError::unsupported(
            dialect,
            &segment.sql,
            "cannot locate the opaque identity in the CREATE prefix",
        )
    })?;
    reject_lifecycle_modifiers(&tokens[..name_start])
        .map_err(|reason| ParseError::unsupported(dialect, &segment.sql, reason))
}

fn reject_lifecycle_modifiers(tokens: &[SqlToken]) -> Result<(), String> {
    if tokens.windows(2).any(|window| {
        window[0].canonical_word() == Some("OR") && window[1].canonical_word() == Some("REPLACE")
    }) {
        return Err("CREATE OR REPLACE is not accepted; Gaman owns replacement".to_string());
    }
    if tokens.windows(3).any(|window| {
        window[0].canonical_word() == Some("IF")
            && window[1].canonical_word() == Some("NOT")
            && window[2].canonical_word() == Some("EXISTS")
    }) {
        return Err(
            "CREATE IF NOT EXISTS is not accepted; Gaman owns existence handling".to_string(),
        );
    }
    Ok(())
}

fn significant_tokens(
    dialect: Dialect,
    source: &str,
) -> Result<Vec<SqlToken>, super::tokens::TokenizeError> {
    dialect.tokenizer().tokenize(source).map(|tokens| {
        tokens
            .into_iter()
            .filter(|token| !token.is_trivia())
            .collect()
    })
}

fn find_object_name(tokens: &[SqlToken], parts: &[String]) -> Option<usize> {
    (!parts.is_empty()).then_some(())?;
    (0..tokens.len()).find(|start| object_name_matches(tokens, *start, parts))
}

fn object_name_matches(tokens: &[SqlToken], start: usize, parts: &[String]) -> bool {
    let mut index = start;
    for (part_index, part) in parts.iter().enumerate() {
        if tokens.get(index).and_then(identifier) != Some(part.as_str()) {
            return false;
        }
        index += 1;
        if part_index + 1 < parts.len() {
            if !matches!(
                tokens.get(index).map(|token| &token.kind),
                Some(SqlTokenKind::Dot)
            ) {
                return false;
            }
            index += 1;
        }
    }
    true
}

fn identifier(token: &SqlToken) -> Option<&str> {
    match &token.kind {
        SqlTokenKind::Word { value, .. } | SqlTokenKind::QuotedIdentifier { value, .. } => {
            Some(value)
        }
        _ => None,
    }
}

fn matching_close(tokens: &[SqlToken], open: usize) -> Option<usize> {
    let mut depth = 0usize;
    for (index, token) in tokens.iter().enumerate().skip(open) {
        match token.kind {
            SqlTokenKind::LeftParen => depth += 1,
            SqlTokenKind::RightParen => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
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

    /// Verifies every standalone opaque entity kind uses one shared declaration shape.
    #[test]
    fn opaque_create_classifies_every_supported_kind() {
        let cases = [
            (
                "CREATE INDEX users_name_idx ON users ((lower(name)))",
                EntityKind::Index,
            ),
            (
                "CREATE TRIGGER users_touch BEFORE UPDATE ON users EXECUTE FUNCTION touch_user()",
                EntityKind::Trigger,
            ),
            (
                "CREATE FUNCTION score(value integer) RETURNS integer LANGUAGE SQL AS $$ SELECT value $$",
                EntityKind::Function,
            ),
            ("CREATE VIEW active_users AS SELECT 1", EntityKind::View),
            ("CREATE EXTENSION pg_trgm", EntityKind::Extension),
            (
                "CREATE SEQUENCE event_ids START WITH 100",
                EntityKind::Sequence,
            ),
            (
                "CREATE TYPE user_state AS ENUM ('active')",
                EntityKind::Enum,
            ),
        ];
        for (source, expected) in cases {
            let declaration =
                parse_opaque_create(source, Dialect::Postgres).expect("plain CREATE must classify");
            assert_eq!(declaration.kind(), expected);
        }
    }

    /// Verifies opaque lifecycle modifiers are rejected only in the CREATE prefix.
    #[test]
    fn opaque_create_rejects_lifecycle_modifiers() {
        for source in [
            "CREATE OR REPLACE VIEW active_users AS SELECT 1",
            "CREATE INDEX IF NOT EXISTS users_name_idx ON users ((lower(name)))",
        ] {
            let error = parse_opaque_create(source, Dialect::Postgres)
                .expect_err("caller-owned lifecycle modifier must fail");
            assert!(opaque_parse_reason(&error).contains("Gaman owns"));
        }
    }

    /// Verifies sequence lifecycle features outside root ownership fail closed.
    #[test]
    fn opaque_sequence_rejects_temporary_and_owned_by() {
        for source in [
            "CREATE TEMP SEQUENCE event_ids",
            "CREATE SEQUENCE event_ids OWNED BY events.id",
        ] {
            let error = parse_opaque_create(source, Dialect::Postgres)
                .expect_err("unsupported sequence lifecycle must fail");
            assert!(matches!(error, ParseError::UnsupportedStatement { .. }));
        }
    }

    /// Verifies sequence source is retained under its canonical root identity.
    #[test]
    fn opaque_sequence_preserves_source_and_identity() {
        let declaration = parse_opaque_create(
            "CREATE SEQUENCE audit.event_ids START WITH 100 INCREMENT BY 5",
            Dialect::Postgres,
        )
        .expect("PostgreSQL sequence should classify");
        assert_eq!(declaration.identity(), "audit.event_ids");
        let OpaqueDeclaration::Sequence { value, .. } = declaration else {
            panic!("expected sequence declaration");
        };
        assert_eq!(
            value.raw_sql(),
            Some("CREATE SEQUENCE audit.event_ids START WITH 100 INCREMENT BY 5")
        );
    }

    /// Verifies comments, terminators, and protected modifier text remain valid source content.
    #[test]
    fn opaque_create_allows_safe_source_boundaries() {
        for source in [
            "-- managed by Gaman\nCREATE VIEW active_users AS SELECT 'CREATE IF NOT EXISTS';",
            "CREATE FUNCTION message() RETURNS text LANGUAGE SQL AS $$ SELECT 'CREATE OR REPLACE' $$;",
        ] {
            parse_opaque_create(source, Dialect::Postgres)
                .expect("protected lifecycle text must not affect CREATE policy");
        }
    }

    /// Verifies opaque input cannot smuggle a second statement.
    #[test]
    fn opaque_create_rejects_multiple_statements() {
        let error = parse_opaque_create(
            "CREATE VIEW active_users AS SELECT 1; CREATE VIEW admins AS SELECT 1",
            Dialect::Postgres,
        )
        .expect_err("multiple CREATE statements must fail");
        assert!(opaque_parse_reason(&error).contains("exactly one"));
    }
}
