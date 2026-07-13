//! Live MySQL and MariaDB execution and catalog inspection.

use std::collections::BTreeMap;

use gaman_core::dialects::Dialect;
use gaman_core::states::{
    Column, Constraint, ForeignKey, FunctionDef, GeneratedStorage, Index, PrimaryKey, Schema,
    Table, TableOptionsMeta, TriggerDef, ViewDef,
};
use sqlx::{MySqlConnection, Row};

use super::{BoxFuture, ExecutorError, InspectionError, SchemaInspector};
use gaman_core::TRACKING_TABLE;

/// SQLx-backed executor shared by MySQL and MariaDB after product validation.
use super::MysqlFamilyExecutor;
impl SchemaInspector for MysqlFamilyExecutor {
    fn inspect<'a>(
        &'a mut self,
        schemas: &'a [&'a str],
    ) -> BoxFuture<'a, Result<Schema, InspectionError>> {
        Box::pin(async move {
            if schemas.len() > 1
                || schemas
                    .first()
                    .is_some_and(|schema| *schema != self.database)
            {
                return Err(InspectionError::query(format!(
                    "{} inspection is limited to selected database '{}'",
                    self.dialect.as_str(),
                    self.database
                )));
            }
            inspect(&mut self.conn, &self.database, self.dialect)
                .await
                .map_err(InspectionError::from)
        })
    }
}

#[derive(sqlx::FromRow)]
struct TableRow {
    table_name: String,
    engine: Option<String>,
    table_collation: Option<String>,
    row_format: Option<String>,
    table_comment: String,
}
#[derive(sqlx::FromRow)]
struct ColumnRow {
    table_name: String,
    column_name: String,
    column_type: String,
    data_type: String,
    is_nullable: String,
    column_default: Option<String>,
    extra: String,
    generation_expression: String,
    character_set_name: Option<String>,
    collation_name: Option<String>,
    column_comment: String,
}
#[derive(sqlx::FromRow)]
struct KeyRow {
    table_name: String,
    constraint_name: String,
    constraint_type: String,
    column_name: String,
    ordinal_position: i64,
}
#[derive(sqlx::FromRow)]
struct ForeignKeyRow {
    table_name: String,
    constraint_name: String,
    column_name: String,
    referenced_table_name: String,
    referenced_column_name: String,
    ordinal_position: i64,
    delete_rule: String,
    update_rule: String,
}
#[derive(sqlx::FromRow)]
struct IndexRow {
    table_name: String,
    index_name: String,
    non_unique: i64,
    seq_in_index: i64,
    column_name: Option<String>,
    sub_part: Option<i64>,
    collation: Option<String>,
    index_type: String,
    expression: Option<String>,
    enabled: String,
    index_comment: String,
}
#[derive(sqlx::FromRow)]
struct CheckRow {
    table_name: String,
    constraint_name: String,
    check_clause: String,
}
#[derive(sqlx::FromRow)]
struct ViewRow {
    table_name: String,
    #[sqlx(default)]
    create_sql: Option<String>,
}
#[derive(sqlx::FromRow)]
struct TriggerRow {
    trigger_name: String,
    event_object_table: String,
    #[sqlx(default)]
    create_sql: Option<String>,
}
#[derive(sqlx::FromRow)]
struct FunctionRow {
    routine_name: String,
    #[sqlx(default)]
    create_sql: Option<String>,
}

/// Batched catalog rows assembled into one inspected schema.
struct CatalogSnapshot {
    tables: Vec<TableRow>,
    columns: Vec<ColumnRow>,
    keys: Vec<KeyRow>,
    foreign_keys: Vec<ForeignKeyRow>,
    indexes: Vec<IndexRow>,
    checks: Vec<CheckRow>,
    views: Vec<ViewRow>,
    triggers: Vec<TriggerRow>,
    functions: Vec<FunctionRow>,
}

