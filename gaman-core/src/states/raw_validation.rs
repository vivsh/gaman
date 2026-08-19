//! Fail-closed validation for authored opaque definitions and unmanaged table fragments.

use crate::dialects::Dialect;
use crate::parsers::tokens::{
    MYSQL_TOKENIZER, POSTGRES_TOKENIZER, SQLITE_TOKENIZER, SqlToken, SqlTokenKind, SqlTokenizer,
};
use crate::parsers::{extract_function_arguments, opaque_parse_reason, parse_opaque_create};

use super::{EntityKind, Schema, SchemaBuilderIssue, schema_qualified_key};

/// Parses a builder identity as an exact one- or two-part SQL object name.
pub(crate) fn parse_qualified_name(
    dialect: Dialect,
    source: &str,
) -> Result<(String, Option<String>), String> {
    let tokens = significant_tokens(dialect, source).map_err(|error| error.to_string())?;
    match tokens.as_slice() {
        [name] => {
            let name = identity_part(name)?;
            Ok((name.to_string(), None))
        }
        [schema, dot, name] if matches!(dot.kind, SqlTokenKind::Dot) => {
            let schema = identity_part(schema)?;
            let name = identity_part(name)?;
            Ok((name.to_string(), Some(schema.to_string())))
        }
        _ => Err(invalid_qualified_name()),
    }
}

fn identity_part(token: &SqlToken) -> Result<&str, String> {
    let value = identifier(token).ok_or_else(invalid_qualified_name)?;
    if value.contains('.') {
        return Err("identifier components containing dots are not supported".to_string());
    }
    Ok(value)
}

fn invalid_qualified_name() -> String {
    "expected an unambiguous one- or two-part SQL identifier".to_string()
}

/// Validates every untrusted raw definition before authored schema preparation.
pub(crate) fn validate_authored_raw(schema: &Schema, dialect: Dialect) -> Vec<SchemaBuilderIssue> {
    let mut issues = Vec::new();
    for function in schema.functions.values() {
        if validate_top_level(
            &mut issues,
            dialect,
            EntityKind::Function,
            &function.name,
            function.schema.as_deref(),
            function.opaque.raw.as_deref(),
            function.opaque.trusted,
        ) && let Err(reason) = validate_function_signature(
            dialect,
            function.opaque.raw.as_deref().unwrap_or_default(),
            &function.parameters_sql(),
        ) {
            push_opaque(
                &mut issues,
                EntityKind::Function,
                schema_qualified_key(&function.name, function.schema.as_deref()),
                reason,
            );
        }
    }
    for view in schema.views.values() {
        validate_top_level(
            &mut issues,
            dialect,
            EntityKind::View,
            &view.name,
            view.schema.as_deref(),
            view.opaque.raw.as_deref(),
            view.opaque.trusted,
        );
    }
    for extension in schema.extensions.values() {
        validate_top_level(
            &mut issues,
            dialect,
            EntityKind::Extension,
            &extension.name,
            extension.schema.as_deref(),
            extension.opaque.raw.as_deref(),
            extension.opaque.trusted,
        );
    }
    for enum_def in schema.enums.values() {
        validate_top_level(
            &mut issues,
            dialect,
            EntityKind::Enum,
            &enum_def.name,
            enum_def.schema.as_deref(),
            enum_def.opaque.raw.as_deref(),
            enum_def.opaque.trusted,
        );
    }
    for (table_key, table) in &schema.tables {
        for index in &table.indexes {
            validate_owned(
                &mut issues,
                dialect,
                EntityKind::Index,
                &index.name,
                table_key,
                index.opaque.raw.as_deref(),
                index.opaque.trusted,
            );
        }
        for trigger in &table.triggers {
            validate_owned(
                &mut issues,
                dialect,
                EntityKind::Trigger,
                trigger.name.as_deref().unwrap_or(""),
                table_key,
                trigger.opaque.raw.as_deref(),
                trigger.opaque.trusted,
            );
        }
        for constraint in &table.constraints {
            if let Some(meta) = constraint.opaque_meta()
                && !meta.trusted
                && let Some(raw) = meta.raw.as_deref()
            {
                validate_constraint(&mut issues, dialect, table_key, constraint.name(), raw);
            }
        }
        if !table.options.trusted {
            for clause in &table.options.header_raw {
                validate_clause(&mut issues, dialect, table_key, "prefix", clause);
            }
            for clause in &table.options.tail_raw {
                validate_clause(&mut issues, dialect, table_key, "suffix", clause);
            }
        }
    }
    issues
}

