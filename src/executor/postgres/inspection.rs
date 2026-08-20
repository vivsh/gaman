use std::collections::BTreeMap;

use sqlx::PgConnection;

use super::{BoxFuture, ExecutorError, InspectionError, SchemaInspector};
use gaman_core::{Dialect, TRACKING_TABLE};
use gaman_core::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, OpaqueMeta,
    PrimaryKey, Schema, SequenceDef, Table, TableOptionsMeta, TriggerDef, TriggerEvent, TriggerScope,
    TriggerTiming, ViewDef, Volatility, schema_qualified_key,
};

/// Wraps a live Postgres connection and manages transaction boundaries explicitly.
/// Call `begin()` before a migration and `commit()` or `rollback()` after.
use super::PostgresExecutor;
impl SchemaInspector for PostgresExecutor {
    fn inspect<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        Box::pin(async move {
            inspect_postgres_schema(&mut self.conn, schemas)
                .await
                .map_err(|error| InspectionError::query(error.to_string()))
        })
    }
}

#[derive(sqlx::FromRow)]
struct PgTableRow {
    oid: i64,
    schema_name: String,
    table_name: String,
}

#[derive(sqlx::FromRow)]
struct PgColumnRow {
    table_oid: i64,
    name: String,
    type_display: String,
    nullable: bool,
    default_expr: Option<String>,
    identity_kind: String,
    generated_kind: String,
    generated_expr: Option<String>,
    has_owned_sequence: bool,
}

#[derive(sqlx::FromRow)]
struct PgKeyColumnRow {
    table_oid: i64,
    constraint_name: String,
    column_name: String,
}

#[derive(sqlx::FromRow)]
struct PgForeignKeyRow {
    table_oid: i64,
    constraint_name: String,
    from_column: String,
    ref_schema: String,
    ref_table: String,
    ref_column: String,
    on_delete: Option<String>,
    on_update: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PgCheckRow {
    table_oid: i64,
    constraint_name: String,
    definition: String,
}

#[derive(sqlx::FromRow)]
struct PgIndexRow {
    table_oid: i64,
    index_name: String,
    unique: bool,
    method: String,
    raw: String,
    predicate: Option<String>,
    columns: Vec<Option<String>>,
    element_defs: Vec<String>,
    has_sort_options: bool,
    has_explicit_collation: bool,
    has_nondefault_opclass: bool,
}

#[derive(sqlx::FromRow)]
struct PgTriggerRow {
    table_oid: i64,
    trigger_name: String,
    tgtype: i16,
    function_name: String,
    function_schema: String,
    language: String,
}

#[derive(sqlx::FromRow)]
struct PgViewRow {
    name: String,
    schema_name: String,
    definition: String,
}

#[derive(sqlx::FromRow)]
struct PgFunctionRow {
    name: String,
    schema_name: String,
    arguments: String,
    body: String,
    language: String,
    volatility: i8,
    security_definer: bool,
    returns: String,
}

#[derive(sqlx::FromRow)]
struct PgExtensionRow {
    name: String,
    schema_name: String,
    version: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PgSequenceRow {
    name: String,
    schema_name: String,
}

#[derive(sqlx::FromRow)]
struct PgEnumRow {
    name: String,
    schema_name: String,
    label: String,
}

async fn inspect_postgres_schema(
    conn: &mut PgConnection,
    schemas: &[&str],
) -> Result<Schema, ExecutorError> {
    let schema_list: Vec<String> = schemas.iter().map(|schema| schema.to_string()).collect();
    let mut tables = fetch_tables(conn, &schema_list).await?;
    attach_columns(conn, &schema_list, &mut tables).await?;
    attach_primary_keys(conn, &schema_list, &mut tables).await?;
    attach_foreign_keys(conn, &schema_list, &mut tables).await?;
    attach_unique_constraints(conn, &schema_list, &mut tables).await?;
    attach_check_constraints(conn, &schema_list, &mut tables).await?;
    attach_opaque_constraints(conn, &schema_list, &mut tables).await?;
    attach_indexes(conn, &schema_list, &mut tables).await?;
    attach_triggers(conn, &schema_list, &mut tables).await?;

    Ok(Schema {
        tables: tables
            .into_values()
            .map(|table| (table.qualified_name(), table))
            .collect(),
        views: fetch_views(conn, &schema_list).await?,
        functions: fetch_functions(conn, &schema_list).await?,
        extensions: fetch_extensions(conn, &schema_list).await?,
        sequences: fetch_sequences(conn, &schema_list).await?,
        enums: fetch_enums(conn, &schema_list).await?,
        managed_rows: BTreeMap::new(),
    })
}

async fn fetch_tables(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<i64, Table>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgTableRow>(
        "SELECT c.oid::int8 AS oid, n.nspname AS schema_name, c.relname AS table_name \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind IN ('r', 'p') \
         AND n.nspname = ANY($1) \
         AND c.relname != $2 \
         ORDER BY n.nspname, c.relname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let table = Table {
                name: row.table_name,
                schema: schema_for_output(&row.schema_name),
                primary_key: None,
                columns: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                triggers: Vec::new(),
                options: TableOptionsMeta::default(),
            };
            (row.oid, table)
        })
        .collect())
}

