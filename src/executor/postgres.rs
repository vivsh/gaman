use postgres::Client;

use crate::states::{
    Column, Constraint, ForeignKey, FunctionDef, Index, SchemaState, Table, TriggerDef,
    TriggerEvent, TriggerScope, TriggerTiming, ViewDef, Volatility, schema_qualified_key,
};
use super::{Executor, ExecutorError, Introspectable};

// Stable advisory lock key for all gaman instances. Session-scoped — released automatically
// when the connection closes, so a crashed process can never leave a stale lock.
const GAMAN_LOCK_KEY: i64 = 7242068691819328000;

/// Wraps a live Postgres client and manages transaction boundaries explicitly.
/// Call `begin()` before a migration and `commit()` or `rollback()` after.
pub struct PostgresExecutor {
    client: Client,
}

impl PostgresExecutor {
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

fn pg_error_message(e: &postgres::Error) -> String {
    if let Some(db) = e.as_db_error() {
        let mut msg = db.message().to_string();
        if let Some(detail) = db.detail() {
            msg.push_str(&format!("\n  DETAIL: {detail}"));
        }
        if let Some(hint) = db.hint() {
            msg.push_str(&format!("\n  HINT: {hint}"));
        }
        msg
    } else {
        e.to_string()
    }
}

impl Executor for PostgresExecutor {
    fn execute(&mut self, sql: &str) -> Result<(), ExecutorError> {
        self.client
            .execute(sql, &[])
            .map_err(|e| ExecutorError::Execute(format!("{}\n  SQL: {sql}", pg_error_message(&e))))?;
        Ok(())
    }

    fn fetch_strings(&mut self, sql: &str) -> Result<Vec<String>, ExecutorError> {
        let rows = self.client
            .query(sql, &[])
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
        Ok(rows.into_iter().map(|r| r.get::<_, String>(0)).collect())
    }

    fn begin(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("BEGIN", &[]).map_err(|e| ExecutorError::Transaction(e.to_string()))?;
        Ok(())
    }

    fn commit(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("COMMIT", &[]).map_err(|e| ExecutorError::Transaction(e.to_string()))?;
        Ok(())
    }

    fn rollback(&mut self) -> Result<(), ExecutorError> {
        self.client.execute("ROLLBACK", &[]).map_err(|e| ExecutorError::Transaction(e.to_string()))?;
        Ok(())
    }

    fn acquire_lock(&mut self) -> Result<(), ExecutorError> {
        self.client
            .execute("SET lock_timeout = '30s'", &[])
            .map_err(|e| ExecutorError::Execute(e.to_string()))?;
        self.client
            .execute(&format!("SELECT pg_advisory_lock({GAMAN_LOCK_KEY})"), &[])
            .map_err(|e| ExecutorError::Execute(format!("could not acquire migration lock: {e}")))?;
        Ok(())
    }