async fn inspect(
    conn: &mut MySqlConnection,
    database: &str,
    dialect: Dialect,
) -> Result<Schema, ExecutorError> {
    let tables: Vec<TableRow> = sqlx::query_as("SELECT TABLE_NAME table_name, CAST(ENGINE AS CHAR) engine, CAST(TABLE_COLLATION AS CHAR) table_collation, CAST(ROW_FORMAT AS CHAR) row_format, CAST(TABLE_COMMENT AS CHAR) table_comment FROM INFORMATION_SCHEMA.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_TYPE = 'BASE TABLE' AND TABLE_NAME <> ? ORDER BY TABLE_NAME").bind(database).bind(TRACKING_TABLE).fetch_all(&mut *conn).await.map_err(|error| fetch("tables", error))?;
    let columns: Vec<ColumnRow> = sqlx::query_as("SELECT c.TABLE_NAME table_name, c.COLUMN_NAME column_name, CAST(c.COLUMN_TYPE AS CHAR) column_type, CAST(c.DATA_TYPE AS CHAR) data_type, CAST(c.IS_NULLABLE AS CHAR) is_nullable, CAST(c.COLUMN_DEFAULT AS CHAR) column_default, CAST(c.EXTRA AS CHAR) extra, CAST(c.GENERATION_EXPRESSION AS CHAR) generation_expression, CAST(c.CHARACTER_SET_NAME AS CHAR) character_set_name, CAST(c.COLLATION_NAME AS CHAR) collation_name, CAST(c.COLUMN_COMMENT AS CHAR) column_comment FROM INFORMATION_SCHEMA.COLUMNS c JOIN INFORMATION_SCHEMA.TABLES t ON t.TABLE_SCHEMA=c.TABLE_SCHEMA AND t.TABLE_NAME=c.TABLE_NAME WHERE c.TABLE_SCHEMA = ? AND t.TABLE_TYPE='BASE TABLE' AND c.TABLE_NAME <> ? ORDER BY c.TABLE_NAME, c.ORDINAL_POSITION").bind(database).bind(TRACKING_TABLE).fetch_all(&mut *conn).await.map_err(|error| fetch("columns", error))?;
    let keys: Vec<KeyRow> = sqlx::query_as("SELECT tc.TABLE_NAME table_name, tc.CONSTRAINT_NAME constraint_name, CAST(tc.CONSTRAINT_TYPE AS CHAR) constraint_type, kcu.COLUMN_NAME column_name, CAST(kcu.ORDINAL_POSITION AS SIGNED) ordinal_position FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc JOIN INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu ON kcu.CONSTRAINT_SCHEMA=tc.CONSTRAINT_SCHEMA AND kcu.TABLE_NAME=tc.TABLE_NAME AND kcu.CONSTRAINT_NAME=tc.CONSTRAINT_NAME WHERE tc.CONSTRAINT_SCHEMA=? AND tc.CONSTRAINT_TYPE IN ('PRIMARY KEY','UNIQUE') ORDER BY tc.TABLE_NAME, tc.CONSTRAINT_NAME, kcu.ORDINAL_POSITION").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("keys", error))?;
    let foreign_keys: Vec<ForeignKeyRow> = sqlx::query_as("SELECT kcu.TABLE_NAME table_name, kcu.CONSTRAINT_NAME constraint_name, kcu.COLUMN_NAME column_name, kcu.REFERENCED_TABLE_NAME referenced_table_name, kcu.REFERENCED_COLUMN_NAME referenced_column_name, CAST(kcu.ORDINAL_POSITION AS SIGNED) ordinal_position, CAST(rc.DELETE_RULE AS CHAR) delete_rule, CAST(rc.UPDATE_RULE AS CHAR) update_rule FROM INFORMATION_SCHEMA.KEY_COLUMN_USAGE kcu JOIN INFORMATION_SCHEMA.REFERENTIAL_CONSTRAINTS rc ON rc.CONSTRAINT_SCHEMA=kcu.CONSTRAINT_SCHEMA AND rc.CONSTRAINT_NAME=kcu.CONSTRAINT_NAME WHERE kcu.CONSTRAINT_SCHEMA=? AND kcu.REFERENCED_TABLE_NAME IS NOT NULL ORDER BY kcu.TABLE_NAME, kcu.CONSTRAINT_NAME, kcu.ORDINAL_POSITION").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("foreign keys", error))?;
    let index_sql = match dialect {
        Dialect::Mysql => {
            "SELECT TABLE_NAME table_name, INDEX_NAME index_name, CAST(NON_UNIQUE AS SIGNED) non_unique, CAST(SEQ_IN_INDEX AS SIGNED) seq_in_index, COLUMN_NAME column_name, CAST(SUB_PART AS SIGNED) sub_part, CAST(COLLATION AS CHAR) collation, CAST(INDEX_TYPE AS CHAR) index_type, CAST(EXPRESSION AS CHAR) expression, CAST(IS_VISIBLE AS CHAR) enabled, CAST(INDEX_COMMENT AS CHAR) index_comment FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA=? AND INDEX_NAME <> 'PRIMARY' ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX"
        }
        Dialect::Mariadb => {
            "SELECT TABLE_NAME table_name, INDEX_NAME index_name, CAST(NON_UNIQUE AS SIGNED) non_unique, CAST(SEQ_IN_INDEX AS SIGNED) seq_in_index, COLUMN_NAME column_name, CAST(SUB_PART AS SIGNED) sub_part, CAST(COLLATION AS CHAR) collation, CAST(INDEX_TYPE AS CHAR) index_type, CAST(NULL AS CHAR) expression, CAST(IF(IGNORED='YES','NO','YES') AS CHAR) enabled, CAST(INDEX_COMMENT AS CHAR) index_comment FROM INFORMATION_SCHEMA.STATISTICS WHERE TABLE_SCHEMA=? AND INDEX_NAME <> 'PRIMARY' ORDER BY TABLE_NAME, INDEX_NAME, SEQ_IN_INDEX"
        }
        _ => {
            return Err(ExecutorError::Fetch(
                "MySQL-family inspection received another dialect".to_string(),
            ));
        }
    };
    let indexes: Vec<IndexRow> = sqlx::query_as(index_sql)
        .bind(database)
        .fetch_all(&mut *conn)
        .await
        .map_err(|error| fetch("indexes", error))?;
    let checks: Vec<CheckRow> = sqlx::query_as("SELECT tc.TABLE_NAME table_name, tc.CONSTRAINT_NAME constraint_name, CAST(cc.CHECK_CLAUSE AS CHAR) check_clause FROM INFORMATION_SCHEMA.TABLE_CONSTRAINTS tc JOIN INFORMATION_SCHEMA.CHECK_CONSTRAINTS cc ON cc.CONSTRAINT_SCHEMA=tc.CONSTRAINT_SCHEMA AND cc.CONSTRAINT_NAME=tc.CONSTRAINT_NAME WHERE tc.CONSTRAINT_SCHEMA=? AND tc.CONSTRAINT_TYPE='CHECK' ORDER BY tc.TABLE_NAME, tc.CONSTRAINT_NAME").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("checks", error))?;
    let mut views: Vec<ViewRow> = sqlx::query_as("SELECT TABLE_NAME table_name FROM INFORMATION_SCHEMA.VIEWS WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("views", error))?;
    let mut triggers: Vec<TriggerRow> = sqlx::query_as("SELECT TRIGGER_NAME trigger_name, EVENT_OBJECT_TABLE event_object_table FROM INFORMATION_SCHEMA.TRIGGERS WHERE TRIGGER_SCHEMA=? ORDER BY TRIGGER_NAME").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("triggers", error))?;
    let mut functions: Vec<FunctionRow> = sqlx::query_as("SELECT ROUTINE_NAME routine_name FROM INFORMATION_SCHEMA.ROUTINES WHERE ROUTINE_SCHEMA=? AND ROUTINE_TYPE='FUNCTION' ORDER BY ROUTINE_NAME").bind(database).fetch_all(&mut *conn).await.map_err(|error| fetch("functions", error))?;
    for view in &mut views {
        view.create_sql = show_create(
            &mut *conn,
            database,
            "VIEW",
            &view.table_name,
            &["Create View"],
        )
        .await?;
    }
    for trigger in &mut triggers {
        trigger.create_sql = show_create(
            &mut *conn,
            database,
            "TRIGGER",
            &trigger.trigger_name,
            &["SQL Original Statement", "Create Trigger"],
        )
        .await?;
    }
    for function in &mut functions {
        function.create_sql = show_create(
            &mut *conn,
            database,
            "FUNCTION",
            &function.routine_name,
            &["Create Function"],
        )
        .await?;
    }
    assemble(
        database,
        dialect,
        CatalogSnapshot {
            tables,
            columns,
            keys,
            foreign_keys,
            indexes,
            checks,
            views,
            triggers,
            functions,
        },
    )
}