async fn attach_columns(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgColumnRow>(
        "SELECT cl.oid::int8 AS table_oid, a.attname AS name, \
         format_type(a.atttypid, a.atttypmod) AS type_display, \
         NOT a.attnotnull AS nullable, pg_get_expr(ad.adbin, ad.adrelid) AS default_expr, \
         a.attidentity::text AS identity_kind, a.attgenerated::text AS generated_kind, \
         CASE WHEN a.attgenerated <> '' THEN pg_get_expr(ad.adbin, ad.adrelid) ELSE NULL END AS generated_expr, \
         pg_get_serial_sequence(format('%I.%I', n.nspname, cl.relname), a.attname) IS NOT NULL AS has_owned_sequence \
         FROM pg_class cl \
         JOIN pg_namespace n ON n.oid = cl.relnamespace \
         JOIN pg_attribute a ON a.attrelid = cl.oid \
         LEFT JOIN pg_attrdef ad ON ad.adrelid = a.attrelid AND ad.adnum = a.attnum \
         WHERE cl.relkind IN ('r', 'p') \
         AND n.nspname = ANY($1) \
         AND cl.relname != $2 \
         AND a.attnum > 0 AND NOT a.attisdropped \
         ORDER BY cl.oid, a.attnum",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    for row in rows {
        if let Some(table) = tables.get_mut(&row.table_oid) {
            table.columns.push(column_from_row(row));
        }
    }
    Ok(())
}

fn column_from_row(row: PgColumnRow) -> Column {
    let generated = (!row.generated_kind.is_empty())
        .then_some(row.generated_expr)
        .flatten();
    let mut default = if generated.is_some() {
        None
    } else {
        match row.identity_kind.as_str() {
            "a" => Some("GENERATED ALWAYS AS IDENTITY".to_string()),
            "d" => Some("GENERATED BY DEFAULT AS IDENTITY".to_string()),
            _ => row.default_expr,
        }
    };
    let mut col_type = row.type_display;
    if row.has_owned_sequence
        && row.identity_kind.is_empty()
        && default
            .as_deref()
            .is_some_and(|expr| expr.trim_start().starts_with("nextval("))
        && let Some(serial_type) = serial_type_for(&col_type)
    {
        col_type = serial_type.to_string();
        default = None;
    }
    Column {
        name: row.name,
        col_type,
        nullable: row.nullable,
        default,
        primary_key: false,
        references: None,
        check: None,
        generated,
        generated_storage: None,
        dialect_options: Default::default(),
    }
}

fn serial_type_for(col_type: &str) -> Option<&'static str> {
    match col_type {
        "integer" => Some("serial"),
        "bigint" => Some("bigserial"),
        "smallint" => Some("smallserial"),
        _ => None,
    }
}

