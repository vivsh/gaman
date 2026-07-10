use sqlx::{Row, SqliteConnection};

use super::{BoxFuture, Executor, ExecutorError, Introspectable};
use crate::tracking::TRACKING_TABLE;
use gaman_core::states::{
    Column, Constraint, ForeignKey, Index, OpaqueMeta, PrimaryKey, Schema, Table, TableOptionsMeta,
    TriggerDef, TriggerEvent, TriggerScope, TriggerTiming, ViewDef,
};

/// Wraps a live SQLite connection and manages transaction boundaries explicitly.
pub struct SqliteExecutor {
    conn: SqliteConnection,
}

impl SqliteExecutor {
    pub fn new(conn: SqliteConnection) -> Self {
        Self { conn }
    }
}

impl Executor for SqliteExecutor {
    fn prepare<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::Executor::prepare(&mut self.conn, sql)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Prepare(format!("{e}\n  SQL: {sql}")))
        })
    }

    fn execute<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query(sql)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Execute(format!("{e}\n  SQL: {sql}")))
        })
    }

    fn fetch_strings<'a>(
        &'a mut self,
        sql: &'a str,
    ) -> BoxFuture<'a, Result<Vec<String>, ExecutorError>> {
        Box::pin(async move {
            let rows = sqlx::query(sql)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
            rows.into_iter()
                .map(|r| {
                    r.try_get::<String, _>(0)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))
                })
                .collect()
        })
    }

    fn begin<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("BEGIN")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn commit<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("COMMIT")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }

    fn rollback<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("ROLLBACK")
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| ExecutorError::Transaction(e.to_string()))
        })
    }
}

fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

fn synth_fk_name(table: &str, from_column: &str) -> String {
    format!("{table}_{from_column}_fkey")
}

type SqliteFkColumns = Vec<(i64, String, String)>;
type SqliteFkGroups =
    std::collections::BTreeMap<i64, (String, SqliteFkColumns, Option<String>, Option<String>)>;

struct MasterRow {
    kind: String,
    name: String,
    table_name: String,
    sql: Option<String>,
}

fn sqlite_fk_action(action: &str) -> Option<String> {
    match action.trim().to_ascii_uppercase().as_str() {
        "CASCADE" => Some("cascade".to_string()),
        "RESTRICT" => Some("restrict".to_string()),
        "SET NULL" => Some("set_null".to_string()),
        "SET DEFAULT" => Some("set_default".to_string()),
        "NO ACTION" | "" => None,
        _ => None,
    }
}

#[derive(Default)]
struct ParsedTableSql {
    generated_columns: std::collections::BTreeMap<String, String>,
    constraints: Vec<Constraint>,
}

#[derive(Default)]
struct ParsedIndexSql {
    predicate: Option<String>,
}

async fn fetch_master_rows(conn: &mut SqliteConnection) -> Result<Vec<MasterRow>, ExecutorError> {
    let query = format!(
        "SELECT type, name, tbl_name, sql FROM sqlite_master \
         WHERE type IN ('table', 'index', 'view', 'trigger') \
         AND name NOT LIKE 'sqlite_%' \
         AND name != '{TRACKING_TABLE}' \
         ORDER BY type, name",
    );
    let rows = sqlx::query(&query)
        .fetch_all(conn)
        .await
        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    rows.into_iter()
        .map(|row| {
            Ok(MasterRow {
                kind: row
                    .try_get("type")
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?,
                name: row
                    .try_get("name")
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?,
                table_name: row
                    .try_get("tbl_name")
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?,
                sql: row
                    .try_get("sql")
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?,
            })
        })
        .collect()
}