fn fetch(stage: &str, error: sqlx::Error) -> ExecutorError {
    ExecutorError::Fetch(format!("{stage} catalog query failed: {error}"))
}

/// Retrieves canonical opaque source without inventing missing catalog details.
async fn show_create(
    conn: &mut MySqlConnection,
    database: &str,
    kind: &str,
    name: &str,
    columns: &[&str],
) -> Result<Option<String>, ExecutorError> {
    let quote = |value: &str| format!("`{}`", value.replace('`', "``"));
    let sql = format!("SHOW CREATE {kind} {}.{}", quote(database), quote(name));
    let row = sqlx::query(&sql)
        .fetch_one(conn)
        .await
        .map_err(|error| fetch("show create", error))?;
    Ok(columns
        .iter()
        .find_map(|column| row.try_get::<String, _>(*column).ok()))
}

fn assemble(
    database: &str,
    dialect: Dialect,
    snapshot: CatalogSnapshot,
) -> Result<Schema, ExecutorError> {
    let CatalogSnapshot {
        tables,
        columns,
        keys,
        foreign_keys,
        indexes,
        checks,
        views,
        triggers,
        functions,
    } = snapshot;
    let mut schema = Schema::default();
    for row in tables {
        let mut options = Vec::new();
        if let Some(engine) = row.engine {
            options.push(format!("ENGINE={engine}"));
        }
        if let Some(collation) = row.table_collation {
            options.push(format!("COLLATE={collation}"));
        }
        if let Some(format) = row.row_format {
            options.push(format!("ROW_FORMAT={format}"));
        }
        if !row.table_comment.is_empty() {
            options.push(format!(
                "COMMENT='{}'",
                row.table_comment.replace('\'', "''")
            ));
        }
        let mut table = Table {
            name: row.table_name.clone(),
            schema: None,
            primary_key: None,
            columns: Vec::new(),
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
            triggers: Vec::new(),
            options: TableOptionsMeta::from_parts(Vec::new(), options),
        };
        table.mark_options_trusted();
        schema.tables.insert(row.table_name, table);
    }
    for row in columns {
        let table = schema.tables.get_mut(&row.table_name).ok_or_else(|| {
            ExecutorError::Fetch(format!(
                "column references unknown table '{}.{}'",
                database, row.table_name
            ))
        })?;
        let extra = row.extra.to_ascii_lowercase();
        let on_update_expression = extra
            .split_once("on update ")
            .map(|(_, value)| value.to_string());
        let dialect_options = match dialect {
            Dialect::Mysql => gaman_core::states::ColumnDialectOptions {
                mysql: Some(gaman_core::states::MysqlColumnOptions {
                    auto_increment: extra.contains("auto_increment"),
                    on_update_expression,
                    character_set: row.character_set_name,
                    collation: row.collation_name,
                    invisible: extra.contains("invisible"),
                    comment: (!row.column_comment.is_empty()).then_some(row.column_comment),
                }),
                mariadb: None,
            },
            Dialect::Mariadb => gaman_core::states::ColumnDialectOptions {
                mysql: None,
                mariadb: Some(gaman_core::states::MariadbColumnOptions {
                    auto_increment: extra.contains("auto_increment"),
                    on_update_expression,
                    character_set: row.character_set_name,
                    collation: row.collation_name,
                    invisible: extra.contains("invisible"),
                    comment: (!row.column_comment.is_empty()).then_some(row.column_comment),
                }),
            },
            _ => gaman_core::states::ColumnDialectOptions::default(),
        };
        table.columns.push(Column {
            name: row.column_name,
            col_type: row.column_type,
            nullable: row.is_nullable == "YES",
            default: decoded_default(&row.data_type, row.column_default, &extra),
            primary_key: false,
            references: None,
            check: None,
            generated: (!row.generation_expression.is_empty()).then_some(row.generation_expression),
            generated_storage: if extra.contains("stored generated") {
                Some(GeneratedStorage::Stored)
            } else if extra.contains("virtual generated") {
                Some(GeneratedStorage::Virtual)
            } else {
                None
            },
            dialect_options,
        });
    }
    apply_keys(&mut schema, keys);
    apply_foreign_keys(&mut schema, foreign_keys);
    apply_indexes(&mut schema, indexes);
    for row in checks {
        if let Some(table) = schema.tables.get_mut(&row.table_name) {
            if dialect == Dialect::Mariadb && canonicalize_mariadb_json_alias(table, &row) {
                continue;
            }
            table.constraints.push(Constraint::Check {
                name: row.constraint_name,
                expression: row.check_clause,
            });
        }
    }
    for row in views {
        let view = row.create_sql.map_or_else(
            || ViewDef::from_trusted_identity(row.table_name.clone()),
            |raw| ViewDef::from_trusted_raw(row.table_name.clone(), raw),
        );
        schema.views.insert(row.table_name, view);
    }
    for row in triggers {
        if let Some(table) = schema.tables.get_mut(&row.event_object_table) {
            let trigger = row.create_sql.map_or_else(
                || TriggerDef::from_trusted_identity(row.trigger_name.clone()),
                |raw| TriggerDef::from_trusted_raw(row.trigger_name.clone(), raw),
            );
            table.triggers.push(trigger);
        }
    }
    for row in functions {
        let function = row.create_sql.map_or_else(
            || FunctionDef::from_trusted_identity(row.routine_name.clone()),
            |raw| FunctionDef::from_trusted_raw(row.routine_name.clone(), raw),
        );
        schema.functions.insert(row.routine_name, function);
    }
    Ok(schema)
}