async fn attach_primary_keys(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = key_column_rows(conn, schemas, "p").await?;
    let mut keys: BTreeMap<i64, (String, Vec<String>)> = BTreeMap::new();
    for row in rows {
        let entry = keys
            .entry(row.table_oid)
            .or_insert_with(|| (row.constraint_name, Vec::new()));
        entry.1.push(row.column_name);
    }
    for (oid, (name, columns)) in keys {
        if let Some(table) = tables.get_mut(&oid) {
            for column in &mut table.columns {
                column.primary_key = columns.iter().any(|name| name == &column.name);
                if column.primary_key {
                    column.nullable = false;
                }
            }
            table.primary_key = Some(PrimaryKey { name, columns });
        }
    }
    Ok(())
}

async fn attach_unique_constraints(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = key_column_rows(conn, schemas, "u").await?;
    let mut keys: BTreeMap<(i64, String), Vec<String>> = BTreeMap::new();
    for row in rows {
        keys.entry((row.table_oid, row.constraint_name))
            .or_default()
            .push(row.column_name);
    }
    for ((oid, name), columns) in keys {
        if let Some(table) = tables.get_mut(&oid) {
            table.constraints.push(Constraint::Unique { name, columns });
        }
    }
    Ok(())
}

async fn key_column_rows(
    conn: &mut PgConnection,
    schemas: &[String],
    contype: &str,
) -> Result<Vec<PgKeyColumnRow>, ExecutorError> {
    sqlx::query_as::<_, PgKeyColumnRow>(
        "SELECT rel.oid::int8 AS table_oid, con.conname AS constraint_name, a.attname AS column_name \
         FROM pg_constraint con \
         JOIN pg_class rel ON rel.oid = con.conrelid \
         JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
         JOIN unnest(con.conkey) WITH ORDINALITY AS keys(attnum, ordinality) ON TRUE \
         JOIN pg_attribute a ON a.attrelid = rel.oid AND a.attnum = keys.attnum \
         WHERE con.contype = $2 AND ns.nspname = ANY($1) \
         AND rel.relname != $3 \
         ORDER BY rel.oid, con.conname, keys.ordinality",
    )
    .bind(schemas)
    .bind(contype)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))
}

