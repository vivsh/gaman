use sqlx::PgConnection;
use sqlx::Row;

use super::{BoxFuture, Executor, ExecutorError, Introspectable};
use gaman_core::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, PrimaryKey, Schema,
    Table, TriggerDef, TriggerEvent, TriggerScope, TriggerTiming, ViewDef, Volatility,
    schema_qualified_key,
};

const GAMAN_LOCK_KEY: i64 = 7242068691819328000;

/// Wraps a live Postgres connection and manages transaction boundaries explicitly.
/// Call `begin()` before a migration and `commit()` or `rollback()` after.
pub struct PostgresExecutor {
    conn: PgConnection,
}

impl PostgresExecutor {
    pub fn new(conn: PgConnection) -> Self {
        Self { conn }
    }
}

impl Executor for PostgresExecutor {
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

    fn acquire_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("SET lock_timeout = '30s'")
                .execute(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Execute(e.to_string()))?;
            sqlx::query("SELECT pg_advisory_lock($1)")
                .bind(GAMAN_LOCK_KEY)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| {
                    ExecutorError::Execute(format!("could not acquire migration lock: {e}"))
                })
        })
    }

    fn release_lock<'a>(&'a mut self) -> BoxFuture<'a, Result<(), ExecutorError>> {
        Box::pin(async move {
            sqlx::query("SELECT pg_advisory_unlock($1)")
                .bind(GAMAN_LOCK_KEY)
                .execute(&mut self.conn)
                .await
                .map(|_| ())
                .map_err(|e| {
                    ExecutorError::Execute(format!("could not release migration lock: {e}"))
                })
        })
    }
}

// Extracts column list, UNIQUE flag, and optional partial-index predicate from a
// pg_indexes.indexdef string.
// e.g. "CREATE UNIQUE INDEX idx ON t USING btree (a, b DESC) WHERE (deleted IS NULL)"
fn parse_index_def(def: &str) -> Result<(Vec<String>, bool, Option<String>), ExecutorError> {
    let unique = def.contains("CREATE UNIQUE INDEX");

    let (col_part, predicate) = if let Some(where_pos) = def.find(" WHERE ") {
        let pred = normalize_index_predicate(&def[where_pos + " WHERE ".len()..]);
        (&def[..where_pos], Some(pred))
    } else {
        (def, None)
    };

    let cols: Vec<String> = if let Some(start) = col_part.find('(') {
        let inner = &col_part[start + 1..];
        let end = inner.rfind(')').unwrap_or(inner.len());
        let inner = inner[..end].trim();
        if inner.is_empty() {
            return Err(ExecutorError::Fetch(format!(
                "unsupported PostgreSQL index definition with empty column list: {def}"
            )));
        }
        let mut cols = Vec::new();
        for raw in inner.split(',') {
            let stripped = raw.trim();
            if stripped.starts_with('(') {
                return Err(ExecutorError::Fetch(format!(
                    "unsupported PostgreSQL expression index; model index expressions explicitly before introspecting: {def}"
                )));
            }
            // Strip quotes, then drop trailing sort/opclass tokens:
            // e.g. `"col" DESC`, `col varchar_pattern_ops`, `col NULLS FIRST`
            let stripped = stripped.trim_matches('"');
            let col_name = stripped
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches('"');
            if col_name.contains('(') || col_name.contains(')') {
                return Err(ExecutorError::Fetch(format!(
                    "unsupported PostgreSQL expression index; model index expressions explicitly before introspecting: {def}"
                )));
            }
            if !col_name.is_empty() {
                cols.push(col_name.to_string());
            }
        }
        cols
    } else {
        return Err(ExecutorError::Fetch(format!(
            "unsupported PostgreSQL index definition without a column list: {def}"
        )));
    };
    if cols.is_empty() {
        return Err(ExecutorError::Fetch(format!(
            "unsupported PostgreSQL index definition with no simple columns: {def}"
        )));
    }
    Ok((cols, unique, predicate))
}