/// Recognizes only MariaDB's complete JSON alias shape and consumes its synthetic check.
fn canonicalize_mariadb_json_alias(table: &mut Table, check: &CheckRow) -> bool {
    let expression = check
        .check_clause
        .to_ascii_lowercase()
        .replace([' ', '`'], "");
    let Some(column) = table.columns.iter_mut().find(|column| {
        check.constraint_name == column.name
            && expression == format!("json_valid({})", column.name.to_ascii_lowercase())
            && column.col_type.to_ascii_lowercase().starts_with("longtext")
    }) else {
        return false;
    };
    let compatible = column.dialect_options.mariadb().is_some_and(|options| {
        options.character_set.as_deref() == Some("utf8mb4")
            && options
                .collation
                .as_deref()
                .is_some_and(|value| value.starts_with("utf8mb4_"))
    });
    if compatible {
        column.col_type = "json".to_string();
    }
    compatible
}

/// Converts catalog defaults back into executable SQL without confusing literals and expressions.
fn decoded_default(data_type: &str, value: Option<String>, extra: &str) -> Option<String> {
    let value = value?;
    if extra.contains("default_generated")
        || is_default_keyword(&value)
        || is_numeric_type(data_type)
    {
        return Some(value);
    }
    if matches!(
        data_type.to_ascii_lowercase().as_str(),
        "bit" | "binary" | "varbinary"
    ) && (value.starts_with("b'") || value.starts_with("x'"))
    {
        return Some(value);
    }
    Some(format!("'{}'", value.replace('\'', "''")))
}