async fn attach_foreign_keys(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgForeignKeyRow>(
        "SELECT t.oid::int8 AS table_oid, c.conname AS constraint_name, \
         a.attname AS from_column, fn.nspname AS ref_schema, fc.relname AS ref_table, \
         fa.attname AS ref_column, \
         CASE c.confdeltype \
           WHEN 'c' THEN 'cascade' \
           WHEN 'r' THEN 'restrict' \
           WHEN 'n' THEN 'set_null' \
           WHEN 'd' THEN 'set_default' \
           ELSE NULL \
         END AS on_delete, \
         CASE c.confupdtype \
           WHEN 'c' THEN 'cascade' \
           WHEN 'r' THEN 'restrict' \
           WHEN 'n' THEN 'set_null' \
           WHEN 'd' THEN 'set_default' \
           ELSE NULL \
         END AS on_update \
         FROM pg_constraint c \
         JOIN pg_class t ON t.oid = c.conrelid \
         JOIN pg_namespace tn ON tn.oid = t.relnamespace \
         JOIN pg_class fc ON fc.oid = c.confrelid \
         JOIN pg_namespace fn ON fn.oid = fc.relnamespace \
         JOIN unnest(c.conkey, c.confkey) WITH ORDINALITY AS keys(from_attnum, ref_attnum, ordinality) ON TRUE \
         JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = keys.from_attnum \
         JOIN pg_attribute fa ON fa.attrelid = fc.oid AND fa.attnum = keys.ref_attnum \
         WHERE c.contype = 'f' AND tn.nspname = ANY($1) \
         AND t.relname != $2 \
         ORDER BY t.oid, c.conname, keys.ordinality",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    type ForeignKeyParts = (
        String,
        Vec<String>,
        Vec<String>,
        Option<String>,
        Option<String>,
    );
    let mut keys: BTreeMap<(i64, String), ForeignKeyParts> = BTreeMap::new();
    for row in rows {
        let to_table = schema_qualified_key(&row.ref_table, Some(&row.ref_schema));
        let entry = keys
            .entry((row.table_oid, row.constraint_name))
            .or_insert_with(|| {
                (
                    to_table,
                    Vec::new(),
                    Vec::new(),
                    row.on_delete,
                    row.on_update,
                )
            });
        entry.1.push(row.from_column);
        entry.2.push(row.ref_column);
    }
    for ((oid, name), (to_table, columns, to_columns, on_delete, on_update)) in keys {
        if let Some(table) = tables.get_mut(&oid) {
            let mut foreign_key = ForeignKey::new(name, columns, to_table, to_columns);
            foreign_key.on_delete = on_delete;
            foreign_key.on_update = on_update;
            table.foreign_keys.push(foreign_key);
        }
    }
    Ok(())
}

async fn attach_check_constraints(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgCheckRow>(
        "SELECT rel.oid::int8 AS table_oid, con.conname AS constraint_name, \
         pg_get_constraintdef(con.oid, true) AS definition \
         FROM pg_constraint con \
         JOIN pg_class rel ON rel.oid = con.conrelid \
         JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
         WHERE con.contype = 'c' AND ns.nspname = ANY($1) \
         AND rel.relname != $2 \
         ORDER BY rel.oid, con.conname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    for row in rows {
        if let Some(table) = tables.get_mut(&row.table_oid) {
            table.constraints.push(Constraint::Check {
                name: row.constraint_name,
                expression: strip_check_wrapper(&row.definition).to_string(),
            });
        }
    }
    Ok(())
}

/// Preserves named PostgreSQL table constraints outside Gaman's modeled subset.
async fn attach_opaque_constraints(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgCheckRow>(
        "SELECT rel.oid::int8 AS table_oid, con.conname AS constraint_name, \
         pg_get_constraintdef(con.oid, true) AS definition \
         FROM pg_constraint con \
         JOIN pg_class rel ON rel.oid = con.conrelid \
         JOIN pg_namespace ns ON ns.oid = rel.relnamespace \
         WHERE con.contype NOT IN ('p', 'u', 'f', 'c') \
         AND con.conrelid != 0 AND ns.nspname = ANY($1) \
         AND rel.relname != $2 \
         ORDER BY rel.oid, con.conname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|error| ExecutorError::Fetch(error.to_string()))?;

    for row in rows {
        if let Some(table) = tables.get_mut(&row.table_oid) {
            table.constraints.push(Constraint::from_trusted_raw(
                row.constraint_name,
                row.definition,
            ));
        }
    }
    Ok(())
}

async fn attach_indexes(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgIndexRow>(
        "SELECT t.oid::int8 AS table_oid, i.relname AS index_name, ix.indisunique AS unique, \
         am.amname AS method, pg_get_indexdef(i.oid) AS raw, \
         pg_get_expr(ix.indpred, ix.indrelid) AS predicate, \
         array_agg(a.attname ORDER BY keys.ordinality) AS columns, \
         array_agg(pg_get_indexdef(i.oid, keys.ordinality::int, true) ORDER BY keys.ordinality) AS element_defs, \
         bool_or(ix.indoption[(keys.ordinality - 1)::int] <> 0) AS has_sort_options, \
         bool_or(ix.indcollation[(keys.ordinality - 1)::int] <> COALESCE(a.attcollation, 0)) AS has_explicit_collation, \
         bool_or(NOT opc.opcdefault) AS has_nondefault_opclass \
         FROM pg_index ix \
         JOIN pg_class t ON t.oid = ix.indrelid \
         JOIN pg_namespace n ON n.oid = t.relnamespace \
         JOIN pg_class i ON i.oid = ix.indexrelid \
         JOIN pg_am am ON am.oid = i.relam \
         JOIN unnest(ix.indkey) WITH ORDINALITY AS keys(attnum, ordinality) ON TRUE \
         LEFT JOIN pg_attribute a ON a.attrelid = t.oid AND a.attnum = keys.attnum \
         LEFT JOIN pg_opclass opc ON opc.oid = ix.indclass[(keys.ordinality - 1)::int] \
         WHERE n.nspname = ANY($1) \
         AND t.relname != $2 \
         AND NOT ix.indisprimary \
         AND NOT EXISTS (SELECT 1 FROM pg_constraint con WHERE con.conindid = ix.indexrelid) \
         GROUP BY t.oid, i.oid, i.relname, ix.indisunique, ix.indpred, ix.indrelid, am.amname \
         ORDER BY t.oid, i.relname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    for row in rows {
        let index = index_from_row(row)?;
        if let Some(table) = tables.get_mut(&index.0) {
            table.indexes.push(index.1);
        }
    }
    Ok(())
}

fn index_from_row(row: PgIndexRow) -> Result<(i64, Index), ExecutorError> {
    let mut columns = Vec::new();
    let mut unsupported = Vec::new();
    for (column, element_def) in row.columns.iter().zip(row.element_defs.iter()) {
        match column {
            Some(column) if element_def == column || element_def == &quote_ident(column) => {
                columns.push(column.clone());
            }
            Some(column) => {
                columns.push(column.clone());
                unsupported.push(format!("column metadata: {element_def}"));
            }
            None => {
                columns.push(element_def.clone());
                unsupported.push(format!("expression: {element_def}"));
            }
        }
    }
    let method = (row.method != "btree").then(|| row.method.clone());
    if let Some(method) = &method {
        unsupported.push(format!("access method: {method}"));
    }
    if row.has_sort_options {
        unsupported.push("sort or null ordering".to_string());
    }
    if row.has_explicit_collation {
        unsupported.push("explicit collation".to_string());
    }
    if row.has_nondefault_opclass {
        unsupported.push("operator class".to_string());
    }
    if !unsupported.is_empty() {
        let mut index = Index::from_trusted_raw(row.index_name, row.raw);
        index.unique = row.unique;
        index.predicate = row.predicate.map(|value| normalize_index_predicate(&value));
        return Ok((row.table_oid, index));
    }
    Ok((
        row.table_oid,
        Index {
            name: row.index_name,
            columns,
            unique: row.unique,
            predicate: row.predicate.map(|value| normalize_index_predicate(&value)),
            opaque: OpaqueMeta::default(),
        },
    ))
}

async fn attach_triggers(
    conn: &mut PgConnection,
    schemas: &[String],
    tables: &mut BTreeMap<i64, Table>,
) -> Result<(), ExecutorError> {
    let rows = sqlx::query_as::<_, PgTriggerRow>(
        "SELECT c.oid::int8 AS table_oid, t.tgname AS trigger_name, t.tgtype, \
         p.proname AS function_name, n2.nspname AS function_schema, l.lanname AS language \
         FROM pg_trigger t \
         JOIN pg_class c ON c.oid = t.tgrelid \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         JOIN pg_proc p ON p.oid = t.tgfoid \
         JOIN pg_namespace n2 ON n2.oid = p.pronamespace \
         JOIN pg_language l ON l.oid = p.prolang \
         WHERE n.nspname = ANY($1) AND NOT t.tgisinternal \
         AND c.relname != $2 \
         ORDER BY c.oid, t.tgname",
    )
    .bind(schemas)
    .bind(TRACKING_TABLE)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    for row in rows {
        if let Some(table) = tables.get_mut(&row.table_oid) {
            let (timing, events, scope) = decode_tgtype(row.tgtype);
            table.triggers.push(TriggerDef {
                name: Some(row.trigger_name),
                timing,
                events,
                scope,
                function_name: Some(schema_qualified_key(
                    &row.function_name,
                    Some(&row.function_schema),
                )),
                when: None,
                query: None,
                language: Some(row.language),
                opaque: OpaqueMeta::default(),
            });
        }
    }
    Ok(())
}

async fn fetch_views(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<String, ViewDef>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgViewRow>(
        "SELECT table_name AS name, table_schema AS schema_name, view_definition AS definition \
         FROM information_schema.views \
         WHERE table_schema = ANY($1) \
         ORDER BY table_schema, table_name",
    )
    .bind(schemas)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let view = ViewDef {
                name: row.name,
                schema: schema_for_output(&row.schema_name),
                definition: row.definition,
                opaque: OpaqueMeta::default(),
            };
            (view.qualified_name(), view)
        })
        .collect())
}