impl Introspectable for SqliteExecutor {
    fn inspect_db<'a>(
        &'a mut self,
        _schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, ExecutorError>> {
        Box::pin(async move {
            let mut state = Schema::default();

            let master_rows = fetch_master_rows(&mut self.conn).await?;

            for row in master_rows.iter().filter(|row| row.kind == "table") {
                let table_name = row.name.clone();
                let parsed_table = parse_create_table_sql(row.sql.as_deref().unwrap_or_default());
                let quoted_table = quote_ident(&table_name);
                let mut table = Table {
                    name: table_name.clone(),
                    schema: None,
                    primary_key: None,
                    columns: vec![],
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                    options: TableOptionsMeta::default(),
                };

                let col_rows = sqlx::query(&format!("PRAGMA table_xinfo({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                let mut pk_columns: Vec<(i64, String)> = Vec::new();
                for cr in col_rows {
                    let hidden: i64 = cr
                        .try_get("hidden")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    if hidden == 1 {
                        continue;
                    }
                    let name: String = cr
                        .try_get("name")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let col_type: String = cr
                        .try_get("type")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let notnull: i64 = cr
                        .try_get("notnull")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let default: Option<String> = cr
                        .try_get("dflt_value")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let pk: i64 = cr
                        .try_get("pk")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    if pk > 0 {
                        pk_columns.push((pk, name.clone()));
                    }
                    table.columns.push(Column {
                        generated: parsed_table.generated_columns.get(&name).cloned(),
                        name,
                        col_type,
                        nullable: notnull == 0 && pk == 0,
                        default,
                        primary_key: pk > 0,
                        references: None,
                        check: None,
                    });
                }
                table.constraints.extend(parsed_table.constraints);
                if !pk_columns.is_empty() {
                    pk_columns.sort_by_key(|(position, _)| *position);
                    table.primary_key = Some(PrimaryKey {
                        name: table.pk_constraint_name(),
                        columns: pk_columns.into_iter().map(|(_, column)| column).collect(),
                    });
                }

                let fk_rows = sqlx::query(&format!("PRAGMA foreign_key_list({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let mut fk_map: SqliteFkGroups = std::collections::BTreeMap::new();
                for fkr in fk_rows {
                    let id: i64 = fkr
                        .try_get("id")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let seq: i64 = fkr
                        .try_get("seq")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let from_column: String = fkr
                        .try_get("from")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let to_table: String = fkr
                        .try_get("table")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let to_column: String = fkr
                        .try_get("to")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let on_delete: String = fkr
                        .try_get("on_delete")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let on_update: String = fkr
                        .try_get("on_update")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let entry = fk_map.entry(id).or_insert_with(|| {
                        (
                            to_table,
                            Vec::new(),
                            sqlite_fk_action(&on_delete),
                            sqlite_fk_action(&on_update),
                        )
                    });
                    entry.1.push((seq, from_column, to_column));
                }
                for (_, (to_table, mut columns, on_delete, on_update)) in fk_map {
                    columns.sort_by_key(|(seq, _, _)| *seq);
                    let from_columns: Vec<String> = columns
                        .iter()
                        .map(|(_, from_column, _)| from_column.clone())
                        .collect();
                    let to_columns: Vec<String> = columns
                        .into_iter()
                        .map(|(_, _, to_column)| to_column)
                        .collect();
                    let mut foreign_key = ForeignKey::new(
                        synth_fk_name(&table_name, &from_columns.join("_")),
                        from_columns,
                        to_table,
                        to_columns,
                    );
                    foreign_key.on_delete = on_delete;
                    foreign_key.on_update = on_update;
                    table.foreign_keys.push(foreign_key);
                }

                let idx_rows = sqlx::query(&format!("PRAGMA index_list({quoted_table})"))
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                for idxr in idx_rows {
                    let idx_name: String = idxr
                        .try_get("name")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let origin: String = idxr
                        .try_get("origin")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    if origin != "c" {
                        continue;
                    }
                    let unique: i64 = idxr
                        .try_get("unique")
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let quoted_idx = quote_ident(&idx_name);
                    let index_sql = master_rows
                        .iter()
                        .find(|row| row.kind == "index" && row.name == idx_name)
                        .and_then(|row| row.sql.as_deref());
                    let parsed_index = index_sql.map(parse_create_index_sql).unwrap_or_default();
                    let info_rows = sqlx::query(&format!("PRAGMA index_xinfo({quoted_idx})"))
                        .fetch_all(&mut self.conn)
                        .await
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let mut keyed_columns = Vec::new();
                    for ir in info_rows {
                        let key: i64 = ir
                            .try_get("key")
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        if key == 0 {
                            continue;
                        }
                        let seqno: i64 = ir
                            .try_get("seqno")
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let name: Option<String> = ir
                            .try_get("name")
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        keyed_columns.push((seqno, name));
                    }
                    keyed_columns.sort_by_key(|(seqno, _)| *seqno);
                    let mut columns = Vec::new();
                    let mut opaque_index = None;
                    for (_, name) in keyed_columns {
                        match name {
                            Some(name) => columns.push(name),
                            None => {
                                let raw = index_sql.unwrap_or_default();
                                let mut index = Index::from_raw(idx_name.clone(), raw.to_string());
                                index.unique = unique != 0;
                                index.predicate = parsed_index.predicate.clone();
                                index.mark_trusted();
                                opaque_index = Some(index);
                                break;
                            }
                        }
                    }
                    if let Some(index) = opaque_index {
                        table.indexes.push(index);
                        continue;
                    }
                    table.indexes.push(Index {
                        name: idx_name,
                        columns,
                        unique: unique != 0,
                        predicate: parsed_index.predicate,
                        opaque: OpaqueMeta::default(),
                    });
                }

                state.tables.insert(table_name, table);
            }

            for row in master_rows.iter().filter(|row| row.kind == "view") {
                let name = row.name.clone();
                state.views.insert(
                    name.clone(),
                    ViewDef {
                        name,
                        schema: None,
                        definition: parse_view_definition(row.sql.as_deref().unwrap_or_default()),
                        opaque: OpaqueMeta::default(),
                    },
                );
            }

            for row in master_rows.iter().filter(|row| row.kind == "trigger") {
                if let Some(table) = state.tables.get_mut(&row.table_name)
                    && let Some(trigger) =
                        parse_trigger_sql(&row.name, row.sql.as_deref().unwrap_or_default())
                {
                    table.triggers.push(trigger);
                }
            }

            Ok(state)
        })
    }
}

fn parse_create_table_sql(sql: &str) -> ParsedTableSql {
    let mut parsed = ParsedTableSql::default();
    let Some(body) = parenthesized_body(sql) else {
        return parsed;
    };
    for part in split_top_level(body, ',') {
        let trimmed = part.trim();
        if trimmed.is_empty() {
            continue;
        }
        let upper = trimmed.to_ascii_uppercase();
        if upper.starts_with("CONSTRAINT ") {
            parse_named_constraint(trimmed, &mut parsed);
        } else if let Some((name, expr)) = parse_generated_column(trimmed) {
            parsed.generated_columns.insert(name, expr);
        }
    }
    parsed
}

fn parse_create_index_sql(sql: &str) -> ParsedIndexSql {
    let Some(on_pos) = find_top_level_keyword(sql, "ON") else {
        return ParsedIndexSql::default();
    };
    let after_on = &sql[on_pos + "ON".len()..];
    let Some(_body) = parenthesized_body(after_on) else {
        return ParsedIndexSql::default();
    };
    ParsedIndexSql {
        predicate: find_top_level_keyword(sql, "WHERE")
            .map(|pos| {
                sql[pos + "WHERE".len()..]
                    .trim()
                    .trim_end_matches(';')
                    .to_string()
            })
            .filter(|predicate| !predicate.is_empty()),
    }
}

fn parse_named_constraint(input: &str, parsed: &mut ParsedTableSql) {
    let Some((_, rest)) = strip_keyword(input, "CONSTRAINT") else {
        return;
    };
    let Some((name, rest)) = take_ident(rest.trim_start()) else {
        return;
    };
    let rest = rest.trim_start();
    if let Some(inner) = keyword_paren_body(rest, "UNIQUE") {
        parsed.constraints.push(Constraint::Unique {
            name,
            columns: split_ident_list(inner),
        });
    } else if let Some(inner) = keyword_paren_body(rest, "CHECK") {
        parsed.constraints.push(Constraint::Check {
            name,
            expression: inner.trim().to_string(),
        });
    } else if strip_keyword(rest, "PRIMARY").is_some() || strip_keyword(rest, "FOREIGN").is_some() {
    } else {
        parsed.constraints.push(Constraint::from_trusted_raw(
            name.clone(),
            format!("CONSTRAINT {name} {rest}"),
        ));
    }
}

fn parse_generated_column(input: &str) -> Option<(String, String)> {
    let (name, rest) = take_ident(input.trim_start())?;
    let upper = rest.to_ascii_uppercase();
    let pos = upper.find("GENERATED ALWAYS AS")?;
    let after = &rest[pos + "GENERATED ALWAYS AS".len()..];
    let expr = parenthesized_body(after.trim_start())?;
    Some((name, expr.trim().to_string()))
}

fn parenthesized_body(input: &str) -> Option<&str> {
    let start = input.find('(')?;
    let end = matching_paren(input, start)?;
    Some(&input[start + 1..end])
}

fn keyword_paren_body<'a>(input: &'a str, keyword: &str) -> Option<&'a str> {
    let (_, rest) = strip_keyword(input.trim_start(), keyword)?;
    parenthesized_body(rest.trim_start())
}

fn strip_keyword<'a>(input: &'a str, keyword: &str) -> Option<(&'a str, &'a str)> {
    let trimmed = input.trim_start();
    if trimmed.len() < keyword.len() || !trimmed[..keyword.len()].eq_ignore_ascii_case(keyword) {
        return None;
    }
    let rest = &trimmed[keyword.len()..];
    if rest
        .chars()
        .next()
        .is_some_and(|ch| ch == '_' || ch.is_ascii_alphanumeric())
    {
        return None;
    }
    Some((&trimmed[..keyword.len()], rest))
}

fn matching_paren(input: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut chars = input.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if idx < open {
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                if chars.peek().is_some_and(|(_, next)| *next == q) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '[' => quote = Some(']'),
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(idx);
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top_level(input: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0usize;
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut chars = input.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if let Some(q) = quote {
            if ch == q {
                if chars.peek().is_some_and(|(_, next)| *next == q) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '[' => quote = Some(']'),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            c if c == delimiter && depth == 0 => {
                parts.push(&input[start..idx]);
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&input[start..]);
    parts
}

fn split_ident_list(input: &str) -> Vec<String> {
    split_top_level(input, ',')
        .into_iter()
        .filter_map(|part| take_ident(part.trim()).map(|(ident, _)| ident))
        .collect()
}

fn find_top_level_keyword(input: &str, keyword: &str) -> Option<usize> {
    let mut depth = 0usize;
    let mut quote: Option<char> = None;
    let mut chars = input.char_indices().peekable();
    while let Some((idx, ch)) = chars.next() {
        if let Some(q) = quote {
            if ch == q {
                if chars.peek().is_some_and(|(_, next)| *next == q) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        match ch {
            '\'' | '"' | '`' => quote = Some(ch),
            '[' => quote = Some(']'),
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 && starts_keyword_at(input, idx, keyword) => return Some(idx),
            _ => {}
        }
    }
    None
}

fn starts_keyword_at(input: &str, idx: usize, keyword: &str) -> bool {
    let Some(rest) = input.get(idx..) else {
        return false;
    };
    if rest.len() < keyword.len() || !rest[..keyword.len()].eq_ignore_ascii_case(keyword) {
        return false;
    }
    let before_ok = input[..idx]
        .chars()
        .next_back()
        .is_none_or(|ch| !(ch == '_' || ch.is_ascii_alphanumeric()));
    let after_ok = rest[keyword.len()..]
        .chars()
        .next()
        .is_none_or(|ch| !(ch == '_' || ch.is_ascii_alphanumeric()));
    before_ok && after_ok
}

fn take_ident(input: &str) -> Option<(String, &str)> {
    let input = input.trim_start();
    let mut chars = input.char_indices();
    let (_, first) = chars.next()?;
    if matches!(first, '"' | '`' | '[') {
        let end_quote = if first == '[' { ']' } else { first };
        let mut value = String::new();
        let mut iter = input[first.len_utf8()..].char_indices().peekable();
        while let Some((offset, ch)) = iter.next() {
            if ch == end_quote {
                if iter.peek().is_some_and(|(_, next)| *next == end_quote) {
                    value.push(ch);
                    iter.next();
                    continue;
                }
                let rest_start = first.len_utf8() + offset + ch.len_utf8();
                return Some((value, &input[rest_start..]));
            }
            value.push(ch);
        }
        return None;
    }
    let mut end = first.len_utf8();
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return None;
    }
    for (idx, ch) in chars {
        if ch == '_' || ch.is_ascii_alphanumeric() {
            end = idx + ch.len_utf8();
        } else {
            break;
        }
    }
    Some((input[..end].to_string(), &input[end..]))
}

fn parse_view_definition(sql: &str) -> String {
    let upper = sql.to_ascii_uppercase();
    match upper.find(" AS ") {
        Some(pos) => sql[pos + 4..].trim().to_string(),
        None => sql.to_string(),
    }
}

fn parse_trigger_sql(name: &str, sql: &str) -> Option<TriggerDef> {
    let normalized = sql.split_whitespace().collect::<Vec<_>>().join(" ");
    let upper = normalized.to_ascii_uppercase();
    let timing = if upper.contains(" BEFORE ") {
        TriggerTiming::Before
    } else if upper.contains(" INSTEAD OF ") {
        TriggerTiming::InsteadOf
    } else {
        TriggerTiming::After
    };
    let event = if upper.contains(" INSERT ON ") {
        TriggerEvent::Insert
    } else if upper.contains(" UPDATE ON ") {
        TriggerEvent::Update
    } else if upper.contains(" DELETE ON ") {
        TriggerEvent::Delete
    } else {
        return None;
    };
    let query = extract_trigger_body(sql);
    Some(TriggerDef {
        name: Some(name.to_string()),
        timing,
        events: vec![event],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query,
        language: None,
        opaque: OpaqueMeta::default(),
    })
}

fn extract_trigger_body(sql: &str) -> Option<String> {
    let upper = sql.to_ascii_uppercase();
    let begin = upper.find("BEGIN")?;
    let end = upper.rfind("END")?;
    (end > begin).then(|| {
        sql[begin + "BEGIN".len()..end]
            .trim()
            .trim_end_matches(';')
            .trim()
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema_check::{SqlSchemaInput, check_sql_schema_with_executor};
    use gaman_core::dialects::Dialect;
    use sqlx::ConnectOptions;

    /// Verifies SQLite CREATE TABLE parsing recovers modeled generated columns and constraints.
    #[test]
    fn parse_create_table_sql_recovers_generated_columns_and_constraints() {
        let parsed = parse_create_table_sql(
            r#"CREATE TABLE "items" ("id" integer, "parent_id" integer, "total" integer GENERATED ALWAYS AS (price * qty) STORED, CONSTRAINT "items_pkey" PRIMARY KEY ("id"), CONSTRAINT "items_parent_fkey" FOREIGN KEY ("parent_id") REFERENCES "parents" ("id"), CONSTRAINT "items_sku_key" UNIQUE ("sku"), CONSTRAINT "items_price_check" CHECK (price >= 0))"#,
        );

        assert_eq!(
            parsed.generated_columns.get("total").map(String::as_str),
            Some("price * qty")
        );
        assert_eq!(
            parsed.constraints,
            vec![
                Constraint::Unique {
                    name: "items_sku_key".to_string(),
                    columns: vec!["sku".to_string()],
                },
                Constraint::Check {
                    name: "items_price_check".to_string(),
                    expression: "price >= 0".to_string(),
                }
            ]
        );
    }

    /// Verifies SQLite trigger parsing recovers verify-grade trigger metadata.
    #[test]
    fn parse_trigger_sql_recovers_trigger_metadata() {
        let trigger = parse_trigger_sql(
            "users_insert_after_trg",
            "CREATE TRIGGER \"users_insert_after_trg\"\nAFTER INSERT ON \"users\"\nFOR EACH ROW\nBEGIN\nINSERT INTO audit_log(user_id) VALUES (NEW.id);\nEND",
        )
        .expect("trigger should parse");

        assert_eq!(trigger.name.as_deref(), Some("users_insert_after_trg"));
        assert_eq!(trigger.timing, TriggerTiming::After);
        assert_eq!(trigger.events, vec![TriggerEvent::Insert]);
        assert_eq!(trigger.scope, TriggerScope::Row);
    }

    /// Verifies SQLite driver preparation validates source without creating schema objects.
    #[tokio::test]
    async fn prepare_validates_sql_without_executing_it() {
        let options = "sqlite::memory:"
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .expect("SQLite options");
        let mut executor = SqliteExecutor::new(options.connect().await.expect("SQLite connection"));
        let report = check_sql_schema_with_executor(
            &mut executor,
            Dialect::Sqlite,
            [SqlSchemaInput::new(
                "schema.sql",
                "CREATE TABLE users (id integer);",
            )],
        )
        .await;

        assert!(!report.has_failures());
        let tables = sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'users'",
        )
        .fetch_one(&mut executor.conn)
        .await
        .expect("table count");
        assert_eq!(tables, 0);
    }

    /// Verifies SQLite driver preparation reports malformed statements without execution.
    #[tokio::test]
    async fn prepare_reports_malformed_sql() {
        let options = "sqlite::memory:"
            .parse::<sqlx::sqlite::SqliteConnectOptions>()
            .expect("SQLite options");
        let mut executor = SqliteExecutor::new(options.connect().await.expect("SQLite connection"));
        let report = check_sql_schema_with_executor(
            &mut executor,
            Dialect::Sqlite,
            [SqlSchemaInput::new("schema.sql", "SELECT FROM;")],
        )
        .await;

        assert!(report.has_failures());
    }
}