    fn release_lock(&mut self) -> Result<(), ExecutorError> {
        self.client
            .execute(&format!("SELECT pg_advisory_unlock({GAMAN_LOCK_KEY})"), &[])
            .map_err(|e| ExecutorError::Execute(format!("could not release migration lock: {e}")))?;
        Ok(())
    }
}

// Extracts column list, UNIQUE flag, and optional partial-index predicate from a
// pg_indexes.indexdef string.
// e.g. "CREATE UNIQUE INDEX idx ON t USING btree (a, b DESC) WHERE (deleted IS NULL)"
fn parse_index_def(def: &str) -> (Vec<String>, bool, Option<String>) {
    let unique = def.contains("CREATE UNIQUE INDEX");

    // Split off the optional WHERE clause before parsing columns
    let (col_part, predicate) = if let Some(where_pos) = def.find(") WHERE (") {
        let pred = def[where_pos + 9..].trim_end_matches(')').trim().to_string();
        (&def[..where_pos + 1], Some(pred))
    } else {
        (def, None)
    };

    let cols = if let Some(start) = col_part.find('(') {
        let inner = &col_part[start + 1..];
        let end = inner.rfind(')').unwrap_or(inner.len());
        inner[..end]
            .split(',')
            .map(|s| {
                // Strip quotes, then drop trailing sort/opclass tokens:
                // e.g. `"col" DESC`, `col varchar_pattern_ops`, `col NULLS FIRST`
                let stripped = s.trim().trim_matches('"');
                // Take only the first whitespace-delimited token as the column name
                let col_name = stripped.split_whitespace().next().unwrap_or("").trim_matches('"');
                col_name.to_string()
            })
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![]
    };
    (cols, unique, predicate)
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
    if tgtype & 0x04 != 0 { events.push(TriggerEvent::Insert); }
    if tgtype & 0x08 != 0 { events.push(TriggerEvent::Delete); }
    if tgtype & 0x10 != 0 { events.push(TriggerEvent::Update); }
    if tgtype & 0x20 != 0 { events.push(TriggerEvent::Truncate); }
    let scope = if tgtype & 0x01 != 0 { TriggerScope::Row } else { TriggerScope::Statement };
    (timing, events, scope)
}

impl Introspectable for PostgresExecutor {
    fn inspect_db(&mut self, schemas: &[&str]) -> Result<SchemaState, ExecutorError> {
        let mut state = SchemaState::default();

        for &schema in schemas {
            // Tables
            let table_rows = self.client
                .query(
                    "SELECT table_name FROM information_schema.tables \
                     WHERE table_schema = $1 AND table_type = 'BASE TABLE' \
                     AND table_name != 'gaman_migrations' ORDER BY table_name",
                    &[&schema],
                )
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            for row in &table_rows {
                let table_name: String = row.get(0);
                let key = table_name.clone();

                let mut table = Table {
                    name: table_name.clone(),
                    schema: if schema == "public" { None } else { Some(schema.to_string()) },
                    columns: vec![],
                    foreign_keys: vec![],
                    indexes: vec![],
                    constraints: vec![],
                    triggers: vec![],
                };

                // Columns — join pg_attribute to pick up identity column metadata
                // that information_schema.columns does not expose.
                let col_rows = self.client
                    .query(
                        "SELECT c.column_name, c.data_type, c.character_maximum_length, \
                         c.numeric_precision, c.numeric_scale, c.is_nullable, c.column_default, \
                         a.attidentity \
                         FROM information_schema.columns c \
                         JOIN pg_class cl ON cl.relname = c.table_name \
                         JOIN pg_namespace ns ON ns.nspname = c.table_schema AND ns.oid = cl.relnamespace \
                         JOIN pg_attribute a ON a.attrelid = cl.oid AND a.attname = c.column_name \
                         WHERE c.table_schema = $1 AND c.table_name = $2 \
                         ORDER BY c.ordinal_position",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                // Collect PK columns first so we can mark primary_key on the Column struct.
                let pk_rows = self.client
                    .query(
                        "SELECT kcu.column_name \
                         FROM information_schema.table_constraints tc \
                         JOIN information_schema.key_column_usage kcu \
                           ON tc.constraint_name = kcu.constraint_name \
                           AND tc.table_schema = kcu.table_schema \
                           AND tc.table_name = kcu.table_name \
                         WHERE tc.table_schema = $1 AND tc.table_name = $2 \
                         AND tc.constraint_type = 'PRIMARY KEY' \
                         ORDER BY kcu.ordinal_position",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;
                let pk_cols: Vec<String> = pk_rows.iter().map(|r| r.get::<_, String>(0)).collect();

                for cr in &col_rows {
                    let col_name: String = cr.get(0);
                    let data_type: String = cr.get(1);
                    let char_max: Option<i32> = cr.get(2);
                    let num_prec: Option<i32> = cr.get(3);
                    let num_scale: Option<i32> = cr.get(4);
                    let is_nullable: String = cr.get(5);
                    let col_default: Option<String> = cr.get(6);
                    // attidentity is pg "char" (i8): b'a' = ALWAYS, b'd' = BY DEFAULT, 0 = not identity
                    let attidentity: i8 = cr.get(7);

                    let default = match attidentity as u8 {
                        b'a' => Some("GENERATED ALWAYS AS IDENTITY".to_string()),
                        b'd' => Some("GENERATED BY DEFAULT AS IDENTITY".to_string()),
                        _ => col_default,
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
                    });
                }

                // Foreign keys
                let fk_rows = self.client
                    .query(
                        "SELECT c.conname, \
                         a.attname AS from_col, \
                         fn.nspname AS ref_schema, \
                         fc.relname AS ref_table, \
                         fa.attname AS ref_col \
                         FROM pg_constraint c \
                         JOIN pg_class t ON t.oid = c.conrelid \
                         JOIN pg_namespace tn ON tn.oid = t.relnamespace \
                         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = ANY(c.conkey) \
                         JOIN pg_class fc ON fc.oid = c.confrelid \
                         JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
                         JOIN pg_attribute fa ON fa.attrelid = fc.oid AND fa.attnum = ANY(c.confkey) \
                         WHERE c.contype = 'f' AND tn.nspname = $1 AND t.relname = $2 \
                         ORDER BY c.conname, array_position(c.conkey, a.attnum)",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for fkr in &fk_rows {
                    let fk_name: String = fkr.get(0);
                    let from_col: String = fkr.get(1);
                    let _ref_schema: String = fkr.get(2);
                    let ref_table: String = fkr.get(3);
                    let ref_col: String = fkr.get(4);
                    let to_table = ref_table.clone();
                    table.foreign_keys.push(ForeignKey {
                        name: fk_name,
                        from_column: from_col,
                        to_table,
                        to_column: ref_col,
                    });
                }

                // Unique constraints — one row per column, accumulated in Rust
                let uq_rows = self.client
                    .query(
                        "SELECT tc.constraint_name, kcu.column_name \
                         FROM information_schema.table_constraints tc \
                         JOIN information_schema.key_column_usage kcu \
                           ON tc.constraint_name = kcu.constraint_name \
                           AND tc.table_schema = kcu.table_schema \
                         WHERE tc.table_schema = $1 AND tc.table_name = $2 \
                         AND tc.constraint_type = 'UNIQUE' \
                         ORDER BY tc.constraint_name, kcu.ordinal_position",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                let mut uq_map: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();
                for uqr in &uq_rows {
                    let cname: String = uqr.get(0);
                    let col: String = uqr.get(1);
                    uq_map.entry(cname).or_default().push(col);
                }
                for (name, columns) in uq_map {
                    table.constraints.push(Constraint::Unique { name, columns });
                }

                // Check constraints (skip _not_null auto-generated ones)
                let ck_rows = self.client
                    .query(
                        "SELECT tc.constraint_name, cc.check_clause \
                         FROM information_schema.table_constraints tc \
                         JOIN information_schema.check_constraints cc \
                           ON tc.constraint_name = cc.constraint_name \
                           AND tc.table_schema = cc.constraint_schema \
                         WHERE tc.table_schema = $1 AND tc.table_name = $2 \
                         AND tc.constraint_type = 'CHECK' \
                         AND cc.check_clause NOT LIKE '%IS NOT NULL' \
                         ORDER BY tc.constraint_name",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for ckr in &ck_rows {
                    let cname: String = ckr.get(0);
                    let expr: String = ckr.get(1);
                    table.constraints.push(Constraint::Check { name: cname, expression: expr });
                }

                // Indexes — skip those backing constraints and the PK
                let idx_rows = self.client
                    .query(
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
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for idr in &idx_rows {
                    let idx_name: String = idr.get(0);
                    let idx_def: String = idr.get(1);
                    let (cols, unique, predicate) = parse_index_def(&idx_def);
                    table.indexes.push(Index { name: idx_name, columns: cols, unique, predicate });
                }

                // Triggers
                let tg_rows = self.client
                    .query(
                        "SELECT t.tgname, t.tgtype, p.proname, n2.nspname \
                         FROM pg_trigger t \
                         JOIN pg_class c ON c.oid = t.tgrelid \
                         JOIN pg_namespace n ON n.oid = c.relnamespace \
                         JOIN pg_proc p ON p.oid = t.tgfoid \
                         JOIN pg_namespace n2 ON n2.oid = p.pronamespace \
                         WHERE n.nspname = $1 AND c.relname = $2 \
                         AND NOT t.tgisinternal \
                         ORDER BY t.tgname",
                        &[&schema, &table_name],
                    )
                    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

                for tgr in &tg_rows {
                    let tg_name: String = tgr.get(0);
                    let tgtype: i16 = tgr.get(1);
                    let fn_name: String = tgr.get(2);
                    let fn_schema: String = tgr.get(3);
                    let (timing, events, scope) = decode_tgtype(tgtype);
                    let fn_key = schema_qualified_key(&fn_name, Some(&fn_schema));
                    table.triggers.push(TriggerDef {
                        name: Some(tg_name),
                        timing,
                        events,
                        scope,
                        function_name: Some(fn_key),
                        when: None,
                        body: None,
                        language: None,
                    });
                }

                state.tables.insert(key, table);
            }

            // Views
            let view_rows = self.client
                .query(
                    "SELECT table_name, view_definition FROM information_schema.views \
                     WHERE table_schema = $1 ORDER BY table_name",
                    &[&schema],
                )
                .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

            for vr in &view_rows {
                let view_name: String = vr.get(0);
                let definition: String = vr.get(1);
                let key = schema_qualified_key(&view_name, Some(schema));
                state.views.insert(key, ViewDef {
                    name: view_name,
                    schema: if schema == "public" { None } else { Some(schema.to_string()) },
                    definition,
                });
            }
        }

        // Functions — one query covering all requested schemas
        let fn_rows = self.client
            .query(
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
                &[&schemas.to_vec()],
            )
            .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

        for fnr in &fn_rows {
            let fn_name: String = fnr.get(0);
            let fn_schema: String = fnr.get(1);
            let args: String = fnr.get(2);
            let body: String = fnr.get(3);
            let language: String = fnr.get(4);
            let provolatile: i8 = fnr.get(5);
            let security_definer: bool = fnr.get(6);
            let returns: String = fnr.get(7);

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

            state.functions.insert(final_key, FunctionDef {
                name: fn_name,
                schema: if fn_schema == "public" { None } else { Some(fn_schema) },
                arguments: args,
                returns,
                language,
                body,
                volatility,
                security_definer,
            });
        }

        Ok(state)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that a simple non-unique btree index definition is parsed correctly.
    #[test]
    fn parse_index_def_non_unique() {
        let def = "CREATE INDEX idx_users_email ON public.users USING btree (email)";
        let (cols, unique, predicate) = parse_index_def(def);
        assert_eq!(cols, vec!["email"]);
        assert!(!unique);
        assert!(predicate.is_none());
    }

    /// Verifies that a unique multi-column index is parsed correctly.
    #[test]
    fn parse_index_def_unique_multi_col() {
        let def = r#"CREATE UNIQUE INDEX idx ON public.orders USING btree ("tenant_id", order_num)"#;
        let (cols, unique, predicate) = parse_index_def(def);
        assert_eq!(cols, vec!["tenant_id", "order_num"]);
        assert!(unique);
        assert!(predicate.is_none());
    }

    /// Verifies that an empty indexdef string returns no columns and non-unique.
    #[test]
    fn parse_index_def_empty() {
        let (cols, unique, predicate) = parse_index_def("");
        assert!(cols.is_empty());
        assert!(!unique);
        assert!(predicate.is_none());
    }

    /// Verifies that a partial index has its WHERE predicate extracted and columns are clean.
    #[test]
    fn parse_index_def_partial_index() {
        let def = "CREATE INDEX idx ON public.tasks USING btree (status, ready_time) WHERE (status = 0)";
        let (cols, unique, predicate) = parse_index_def(def);
        assert_eq!(cols, vec!["status", "ready_time"]);
        assert!(!unique);
        assert_eq!(predicate.as_deref(), Some("status = 0"));
    }

    /// Verifies that DESC sort modifiers and operator classes are stripped from column names.
    #[test]
    fn parse_index_def_strips_modifiers() {
        let def = "CREATE INDEX idx ON t USING btree (provider_id, created DESC) WHERE (deleted IS NULL)";
        let (cols, _, predicate) = parse_index_def(def);
        assert_eq!(cols, vec!["provider_id", "created"]);
        assert_eq!(predicate.as_deref(), Some("deleted IS NULL"));
    }

    /// Verifies that varchar operator classes are stripped from LIKE-support indexes.
    #[test]
    fn parse_index_def_strips_opclass() {
        let def = "CREATE INDEX idx ON t USING btree (username varchar_pattern_ops)";
        let (cols, _, _) = parse_index_def(def);
        assert_eq!(cols, vec!["username"]);
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