fn is_default_keyword(value: &str) -> bool {
    let value = value.to_ascii_uppercase();
    value == "CURRENT_TIMESTAMP" || value.starts_with("CURRENT_TIMESTAMP(") || value == "NULL"
}

fn is_numeric_type(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "tinyint"
            | "smallint"
            | "mediumint"
            | "int"
            | "integer"
            | "bigint"
            | "decimal"
            | "numeric"
            | "float"
            | "double"
            | "real"
            | "year"
    )
}

fn apply_keys(schema: &mut Schema, rows: Vec<KeyRow>) {
    let mut groups: BTreeMap<(String, String, String), Vec<(i64, String)>> = BTreeMap::new();
    for row in rows {
        groups
            .entry((row.table_name, row.constraint_name, row.constraint_type))
            .or_default()
            .push((row.ordinal_position, row.column_name));
    }
    for ((table_name, name, kind), mut columns) in groups {
        columns.sort_by_key(|item| item.0);
        let columns = columns.into_iter().map(|item| item.1).collect::<Vec<_>>();
        if let Some(table) = schema.tables.get_mut(&table_name) {
            if kind == "PRIMARY KEY" {
                for column in &mut table.columns {
                    if columns.contains(&column.name) {
                        column.primary_key = true;
                        column.nullable = false;
                    }
                }
                table.primary_key = Some(PrimaryKey { name, columns });
            } else {
                table.constraints.push(Constraint::Unique { name, columns });
            }
        }
    }
}
fn apply_foreign_keys(schema: &mut Schema, rows: Vec<ForeignKeyRow>) {
    type ForeignKeyIdentity = (String, String, String, String, String);
    type ForeignKeyColumns = Vec<(i64, String, String)>;
    let mut groups: BTreeMap<ForeignKeyIdentity, ForeignKeyColumns> = BTreeMap::new();
    for row in rows {
        groups
            .entry((
                row.table_name,
                row.constraint_name,
                row.referenced_table_name,
                row.delete_rule,
                row.update_rule,
            ))
            .or_default()
            .push((
                row.ordinal_position,
                row.column_name,
                row.referenced_column_name,
            ));
    }
    for ((table_name, name, target, delete, update), mut columns) in groups {
        columns.sort_by_key(|item| item.0);
        if let Some(table) = schema.tables.get_mut(&table_name) {
            table.foreign_keys.push(ForeignKey {
                name,
                columns: columns.iter().map(|item| item.1.clone()).collect(),
                to_table: target,
                to_columns: columns.into_iter().map(|item| item.2).collect(),
                on_delete: action(&delete),
                on_update: action(&update),
            });
        }
    }
}
fn apply_indexes(schema: &mut Schema, rows: Vec<IndexRow>) {
    let mut groups: BTreeMap<(String, String, bool), Vec<IndexRow>> = BTreeMap::new();
    for row in rows {
        let unique = row.non_unique == 0;
        groups
            .entry((row.table_name.clone(), row.index_name.clone(), unique))
            .or_default()
            .push(row);
    }
    for ((table_name, name, unique), mut rows) in groups {
        rows.sort_by_key(|row| row.seq_in_index);
        if let Some(table) = schema.tables.get_mut(&table_name) {
            let advanced = rows.iter().any(|row| {
                row.column_name.is_none()
                    || row.expression.is_some()
                    || row.sub_part.is_some()
                    || row.collation.as_deref() == Some("D")
                    || !row.index_type.eq_ignore_ascii_case("BTREE")
                    || row.enabled != "YES"
                    || !row.index_comment.is_empty()
            });
            let index = if advanced {
                Index::from_trusted_identity(name)
            } else {
                Index {
                    name,
                    columns: rows.into_iter().filter_map(|row| row.column_name).collect(),
                    unique,
                    predicate: None,
                    opaque: Default::default(),
                }
            };
            table.indexes.push(index);
        }
    }
}
fn action(value: &str) -> Option<String> {
    match value.to_ascii_uppercase().as_str() {
        "CASCADE" => Some("cascade".to_string()),
        "RESTRICT" => Some("restrict".to_string()),
        "SET NULL" => Some("set_null".to_string()),
        "SET DEFAULT" => Some("set_default".to_string()),
        _ => None,
    }
}