async fn fetch_functions(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<String, FunctionDef>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgFunctionRow>(
        "SELECT p.proname AS name, n.nspname AS schema_name, \
         pg_get_function_identity_arguments(p.oid) AS arguments, p.prosrc AS body, \
         l.lanname AS language, p.provolatile AS volatility, p.prosecdef AS security_definer, \
         pg_get_function_result(p.oid) AS returns \
         FROM pg_proc p \
         JOIN pg_namespace n ON n.oid = p.pronamespace \
         JOIN pg_language l ON l.oid = p.prolang \
         WHERE p.prokind = 'f' \
         AND l.lanname NOT IN ('internal', 'c') \
         AND n.nspname = ANY($1) \
         ORDER BY n.nspname, p.proname",
    )
    .bind(schemas)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let mut function = FunctionDef {
                name: row.name,
                schema: schema_for_output(&row.schema_name),
                arguments: row.arguments,
                parameters: Vec::new(),
                depends_on: Vec::new(),
                returns: row.returns,
                language: row.language,
                body: row.body,
                volatility: volatility_from_pg(row.volatility),
                security_definer: row.security_definer,
                opaque: OpaqueMeta::default(),
            };
            normalize_function_identity(&mut function);
            (function_key(&function), function)
        })
        .collect())
}