fn normalize_index_predicate(predicate: &str) -> String {
    let trimmed = predicate.trim();
    if let Some(inner) = strip_balanced_outer_parens(trimmed) {
        inner.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn strip_balanced_outer_parens(value: &str) -> Option<&str> {
    let inner = value.strip_prefix('(')?.strip_suffix(')')?;
    let mut depth = 0usize;
    for (idx, ch) in value.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 && idx != value.len() - 1 {
                    return None;
                }
            }
            _ => {}
        }
    }
    Some(inner)
}

// Decodes the tgtype bitmask from pg_trigger into timing, events, and scope.
// Bitmask values per PostgreSQL source (commands/trigger.h):
//   TRIGGER_TYPE_ROW       0x01
//   TRIGGER_TYPE_BEFORE    0x02
//   TRIGGER_TYPE_INSERT    0x04
//   TRIGGER_TYPE_DELETE    0x08
//   TRIGGER_TYPE_UPDATE    0x10
//   TRIGGER_TYPE_TRUNCATE  0x20
//   TRIGGER_TYPE_INSTEAD   0x40
fn decode_tgtype(tgtype: i16) -> (TriggerTiming, Vec<TriggerEvent>, TriggerScope) {
    let timing = if tgtype & 0x40 != 0 {
        TriggerTiming::InsteadOf
    } else if tgtype & 0x02 != 0 {
        TriggerTiming::Before
    } else {
        TriggerTiming::After
    };
    let mut events = vec![];
    if tgtype & 0x04 != 0 {
        events.push(TriggerEvent::Insert);
    }
    if tgtype & 0x08 != 0 {
        events.push(TriggerEvent::Delete);
    }
    if tgtype & 0x10 != 0 {
        events.push(TriggerEvent::Update);
    }
    if tgtype & 0x20 != 0 {
        events.push(TriggerEvent::Truncate);
    }
    let scope = if tgtype & 0x01 != 0 {
        TriggerScope::Row
    } else {
        TriggerScope::Statement
    };
    (timing, events, scope)
}