fn validate_top_level(
    issues: &mut Vec<SchemaBuilderIssue>,
    dialect: Dialect,
    kind: EntityKind,
    name: &str,
    schema: Option<&str>,
    raw: Option<&str>,
    trusted: bool,
) -> bool {
    if trusted || raw.is_none() {
        return false;
    }
    let identity = if kind == EntityKind::Extension {
        name.to_string()
    } else {
        schema_qualified_key(name, schema)
    };
    if let Err(reason) = validate_create(dialect, kind, &identity, None, raw.unwrap_or_default()) {
        push_opaque(issues, kind, identity, reason);
        return false;
    }
    true
}

fn validate_owned(
    issues: &mut Vec<SchemaBuilderIssue>,
    dialect: Dialect,
    kind: EntityKind,
    name: &str,
    owner: &str,
    raw: Option<&str>,
    trusted: bool,
) {
    if trusted || raw.is_none() {
        return;
    }
    if let Err(reason) = validate_create(dialect, kind, name, Some(owner), raw.unwrap_or_default())
    {
        push_opaque(issues, kind, format!("{owner}.{name}"), reason);
    }
}

fn validate_create(
    dialect: Dialect,
    kind: EntityKind,
    identity: &str,
    owner: Option<&str>,
    source: &str,
) -> Result<(), String> {
    let declaration =
        parse_opaque_create(source, dialect).map_err(|error| opaque_parse_reason(&error))?;
    if declaration.kind() != kind {
        return Err(format!(
            "statement creates {:?}, not {:?}",
            declaration.kind(),
            kind
        ));
    }
    let actual = declaration.identity();
    if actual != canonical_expected(identity) {
        return Err(format!("statement identity is '{actual}'"));
    }
    if let Some(expected_owner) = owner {
        let actual_owner = declaration
            .owner()
            .ok_or_else(|| "statement does not expose a reliable table owner".to_string())?;
        if actual_owner != canonical_expected(expected_owner) {
            return Err(format!("statement owner is '{actual_owner}'"));
        }
    }
    Ok(())
}