async fn fetch_extensions(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<String, ExtensionDef>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgExtensionRow>(
        "SELECT e.extname AS name, n.nspname AS schema_name, e.extversion AS version \
         FROM pg_extension e \
         JOIN pg_namespace n ON n.oid = e.extnamespace \
         WHERE n.nspname = ANY($1) \
         ORDER BY n.nspname, e.extname",
    )
    .bind(schemas)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let extension = ExtensionDef {
                name: row.name,
                schema: schema_for_output(&row.schema_name),
                version: row.version,
                opaque: OpaqueMeta::default(),
            };
            (extension.qualified_name(), extension)
        })
        .collect())
}

/// Reflects sequence identities without reading mutable counter state.
async fn fetch_sequences(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<String, SequenceDef>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgSequenceRow>(
        "SELECT c.relname AS name, n.nspname AS schema_name \
         FROM pg_class c \
         JOIN pg_namespace n ON n.oid = c.relnamespace \
         WHERE c.relkind = 'S' AND n.nspname = ANY($1) \
         ORDER BY n.nspname, c.relname",
    )
    .bind(schemas)
    .fetch_all(conn)
    .await
    .map_err(|error| ExecutorError::Fetch(error.to_string()))?;
    Ok(rows
        .into_iter()
        .map(|row| {
            let sequence = SequenceDef::trusted_identity(
                row.name,
                schema_for_output(&row.schema_name),
            );
            (sequence.qualified_name(), sequence)
        })
        .collect())
}

async fn fetch_enums(
    conn: &mut PgConnection,
    schemas: &[String],
) -> Result<BTreeMap<String, EnumDef>, ExecutorError> {
    let rows = sqlx::query_as::<_, PgEnumRow>(
        "SELECT t.typname AS name, n.nspname AS schema_name, e.enumlabel AS label \
         FROM pg_type t \
         JOIN pg_enum e ON e.enumtypid = t.oid \
         JOIN pg_namespace n ON n.oid = t.typnamespace \
         WHERE n.nspname = ANY($1) \
         ORDER BY n.nspname, t.typname, e.enumsortorder",
    )
    .bind(schemas)
    .fetch_all(conn)
    .await
    .map_err(|e| ExecutorError::Fetch(e.to_string()))?;

    let mut enums = BTreeMap::new();
    for row in rows {
        let schema = schema_for_output(&row.schema_name);
        let key = schema_qualified_key(&row.name, schema.as_deref());
        enums
            .entry(key)
            .or_insert_with(|| EnumDef {
                name: row.name,
                schema,
                values: Vec::new(),
                opaque: OpaqueMeta::default(),
            })
            .values
            .push(row.label);
    }
    Ok(enums)
}