impl Introspectable for PostgresExecutor {
    fn inspect_db<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, ExecutorError>> {
        Box::pin(async move {
            let mut state = Schema::default();

            for &schema in schemas {
                let table_rows = sqlx::query(
                    "SELECT table_name FROM information_schema.tables \
                     WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
                     AND table_name != 'gaman_migrations' ORDER BY table_name",
                )
                .bind(schema)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for row in &table_rows {
                    let table_name: String = row
                        .try_get(0)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    let mut table = Table {
                        name: table_name.clone(),
                        schema: if schema == "public" {
                            None
                        } else {
                            Some(schema.to_string())
                        },
                        primary_key: None,
                        columns: vec![],
                        foreign_keys: vec![],
                        indexes: vec![],
                        constraints: vec![],
                        triggers: vec![],
                    };

                    let col_rows = sqlx::query(
                        "SELECT c.column_name, c.data_type, c.udt_schema, c.udt_name, c.character_maximum_length, \
                         c.numeric_precision, c.numeric_scale, c.is_nullable, c.column_default, \
                         a.attidentity, c.generation_expression \
                         FROM information_schema.columns c \
                         JOIN pg_class cl ON cl.relname = c.table_name \
                         JOIN pg_namespace ns ON ns.nspname = c.table_schema AND ns.oid = cl.relnamespace \
                         JOIN pg_attribute a ON a.attrelid = cl.oid AND a.attname = c.column_name \
                         WHERE c.table_schema = $1 AND c.table_name = $2 \
                         ORDER BY c.ordinal_position",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    let pk_rows = sqlx::query(
                        "SELECT tc.constraint_name, kcu.column_name \
                         FROM information_schema.table_constraints tc \
                         JOIN information_schema.key_column_usage kcu \
                           ON tc.constraint_name = kcu.constraint_name \
                           AND tc.table_schema = kcu.table_schema \
                           AND tc.table_name = kcu.table_name \
                         WHERE tc.table_schema = $1 AND tc.table_name = $2 \
                         AND tc.constraint_type = 'PRIMARY KEY' \
                         ORDER BY kcu.ordinal_position",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let mut pk_name: Option<String> = None;
                    let mut pk_cols: Vec<String> = Vec::new();
                    for row in &pk_rows {
                        let name: String = row
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let column: String = row
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        pk_name.get_or_insert(name);
                        pk_cols.push(column);
                    }

                    for cr in &col_rows {
                        let col_name: String = cr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let data_type: String = cr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let udt_schema: String = cr
                            .try_get(2)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let udt_name: String = cr
                            .try_get(3)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let char_max: Option<i32> = cr
                            .try_get(4)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let num_prec: Option<i32> = cr
                            .try_get(5)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let num_scale: Option<i32> = cr
                            .try_get(6)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let is_nullable: String = cr
                            .try_get(7)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let col_default: Option<String> = cr
                            .try_get(8)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        // attidentity is pg "char" (i8): b'a' = ALWAYS, b'd' = BY DEFAULT, 0 = not identity
                        let attidentity: i8 = cr
                            .try_get(9)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let generation_expression: Option<String> = cr
                            .try_get(10)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                        let generated = generation_expression.filter(|s| !s.is_empty());
                        let default = if generated.is_some() {
                            None
                        } else {
                            match attidentity as u8 {
                                b'a' => Some("GENERATED ALWAYS AS IDENTITY".to_string()),
                                b'd' => Some("GENERATED BY DEFAULT AS IDENTITY".to_string()),
                                _ => col_default,
                            }
                        };

                        let col_type = if data_type == "character varying" {
                            match char_max {
                                Some(n) => format!("varchar({})", n),
                                None => "text".to_string(),
                            }
                        } else if data_type == "character" {
                            match char_max {
                                Some(n) => format!("char({})", n),
                                None => "char".to_string(),
                            }
                        } else if data_type == "numeric" || data_type == "decimal" {
                            match (num_prec, num_scale) {
                                (Some(p), Some(s)) => format!("numeric({p}, {s})"),
                                _ => "numeric".to_string(),
                            }
                        } else if data_type == "USER-DEFINED" {
                            schema_qualified_key(&udt_name, Some(&udt_schema))
                        } else {
                            data_type.clone()
                        };

                        let is_pk = pk_cols.contains(&col_name);
                        table.columns.push(Column {
                            name: col_name,
                            col_type,
                            nullable: is_nullable == "YES",
                            default,
                            primary_key: is_pk,
                            references: None,
                            check: None,
                            generated,
                        });
                    }
                    if !pk_cols.is_empty() {
                        table.primary_key = Some(PrimaryKey {
                            name: pk_name.unwrap_or_else(|| table.pk_constraint_name()),
                            columns: pk_cols,
                        });
                    }

                    let fk_rows = sqlx::query(
                        "SELECT c.conname, \
                         a.attname AS from_col, \
                         fn.nspname AS ref_schema, \
                         fc.relname AS ref_table, \
                         fa.attname AS ref_col, \
                         keys.ordinality AS col_ordinality \
                         FROM pg_constraint c \
                         JOIN pg_class t ON t.oid = c.conrelid \
                         JOIN pg_namespace tn ON tn.oid = t.relnamespace \
                         JOIN pg_class fc ON fc.oid = c.confrelid \
                         JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
                         JOIN unnest(c.conkey, c.confkey) WITH ORDINALITY \
                           AS keys(from_attnum, ref_attnum, ordinality) ON TRUE \
                         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = keys.from_attnum \
                         JOIN pg_attribute fa ON fa.attrelid = fc.oid AND fa.attnum = keys.ref_attnum \
                         WHERE c.contype = 'f' AND tn.nspname = $1 AND t.relname = $2 \
                         ORDER BY c.conname, keys.ordinality",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    let mut fk_map: std::collections::BTreeMap<
                        String,
                        (String, Vec<String>, Vec<String>),
                    > = std::collections::BTreeMap::new();
                    for fkr in &fk_rows {
                        let fk_name: String = fkr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let from_col: String = fkr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let ref_schema: String = fkr
                            .try_get(2)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let ref_table: String = fkr
                            .try_get(3)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let ref_col: String = fkr
                            .try_get(4)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let to_table = schema_qualified_key(&ref_table, Some(&ref_schema));
                        let entry = fk_map
                            .entry(fk_name)
                            .or_insert_with(|| (to_table, Vec::new(), Vec::new()));
                        entry.1.push(from_col);
                        entry.2.push(ref_col);
                    }
                    for (name, (to_table, columns, to_columns)) in fk_map {
                        table
                            .foreign_keys
                            .push(ForeignKey::new(name, columns, to_table, to_columns));
                    }

                    let uq_rows = sqlx::query(
                        "SELECT tc.constraint_name, kcu.column_name \
                         FROM information_schema.table_constraints tc \
                         JOIN information_schema.key_column_usage kcu \
                           ON tc.constraint_name = kcu.constraint_name \
                           AND tc.table_schema = kcu.table_schema \
                           AND tc.table_name = kcu.table_name \
                         WHERE tc.table_schema = $1 AND tc.table_name = $2 \
                         AND tc.constraint_type = 'UNIQUE' \
                         ORDER BY tc.constraint_name, kcu.ordinal_position",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    let mut uq_map: std::collections::BTreeMap<String, Vec<String>> =
                        std::collections::BTreeMap::new();
                    for uqr in &uq_rows {
                        let cname: String = uqr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let col: String = uqr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        uq_map.entry(cname).or_default().push(col);
                    }
                    for (name, columns) in uq_map {
                        table.constraints.push(Constraint::Unique { name, columns });
                    }

                    let ck_rows = sqlx::query(
                        "SELECT con.conname, pg_get_constraintdef(con.oid, true) \
                         FROM pg_constraint con \
                         JOIN pg_class rel ON rel.oid = con.conrelid \
                         JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
                         WHERE con.contype = 'c' \
                         AND ns.nspname = $1 AND rel.relname = $2 \
                         AND pg_get_constraintdef(con.oid, true) NOT LIKE '%IS NOT NULL' \
                         ORDER BY con.conname",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    for ckr in &ck_rows {
                        let cname: String = ckr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let expr: String = ckr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let expr = expr
                            .strip_prefix("CHECK (")
                            .and_then(|s| s.strip_suffix(')'))
                            .unwrap_or(&expr)
                            .to_string();
                        table.constraints.push(Constraint::Check {
                            name: cname,
                            expression: expr,
                        });
                    }

                    let idx_rows = sqlx::query(
                        "SELECT i.relname, ix2.indexdef \
                         FROM pg_class t \
                         JOIN pg_index ix ON t.oid = ix.indrelid \
                         JOIN pg_class i ON i.oid = ix.indexrelid \
                         JOIN pg_namespace n ON n.oid = t.relnamespace \
                         JOIN pg_indexes ix2 ON ix2.indexname = i.relname AND ix2.schemaname = n.nspname \
                         WHERE n.nspname = $1 AND t.relname = $2 \
                         AND NOT ix.indisprimary \
                         AND NOT EXISTS ( \
                           SELECT 1 FROM information_schema.table_constraints c \
                           WHERE c.table_schema = n.nspname AND c.table_name = t.relname \
                           AND c.constraint_name = i.relname \
                         ) \
                         ORDER BY i.relname",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    for idr in &idx_rows {
                        let idx_name: String = idr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let idx_def: String = idr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let (cols, unique, predicate) = parse_index_def(&idx_def)?;
                        table.indexes.push(Index {
                            name: idx_name,
                            columns: cols,
                            unique,
                            predicate,
                        });
                    }

                    let tg_rows = sqlx::query(
                        "SELECT t.tgname, t.tgtype, p.proname, n2.nspname \
                         FROM pg_trigger t \
                         JOIN pg_class c ON c.oid = t.tgrelid \
                         JOIN pg_namespace n ON n.oid = c.relnamespace \
                         JOIN pg_proc p ON p.oid = t.tgfoid \
                         JOIN pg_namespace n2 ON n2.oid = p.pronamespace \
                         WHERE n.nspname = $1 AND c.relname = $2 \
                         AND NOT t.tgisinternal \
                         ORDER BY t.tgname",
                    )
                    .bind(schema)
                    .bind(&table_name)
                    .fetch_all(&mut self.conn)
                    .await
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                    for tgr in &tg_rows {
                        let tg_name: String = tgr
                            .try_get(0)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let tgtype: i16 = tgr
                            .try_get(1)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let fn_name: String = tgr
                            .try_get(2)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let fn_schema: String = tgr
                            .try_get(3)
                            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                        let (timing, events, scope) = decode_tgtype(tgtype);
                        let fn_key = schema_qualified_key(&fn_name, Some(&fn_schema));
                        table.triggers.push(TriggerDef {
                            name: Some(tg_name),
                            timing,
                            events,
                            scope,
                            function_name: Some(fn_key),
                            when: None,
                            query: None,
                            language: None,
                        });
                    }

                    state.tables.insert(table.qualified_name(), table);
                }

                let view_rows = sqlx::query(
                    "SELECT table_name, view_definition FROM information_schema.views \
                     WHERE table_schema = $1 ORDER BY table_name",
                )
                .bind(schema)
                .fetch_all(&mut self.conn)
                .await
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for vr in &view_rows {
                    let view_name: String = vr
                        .try_get(0)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let definition: String = vr
                        .try_get(1)
                        .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                    let key = schema_qualified_key(&view_name, Some(schema));
                    state.views.insert(
                        key,
                        ViewDef {
                            name: view_name,
                            schema: if schema == "public" {
                                None
                            } else {
                                Some(schema.to_string())
                            },
                            definition,
                        },
                    );
                }
            }

            let schema_list: Vec<String> = schemas.iter().map(|s| s.to_string()).collect();
            let fn_rows = sqlx::query(
                "SELECT p.proname, n.nspname, \
                 pg_get_function_identity_arguments(p.oid) AS args, \
                 p.prosrc AS body, \
                 l.lanname, \
                 p.provolatile, \
                 p.prosecdef, \
                 pg_get_function_result(p.oid) AS returns \
                 FROM pg_proc p \
                 JOIN pg_namespace n ON n.oid = p.pronamespace \
                 JOIN pg_language l ON l.oid = p.prolang \
                 WHERE p.prokind = 'f' \
                 AND l.lanname NOT IN ('internal', 'c') \
                 AND n.nspname = ANY($1) \
                 ORDER BY n.nspname, p.proname",
            )
            .bind(&schema_list[..])
            .fetch_all(&mut self.conn)
            .await
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            for fnr in &fn_rows {
                let fn_name: String = fnr
                    .try_get(0)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let fn_schema: String = fnr
                    .try_get(1)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let args: String = fnr
                    .try_get(2)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let body: String = fnr
                    .try_get(3)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let language: String = fnr
                    .try_get(4)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let provolatile: i8 = fnr
                    .try_get(5)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let security_definer: bool = fnr
                    .try_get(6)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let returns: String = fnr
                    .try_get(7)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                let volatility = match provolatile as u8 {
                    b'i' => Volatility::Immutable,
                    b's' => Volatility::Stable,
                    _ => Volatility::Volatile,
                };

                let key = schema_qualified_key(&fn_name, Some(&fn_schema));
                let final_key = if args.is_empty() {
                    key
                } else {
                    format!("{}({})", key, args)
                };

                state.functions.insert(
                    final_key,
                    FunctionDef {
                        name: fn_name,
                        schema: if fn_schema == "public" {
                            None
                        } else {
                            Some(fn_schema)
                        },
                        arguments: args,
                        returns,
                        language,
                        body,
                        volatility,
                        security_definer,
                    },
                );
            }

            let ext_rows = sqlx::query(
                "SELECT e.extname, n.nspname, e.extversion \
                 FROM pg_extension e \
                 JOIN pg_namespace n ON n.oid = e.extnamespace \
                 WHERE n.nspname = ANY($1) \
                 ORDER BY n.nspname, e.extname",
            )
            .bind(&schema_list[..])
            .fetch_all(&mut self.conn)
            .await
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            for row in &ext_rows {
                let name: String = row
                    .try_get(0)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let schema: String = row
                    .try_get(1)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let version: Option<String> = row
                    .try_get(2)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let extension = ExtensionDef {
                    name,
                    schema: if schema == "public" {
                        None
                    } else {
                        Some(schema)
                    },
                    version,
                };
                state
                    .extensions
                    .insert(extension.qualified_name(), extension);
            }

            let enum_rows = sqlx::query(
                "SELECT t.typname, n.nspname, e.enumlabel \
                 FROM pg_type t \
                 JOIN pg_enum e ON e.enumtypid = t.oid \
                 JOIN pg_namespace n ON n.oid = t.typnamespace \
                 WHERE n.nspname = ANY($1) \
                 ORDER BY n.nspname, t.typname, e.enumsortorder",
            )
            .bind(&schema_list[..])
            .fetch_all(&mut self.conn)
            .await
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            let mut enum_map: std::collections::BTreeMap<String, EnumDef> =
                std::collections::BTreeMap::new();
            for row in &enum_rows {
                let name: String = row
                    .try_get(0)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let schema: String = row
                    .try_get(1)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let label: String = row
                    .try_get(2)
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let schema = if schema == "public" {
                    None
                } else {
                    Some(schema)
                };
                let key = schema_qualified_key(&name, schema.as_deref());
                enum_map
                    .entry(key)
                    .or_insert_with(|| EnumDef {
                        name,
                        schema,
                        values: Vec::new(),
                    })
                    .values
                    .push(label);
            }
            state.enums.extend(enum_map);

            Ok(state)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that a simple non-unique btree index definition is parsed correctly.
    #[test]
    fn parse_index_def_non_unique() {
        let def = "CREATE INDEX idx_users_email ON public.users USING btree (email)";
        let (cols, unique, predicate) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["email"]);
        assert!(!unique);
        assert!(predicate.is_none());
    }

    /// Verifies that a unique multi-column index is parsed correctly.
    #[test]
    fn parse_index_def_unique_multi_col() {
        let def =
            r#"CREATE UNIQUE INDEX idx ON public.orders USING btree ("tenant_id", order_num)"#;
        let (cols, unique, predicate) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["tenant_id", "order_num"]);
        assert!(unique);
        assert!(predicate.is_none());
    }

    /// Verifies that an empty indexdef string fails instead of producing lossy metadata.
    #[test]
    fn parse_index_def_empty() {
        let err = parse_index_def("").unwrap_err();
        assert!(err.to_string().contains("without a column list"));
    }

    /// Verifies that a partial index has its WHERE predicate extracted and columns are clean.
    #[test]
    fn parse_index_def_partial_index() {
        let def =
            "CREATE INDEX idx ON public.tasks USING btree (status, ready_time) WHERE (status = 0)";
        let (cols, unique, predicate) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["status", "ready_time"]);
        assert!(!unique);
        assert_eq!(predicate.as_deref(), Some("status = 0"));
    }

    /// Verifies unparenthesized PostgreSQL partial-index predicates are preserved.
    #[test]
    fn parse_index_def_partial_index_without_predicate_parens() {
        let def = "CREATE INDEX idx ON public.users USING btree (email) WHERE active";
        let (cols, unique, predicate) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["email"]);
        assert!(!unique);
        assert_eq!(predicate.as_deref(), Some("active"));
    }

    /// Verifies that DESC sort modifiers and operator classes are stripped from column names.
    #[test]
    fn parse_index_def_strips_modifiers() {
        let def =
            "CREATE INDEX idx ON t USING btree (provider_id, created DESC) WHERE (deleted IS NULL)";
        let (cols, _, predicate) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["provider_id", "created"]);
        assert_eq!(predicate.as_deref(), Some("deleted IS NULL"));
    }

    /// Verifies that varchar operator classes are stripped from LIKE-support indexes.
    #[test]
    fn parse_index_def_strips_opclass() {
        let def = "CREATE INDEX idx ON t USING btree (username varchar_pattern_ops)";
        let (cols, _, _) = parse_index_def(def).unwrap();
        assert_eq!(cols, vec!["username"]);
    }

    #[test]
    fn parse_index_def_rejects_expression_indexes() {
        let def = "CREATE INDEX idx ON t USING btree ((lower(username)))";
        let err = parse_index_def(def).unwrap_err();
        assert!(err.to_string().contains("expression index"));
    }

    /// Verifies function-call expression indexes are rejected even without double parentheses.
    #[test]
    fn parse_index_def_rejects_function_expression_indexes() {
        let def = "CREATE INDEX idx ON t USING btree (lower(username))";
        let err = parse_index_def(def).unwrap_err();
        assert!(err.to_string().contains("expression index"));
    }

    /// Verifies BEFORE INSERT ROW trigger decoding.
    #[test]
    fn decode_tgtype_before_insert_row() {
        // TRIGGER_TYPE_BEFORE(0x02) | TRIGGER_TYPE_INSERT(0x04) | TRIGGER_TYPE_ROW(0x01)
        let (timing, events, scope) = decode_tgtype(0x02 | 0x04 | 0x01);
        assert_eq!(timing, TriggerTiming::Before);
        assert_eq!(events, vec![TriggerEvent::Insert]);
        assert_eq!(scope, TriggerScope::Row);
    }

    /// Verifies AFTER UPDATE and DELETE STATEMENT trigger decoding.
    #[test]
    fn decode_tgtype_after_update_delete_statement() {
        let tgtype: i16 = 0x08 | 0x10; // DELETE | UPDATE, statement level
        let (timing, events, scope) = decode_tgtype(tgtype);
        assert_eq!(timing, TriggerTiming::After);
        assert!(events.contains(&TriggerEvent::Delete));
        assert!(events.contains(&TriggerEvent::Update));
        assert!(!events.contains(&TriggerEvent::Insert));
        assert_eq!(scope, TriggerScope::Statement);
    }

    /// Verifies INSTEAD OF trigger decoding.
    #[test]
    fn decode_tgtype_instead_of() {
        let tgtype: i16 = 0x40 | 0x04; // INSTEAD_OF | INSERT
        let (timing, events, scope) = decode_tgtype(tgtype);
        assert_eq!(timing, TriggerTiming::InsteadOf);
        assert_eq!(events, vec![TriggerEvent::Insert]);
        assert_eq!(scope, TriggerScope::Statement);
    }

    /// Verifies TRUNCATE AFTER STATEMENT trigger.
    #[test]
    fn decode_tgtype_truncate_after_statement() {
        let tgtype: i16 = 0x20;
        let (timing, events, scope) = decode_tgtype(tgtype);
        assert_eq!(timing, TriggerTiming::After);
        assert_eq!(events, vec![TriggerEvent::Truncate]);
        assert_eq!(scope, TriggerScope::Statement);
    }
}