fn validate_function_signature(
    dialect: Dialect,
    source: &str,
    expected: &str,
) -> Result<(), String> {
    let actual = extract_function_arguments(dialect, source)
        .ok_or_else(|| "function argument list cannot be recovered safely".to_string())?;
    let expected_parts = split_arguments(dialect, expected)?;
    let actual_parts = split_arguments(dialect, &actual)?;
    if expected_parts.len() != actual_parts.len() {
        return Err(format!(
            "function has {} identity arguments, expected {}",
            actual_parts.len(),
            expected_parts.len()
        ));
    }
    for (index, (expected, actual)) in expected_parts.iter().zip(actual_parts.iter()).enumerate() {
        if !argument_ends_with(actual, expected) {
            return Err(format!(
                "function argument {} does not match type '{}'",
                index + 1,
                expected
                    .iter()
                    .map(|token| token.raw.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
        }
    }
    Ok(())
}

fn split_arguments(dialect: Dialect, source: &str) -> Result<Vec<Vec<SqlToken>>, String> {
    if source.trim().is_empty() {
        return Ok(Vec::new());
    }
    let tokens = significant_tokens(dialect, source).map_err(|error| error.to_string())?;
    let mut result = vec![Vec::new()];
    let mut depth = 0usize;
    for token in tokens {
        match token.kind {
            SqlTokenKind::LeftParen | SqlTokenKind::LeftBracket | SqlTokenKind::LeftBrace => {
                depth += 1
            }
            SqlTokenKind::RightParen | SqlTokenKind::RightBracket | SqlTokenKind::RightBrace => {
                depth = depth.saturating_sub(1)
            }
            SqlTokenKind::Comma if depth == 0 => {
                result.push(Vec::new());
                continue;
            }
            _ => {}
        }
        if let Some(argument) = result.last_mut() {
            argument.push(token);
        }
    }
    if result.iter().any(Vec::is_empty) {
        return Err("function signature contains an empty argument".to_string());
    }
    Ok(result)
}

fn argument_ends_with(actual: &[SqlToken], expected: &[SqlToken]) -> bool {
    let actual = truncate_default(actual);
    actual.len() >= expected.len()
        && actual[actual.len() - expected.len()..]
            .iter()
            .zip(expected)
            .all(|(left, right)| token_equal(left, right))
}

fn truncate_default(tokens: &[SqlToken]) -> &[SqlToken] {
    let end = tokens
        .iter()
        .position(|token| token.canonical_word() == Some("DEFAULT") || token.raw == "=")
        .unwrap_or(tokens.len());
    &tokens[..end]
}

fn validate_constraint(
    issues: &mut Vec<SchemaBuilderIssue>,
    dialect: Dialect,
    table: &str,
    name: &str,
    clause: &str,
) {
    match significant_tokens(dialect, clause) {
        Ok(tokens)
            if tokens.first().and_then(|token| token.canonical_word()) == Some("CONSTRAINT")
                && find_object_name(&tokens, &[name.to_string()]) == Some(1)
                && balanced(&tokens)
                && !tokens
                    .iter()
                    .any(|token| matches!(token.kind, SqlTokenKind::Semicolon)) => {}
        _ => push_opaque(
            issues,
            EntityKind::Constraint,
            format!("{table}.{name}"),
            "constraint must be one balanced 'CONSTRAINT <name> ...' clause".to_string(),
        ),
    }
}

fn validate_clause(
    issues: &mut Vec<SchemaBuilderIssue>,
    dialect: Dialect,
    table: &str,
    placement: &str,
    clause: &str,
) {
    let reason = match significant_tokens(dialect, clause) {
        Err(error) => Some(error.to_string()),
        Ok(tokens) if tokens.is_empty() => Some("clause is empty".to_string()),
        Ok(tokens) if !balanced(&tokens) => Some("clause has unbalanced delimiters".to_string()),
        Ok(tokens)
            if tokens
                .iter()
                .any(|token| matches!(token.kind, SqlTokenKind::Semicolon)) =>
        {
            Some("clause contains a statement terminator".to_string())
        }
        Ok(tokens)
            if matches!(
                tokens.first().and_then(|token| token.canonical_word()),
                Some("CREATE" | "ALTER" | "DROP")
            ) =>
        {
            Some("clause must not contain a complete DDL statement".to_string())
        }
        Ok(_) => None,
    };
    if let Some(reason) = reason {
        issues.push(SchemaBuilderIssue::InvalidUnmanagedClause {
            table: table.to_string(),
            placement: placement.to_string(),
            reason,
        });
    }
}

fn significant_tokens(
    dialect: Dialect,
    source: &str,
) -> Result<Vec<SqlToken>, crate::parsers::tokens::TokenizeError> {
    tokenizer(dialect).tokenize(source).map(|tokens| {
        tokens
            .into_iter()
            .filter(|token| !token.is_trivia())
            .collect()
    })
}

fn tokenizer(dialect: Dialect) -> &'static dyn SqlTokenizer {
    match dialect {
        Dialect::Postgres => &POSTGRES_TOKENIZER,
        Dialect::Sqlite => &SQLITE_TOKENIZER,
        Dialect::Mysql | Dialect::Mariadb => &MYSQL_TOKENIZER,
    }
}

fn canonical_expected(identity: &str) -> String {
    identity
        .strip_prefix("public.")
        .unwrap_or(identity)
        .to_string()
}

fn find_object_name(tokens: &[SqlToken], parts: &[String]) -> Option<usize> {
    if parts.is_empty() {
        return None;
    }
    (0..tokens.len()).find(|start| object_name_matches(tokens, *start, parts))
}

fn object_name_matches(tokens: &[SqlToken], start: usize, parts: &[String]) -> bool {
    let mut index = start;
    for (part_index, part) in parts.iter().enumerate() {
        let Some(token) = tokens.get(index) else {
            return false;
        };
        if identifier(token) != Some(part.as_str()) {
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

fn balanced(tokens: &[SqlToken]) -> bool {
    let mut stack = Vec::new();
    for token in tokens {
        let value = match token.kind {
            SqlTokenKind::LeftParen => Some('('),
            SqlTokenKind::LeftBracket => Some('['),
            SqlTokenKind::LeftBrace => Some('{'),
            SqlTokenKind::RightParen => {
                if stack.pop() != Some('(') {
                    return false;
                }
                None
            }
            SqlTokenKind::RightBracket => {
                if stack.pop() != Some('[') {
                    return false;
                }
                None
            }
            SqlTokenKind::RightBrace => {
                if stack.pop() != Some('{') {
                    return false;
                }
                None
            }
            _ => None,
        };
        if let Some(value) = value {
            stack.push(value);
        }
    }
    stack.is_empty()
}

fn token_equal(left: &SqlToken, right: &SqlToken) -> bool {
    match (left.canonical_word(), right.canonical_word()) {
        (Some(left), Some(right)) => left == right,
        _ => left.raw == right.raw,
    }
}

fn push_opaque(
    issues: &mut Vec<SchemaBuilderIssue>,
    kind: EntityKind,
    entity: String,
    reason: String,
) {
    issues.push(SchemaBuilderIssue::InvalidOpaqueDefinition {
        kind: format!("{kind:?}").to_ascii_lowercase(),
        entity,
        reason,
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{SchemaBuilder, TableBuilder};

    /// Verifies malformed opaque source and unmanaged clauses fail together at build.
    #[test]
    fn builder_accumulates_raw_validation_failures() {
        let error = SchemaBuilder::new(Dialect::Postgres)
            .table_def(
                TableBuilder::new("documents")
                    .unmanaged_suffix("; DROP TABLE users")
                    .build(),
            )
            .opaque("CREATE OR REPLACE VIEW recent_documents AS SELECT 1")
            .opaque("CREATE INDEX IF NOT EXISTS documents_idx ON documents ((lower(id)))")
            .build()
            .expect_err("raw declarations must fail");
        let message = error.to_string();
        assert!(message.contains("CREATE OR REPLACE"));
        assert!(message.contains("CREATE IF NOT EXISTS"));
        assert!(message.contains("statement terminator"));
    }

    /// Verifies qualified opaque identities and owned table objects lower through existing structs.
    #[test]
    fn builder_lowers_verified_opaque_entities() {
        let schema = SchemaBuilder::new(Dialect::Postgres)
            .table_def(
                TableBuilder::new("documents")
                    .schema("search")
                    .column("body", "text", |column| column)
                    .build(),
            )
            .opaque("CREATE INDEX documents_body_idx ON search.documents USING gin (body)")
            .opaque("CREATE VIEW search.recent_documents AS SELECT body FROM search.documents")
            .build()
            .expect("verified source must build");
        assert!(schema.tables["search.documents"].indexes[0].is_opaque());
        assert!(schema.views["search.recent_documents"].is_opaque());
    }

    /// Verifies the base builder APIs parse quoted qualified names without `_in` overloads.
    #[test]
    fn builder_parses_qualified_names_from_identity() {
        let schema = SchemaBuilder::new(Dialect::Postgres)
            .table_def(
                TableBuilder::new("Documents")
                    .schema("Search")
                    .column("body", "text", |column| column)
                    .build(),
            )
            .opaque(
                "CREATE VIEW \"Search\".\"Recent Documents\" AS SELECT body FROM \"Search\".\"Documents\"",
            )
            .extend_table("\"Search\".\"Documents\"", |table| {
                table.unmanaged_prefix("UNLOGGED")
            })
            .build()
            .expect("quoted qualified identities must build");
        assert!(schema.views["Search.Recent Documents"].is_opaque());
        assert!(schema.tables["Search.Documents"].has_unmanaged_options());
    }

    /// Verifies ambiguous and over-qualified identities fail rather than being guessed.
    #[test]
    fn builder_rejects_ambiguous_qualified_names() {
        let error = SchemaBuilder::new(Dialect::Postgres)
            .opaque("CREATE VIEW \"audit.events\" AS SELECT 1")
            .opaque("CREATE TYPE catalog.audit.state AS ENUM ('ready')")
            .opaque("CREATE EXTENSION public.pg_trgm")
            .build()
            .expect_err("ambiguous identities must fail");
        let message = error.to_string();
        assert!(message.contains("components containing dots"));
        assert!(message.contains("one- or two-part"));
        assert!(message.contains("invalid opaque entity"));
    }

    /// Verifies every top-level and table-owned opaque builder lowers through existing models.
    #[test]
    fn builder_lowers_all_opaque_kinds() {
        let table = TableBuilder::new("documents")
            .column("period", "tstzrange", |column| column)
            .opaque_constraint(
                "documents_period_excl",
                "CONSTRAINT documents_period_excl EXCLUDE USING gist (period WITH &&)",
            )
            .build();
        let schema = SchemaBuilder::new(Dialect::Postgres)
            .table_def(table)
            .opaque(
                "CREATE TRIGGER documents_touch BEFORE UPDATE ON documents FOR EACH ROW EXECUTE FUNCTION touch_document()",
            )
            .opaque(
                "CREATE FUNCTION score_document(value integer) RETURNS integer LANGUAGE SQL AS $$ SELECT value $$",
            )
            .opaque("CREATE EXTENSION pg_trgm")
            .opaque("CREATE TYPE document_state AS ENUM ('ready')")
            .build()
            .expect("all verified opaque kinds must build");
        assert!(schema.functions["score_document"].is_opaque());
        assert!(schema.extensions["pg_trgm"].is_opaque());
        assert!(schema.enums["document_state"].is_opaque());
        assert!(schema.tables["documents"].constraints[0].is_opaque());
        assert!(schema.tables["documents"].triggers[0].is_opaque());
    }

    /// Verifies malformed source, table identity changes, and duplicate roots fail closed.
    #[test]
    fn builder_rejects_ambiguous_identity_changes() {
        let base = SchemaBuilder::new(Dialect::Postgres)
            .table_def(TableBuilder::new("documents").build())
            .view("recent_documents", "SELECT 1")
            .build()
            .expect("base schema");
        let error = SchemaBuilder::from_schema(Dialect::Postgres, base)
            .extend_table("documents", |table| table.schema("other"))
            .opaque("CREATE VIEW recent_documents AS SELECT 1")
            .opaque("CREATE OR REPLACE FUNCTION score_document(value integer) RETURNS integer LANGUAGE SQL AS $$ SELECT value $$")
            .build()
            .expect_err("ambiguous identities must fail");
        let message = error.to_string();
        assert!(message.contains("changed identity"));
        assert!(message.contains("duplicate view"));
        assert!(message.contains("CREATE OR REPLACE"));
    }

    /// Verifies an existing model schema can be extended without exposing lifecycle metadata.
    #[test]
    fn builder_extends_existing_schema() {
        let base = SchemaBuilder::new(Dialect::Postgres)
            .table_def(
                TableBuilder::new("documents")
                    .column("body", "text", |column| column)
                    .build(),
            )
            .build()
            .expect("base schema");
        let schema = SchemaBuilder::from_schema(Dialect::Postgres, base)
            .extend_table("documents", |table| table.unmanaged_prefix("UNLOGGED"))
            .build()
            .expect("extended schema");
        assert!(schema.tables["documents"].has_unmanaged_options());
    }

    /// Verifies Rust opaque declarations and SQL fallback produce identical prepared state.
    #[test]
    fn builder_opaque_uses_sql_fallback_lowering() {
        let index_sql = "CREATE INDEX documents_body_idx ON search.documents ((lower(body)))";
        let parsed = crate::parsers::parse_sql(
            &format!("CREATE TABLE search.documents (body text);{index_sql};"),
            Dialect::Postgres,
        )
        .expect("SQL schema must parse");
        let built = SchemaBuilder::new(Dialect::Postgres)
            .opaque(index_sql)
            .table_def(
                TableBuilder::new("documents")
                    .schema("search")
                    .column("body", "text", |column| column.nullable())
                    .build(),
            )
            .build()
            .expect("builder schema must compile");
        assert_eq!(built, parsed);
    }
}