fn schema_for_output(schema: &str) -> Option<String> {
    (schema != "public").then(|| schema.to_string())
}

fn function_key(function: &FunctionDef) -> String {
    function.identity_key()
}

fn normalize_function_identity(function: &mut FunctionDef) {
    function.normalize_legacy_parameters();
    for parameter in &mut function.parameters {
        parameter.type_name = Dialect::Postgres.canonical_type(&parameter.type_name);
    }
}

fn volatility_from_pg(value: i8) -> Volatility {
    match value as u8 {
        b'i' => Volatility::Immutable,
        b's' => Volatility::Stable,
        _ => Volatility::Volatile,
    }
}

fn normalize_index_predicate(predicate: &str) -> String {
    let trimmed = predicate.trim();
    if let Some(inner) = strip_balanced_outer_parens(trimmed) {
        inner.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

fn strip_check_wrapper(value: &str) -> &str {
    let trimmed = value.trim();
    let Some((_, rest)) = strip_keyword(trimmed, "CHECK") else {
        return trimmed;
    };
    let rest = rest.trim_start();
    let Some(end) = matching_paren(rest, 0) else {
        return trimmed;
    };
    if rest[end + 1..].trim().is_empty() {
        &rest[1..end]
    } else {
        trimmed
    }
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

fn strip_balanced_outer_parens(value: &str) -> Option<&str> {
    let inner = value.strip_prefix('(')?.strip_suffix(')')?;
    let end = matching_paren(value, 0)?;
    (end == value.len() - 1).then_some(inner)
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
            '\'' | '"' => quote = Some(ch),
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

fn quote_ident(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}

fn decode_tgtype(tgtype: i16) -> (TriggerTiming, Vec<TriggerEvent>, TriggerScope) {
    let timing = if tgtype & 0x40 != 0 {
        TriggerTiming::InsteadOf
    } else if tgtype & 0x02 != 0 {
        TriggerTiming::Before
    } else {
        TriggerTiming::After
    };
    let mut events = Vec::new();
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies a CHECK wrapper is removed only when the full value is one CHECK expression.
    #[test]
    fn strip_check_wrapper_removes_outer_check() {
        assert_eq!(strip_check_wrapper("CHECK ((value > 0))"), "(value > 0)");
        assert_eq!(strip_check_wrapper("value > 0"), "value > 0");
    }

    /// Verifies index predicates lose only one balanced outer parenthesis pair.
    #[test]
    fn normalize_index_predicate_strips_balanced_outer_parens() {
        assert_eq!(
            normalize_index_predicate("(deleted IS NULL)"),
            "deleted IS NULL"
        );
        assert_eq!(normalize_index_predicate("active"), "active");
    }

    /// Verifies reflected PostgreSQL identity arguments use typed, canonical overload keys.
    #[test]
    fn function_key_normalizes_legacy_catalog_arguments() {
        let mut function = FunctionDef {
            name: "add_one".to_string(),
            schema: None,
            arguments: "value INTEGER".to_string(),
            parameters: Vec::new(),
            depends_on: Vec::new(),
            returns: "integer".to_string(),
            language: "sql".to_string(),
            body: String::new(),
            volatility: Volatility::Volatile,
            security_definer: false,
            opaque: OpaqueMeta::default(),
        };

        normalize_function_identity(&mut function);

        assert_eq!(function_key(&function), "add_one(integer)");
        assert!(function.arguments.is_empty());
    }
}
