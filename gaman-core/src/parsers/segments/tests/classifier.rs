use super::super::{
    DdlStatementKind, DmlStatementKind, SqlObjectName, SqlStatementKind, segment_sql,
};
use crate::dialects::Dialect;
use crate::states::types::EntityKind;

fn kinds(sql: &str, dialect: Dialect) -> Vec<Option<SqlStatementKind>> {
    segment_sql(sql, dialect)
        .unwrap()
        .into_iter()
        .map(|segment| segment.kind)
        .collect()
}

fn ddl(entity: EntityKind, name: &str) -> Option<SqlStatementKind> {
    Some(SqlStatementKind::Ddl(DdlStatementKind {
        entity,
        name: Some(name_obj(name)),
    }))
}

fn dml(kind: DmlStatementKind) -> Option<SqlStatementKind> {
    Some(SqlStatementKind::Dml(kind))
}

fn with(kind: DmlStatementKind) -> Option<SqlStatementKind> {
    dml(DmlStatementKind::With(Box::new(kind)))
}

fn name_obj(raw: &str) -> SqlObjectName {
    name_obj_parts(raw, raw.split('.').collect())
}

fn name_obj_parts(raw: &str, parts: Vec<&str>) -> SqlObjectName {
    SqlObjectName {
        raw: raw.to_string(),
        parts: parts.into_iter().map(ToString::to_string).collect(),
    }
}

/// Verifies modeled CREATE targets classify as DDL entity kinds.
#[test]
fn ddl_classification_for_modeled_create_targets() {
    let kinds = kinds(
        "CREATE TABLE users (id int);
         CREATE UNIQUE INDEX users_id_idx ON users(id);
         CREATE VIEW user_ids AS SELECT id FROM users;
         CREATE TRIGGER users_ai AFTER INSERT ON users EXECUTE FUNCTION audit_users();",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Table, "users"),
            ddl(EntityKind::Index, "users_id_idx"),
            ddl(EntityKind::View, "user_ids"),
            ddl(EntityKind::Trigger, "users_ai"),
        ]
    );
}

/// Verifies CREATE prefixes stop at the first valid determinant instead of scanning later words.
#[test]
fn create_determinant_does_not_scan_later_words() {
    let kinds = kinds(
        "CREATE EVENT TRIGGER users_ai ON ddl_command_start EXECUTE FUNCTION audit();
         CREATE POLICY users_select ON users FOR SELECT USING (true);",
        Dialect::Postgres,
    );
    assert_eq!(kinds, vec![None, None]);
}

/// Verifies unknown CREATE prefix modifiers are skipped until a modeled determinant appears.
#[test]
fn create_determinant_skips_unknown_prefix_modifiers() {
    let kinds = kinds(
        "CREATE FOO BAR TABLE users (id int);
         CREATE ALGORITHM MERGE DEFINER root SQL SECURITY DEFINER VIEW active_users AS SELECT id FROM users;
         CREATE CUSTOM UNIQUE INDEX users_email_idx ON users(email);",
        Dialect::Mysql,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Table, "users"),
            ddl(EntityKind::View, "active_users"),
            ddl(EntityKind::Index, "users_email_idx"),
        ]
    );
}

/// Verifies PostgreSQL-only CREATE targets classify as DDL entity kinds.
#[test]
fn postgres_specific_ddl_classification() {
    let kinds = kinds(
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql AS $$ SELECT 1; $$;
         CREATE EXTENSION pgcrypto;
         CREATE TYPE mood AS ENUM ('happy', 'sad');",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Function, "f"),
            ddl(EntityKind::Extension, "pgcrypto"),
            ddl(EntityKind::Enum, "mood"),
        ]
    );
}

/// Verifies CREATE modifiers do not hide the primary DDL target.
#[test]
fn ddl_classification_with_create_modifiers() {
    let kinds = kinds(
        "CREATE OR REPLACE FUNCTION f() RETURNS int LANGUAGE sql AS $$ SELECT 1; $$;
         CREATE TEMP TABLE temp_users (id int);
         CREATE UNLOGGED TABLE events (id int);
         CREATE TABLE IF NOT EXISTS users (id int);
         CREATE MATERIALIZED VIEW active_users AS SELECT id FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Function, "f"),
            ddl(EntityKind::Table, "temp_users"),
            ddl(EntityKind::Table, "events"),
            ddl(EntityKind::Table, "users"),
            ddl(EntityKind::View, "active_users"),
        ]
    );
}

/// Verifies view and index modifier forms still expose the primary object name.
#[test]
fn ddl_classification_with_view_and_index_modifiers() {
    let kinds = kinds(
        "CREATE OR REPLACE VIEW active_users AS SELECT id FROM users;
         CREATE INDEX CONCURRENTLY users_email_idx ON users(email);
         CREATE INDEX IF NOT EXISTS users_name_idx ON users(name);
         CREATE UNIQUE INDEX users_email_unique_idx ON users(email);",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::View, "active_users"),
            ddl(EntityKind::Index, "users_email_idx"),
            ddl(EntityKind::Index, "users_name_idx"),
            ddl(EntityKind::Index, "users_email_unique_idx"),
        ]
    );
}

/// Verifies enum classification requires CREATE TYPE name AS ENUM shape.
#[test]
fn enum_classification_requires_type_name_as_enum_shape() {
    let kinds = kinds(
        "CREATE TYPE app.mood AS ENUM ('happy', 'sad');
         CREATE TYPE app.point AS (x int, y int);
         CREATE TYPE app.alias AS RANGE (subtype = int4);",
        Dialect::Postgres,
    );
    assert_eq!(kinds, vec![ddl(EntityKind::Enum, "app.mood"), None, None]);
}

/// Verifies quoted object names preserve raw spelling but expose unquoted parts.
#[test]
fn quoted_identifier_names_preserve_raw_and_unquote_parts() {
    let segments = segment_sql(
        "CREATE TABLE \"public\".\"User\" (id int); CREATE TABLE `app`.`order` (id int);",
        Dialect::Mysql,
    )
    .unwrap();
    assert_eq!(
        segments[0].kind,
        Some(SqlStatementKind::Ddl(DdlStatementKind {
            entity: EntityKind::Table,
            name: Some(name_obj_parts(
                "\"public\".\"User\"",
                vec!["public", "User"]
            )),
        }))
    );
    assert_eq!(
        segments[1].kind,
        Some(SqlStatementKind::Ddl(DdlStatementKind {
            entity: EntityKind::Table,
            name: Some(name_obj_parts("`app`.`order`", vec!["app", "order"])),
        }))
    );
}

/// Verifies unsupported CREATE determinants are intentionally unclassified.
#[test]
fn unsupported_create_classification_is_none() {
    let postgres_kinds = kinds(
        "CREATE TYPE point AS (x int, y int);
         CREATE POLICY p ON users USING (true);
         CREATE PROCEDURE p() LANGUAGE sql AS $$ SELECT 1; $$;",
        Dialect::Postgres,
    );
    assert_eq!(postgres_kinds, vec![None, None, None]);

    let mysql_kinds = kinds(
        "DELIMITER //\nCREATE FUNCTION f() RETURNS INT BEGIN RETURN 1; END//",
        Dialect::Mysql,
    );
    assert_eq!(mysql_kinds, vec![ddl(EntityKind::Function, "f")]);
}

/// Verifies basic DML statements classify without broadening the taxonomy.
#[test]
fn dml_classification_for_basic_statements() {
    let kinds = kinds(
        "SELECT * FROM users;
         INSERT INTO users(id) VALUES (1);
         UPDATE users SET id = id;
         DELETE FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            dml(DmlStatementKind::Select),
            dml(DmlStatementKind::Insert),
            dml(DmlStatementKind::Update),
            dml(DmlStatementKind::Delete),
        ]
    );
}

/// Verifies CTE-backed DML classifies by the final top-level DML verb.
#[test]
fn with_dml_classification() {
    let kinds = kinds(
        "WITH u AS (SELECT * FROM users) SELECT * FROM u;
         WITH u AS (SELECT * FROM users) INSERT INTO audit SELECT * FROM u;
         WITH u AS (SELECT * FROM users) UPDATE users SET id = id;
         WITH u AS (SELECT * FROM users) DELETE FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            with(DmlStatementKind::Select),
            with(DmlStatementKind::Insert),
            with(DmlStatementKind::Update),
            with(DmlStatementKind::Delete),
        ]
    );
}

/// Verifies multi-entry and recursive CTEs classify as WITH-backed DML.
#[test]
fn with_dml_classification_for_multiple_and_recursive_ctes() {
    let kinds = kinds(
        "WITH a AS (SELECT 1), b AS (SELECT * FROM a) SELECT * FROM b;
         WITH RECURSIVE n AS (SELECT 1 UNION ALL SELECT 2) SELECT * FROM n;
         WITH a AS MATERIALIZED (SELECT 1) SELECT * FROM a;
         WITH a AS NOT MATERIALIZED (SELECT 1) DELETE FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            with(DmlStatementKind::Select),
            with(DmlStatementKind::Select),
            with(DmlStatementKind::Select),
            with(DmlStatementKind::Delete),
        ]
    );
}

/// Verifies mixed nested delimiters inside CTE groups do not disturb classification.
#[test]
fn classification_handles_mixed_nested_group_delimiters() {
    let kinds = kinds(
        "WITH a AS (SELECT json_build_array((items[1])) FROM users) SELECT * FROM a;",
        Dialect::Postgres,
    );
    assert_eq!(kinds, vec![with(DmlStatementKind::Select)]);
}

/// Verifies every modeled CREATE determinant classifies with its primary object name.
#[test]
fn create_classification_covers_all_modeled_entities() {
    let kinds = kinds(
        "CREATE TABLE users (id int);
         CREATE INDEX users_email_idx ON users(email);
         CREATE VIEW active_users AS SELECT id FROM users;
         CREATE FUNCTION audit_users() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END; $$;
         CREATE TRIGGER users_ai AFTER INSERT ON users EXECUTE FUNCTION audit_users();
         CREATE EXTENSION pgcrypto;
         CREATE TYPE mood AS ENUM ('happy', 'sad');",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Table, "users"),
            ddl(EntityKind::Index, "users_email_idx"),
            ddl(EntityKind::View, "active_users"),
            ddl(EntityKind::Function, "audit_users"),
            ddl(EntityKind::Trigger, "users_ai"),
            ddl(EntityKind::Extension, "pgcrypto"),
            ddl(EntityKind::Enum, "mood"),
        ]
    );
}

/// Verifies CREATE table modifier variants classify as table DDL.
#[test]
fn create_table_modifier_variants_classify() {
    let kinds = kinds(
        "CREATE TABLE users (id int);
         CREATE TABLE IF NOT EXISTS public.users (id int);
         CREATE TEMP TABLE temp_users (id int);
         CREATE TEMPORARY TABLE temp_orders (id int);
         CREATE GLOBAL TEMPORARY TABLE global_temp_users (id int);
         CREATE LOCAL TEMP TABLE local_temp_users (id int);
         CREATE UNLOGGED TABLE event_log (id int);",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Table, "users"),
            ddl(EntityKind::Table, "public.users"),
            ddl(EntityKind::Table, "temp_users"),
            ddl(EntityKind::Table, "temp_orders"),
            ddl(EntityKind::Table, "global_temp_users"),
            ddl(EntityKind::Table, "local_temp_users"),
            ddl(EntityKind::Table, "event_log"),
        ]
    );
}

/// Verifies CREATE index modifier variants classify as index DDL.
#[test]
fn create_index_modifier_variants_classify() {
    let kinds = kinds(
        "CREATE INDEX users_email_idx ON users(email);
         CREATE UNIQUE INDEX users_email_unique_idx ON users(email);
         CREATE INDEX CONCURRENTLY users_name_idx ON users(name);
         CREATE INDEX IF NOT EXISTS users_age_idx ON users(age);
         CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS users_phone_idx ON users(phone);",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Index, "users_email_idx"),
            ddl(EntityKind::Index, "users_email_unique_idx"),
            ddl(EntityKind::Index, "users_name_idx"),
            ddl(EntityKind::Index, "users_age_idx"),
            ddl(EntityKind::Index, "users_phone_idx"),
        ]
    );
}

/// Verifies CREATE view/function/trigger/extension modifier variants classify.
#[test]
fn create_non_table_modifier_variants_classify() {
    let kinds = kinds(
        "CREATE VIEW active_users AS SELECT id FROM users;
         CREATE OR REPLACE VIEW public.active_users AS SELECT id FROM users;
         CREATE MATERIALIZED VIEW user_counts AS SELECT count(*) FROM users;
         CREATE FUNCTION audit_users() RETURNS trigger LANGUAGE sql AS $$ SELECT 1; $$;
         CREATE OR REPLACE FUNCTION public.audit_users() RETURNS trigger LANGUAGE sql AS $$ SELECT 1; $$;
         CREATE TRIGGER users_ai AFTER INSERT ON users EXECUTE FUNCTION audit_users();
         CREATE EXTENSION IF NOT EXISTS pgcrypto;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::View, "active_users"),
            ddl(EntityKind::View, "public.active_users"),
            ddl(EntityKind::View, "user_counts"),
            ddl(EntityKind::Function, "audit_users"),
            ddl(EntityKind::Function, "public.audit_users"),
            ddl(EntityKind::Trigger, "users_ai"),
            ddl(EntityKind::Extension, "pgcrypto"),
        ]
    );
}

/// Verifies CREATE enum variants classify only for the exact AS ENUM shape.
#[test]
fn create_enum_variants_classify_only_as_enum() {
    let kinds = kinds(
        "CREATE TYPE mood AS ENUM ('happy', 'sad');
         CREATE TYPE public.mood AS ENUM ('happy', 'sad');
         CREATE TYPE \"public\".\"Mood\" AS ENUM ('happy', 'sad');
         CREATE TYPE point AS (x int, y int);
         CREATE TYPE money_range AS RANGE (subtype = numeric);",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Enum, "mood"),
            ddl(EntityKind::Enum, "public.mood"),
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Enum,
                name: Some(name_obj_parts(
                    "\"public\".\"Mood\"",
                    vec!["public", "Mood"]
                )),
            })),
            None,
            None,
        ]
    );
}

/// Verifies CREATE classification is broad-stroke and dialect agnostic for MySQL-style SQL.
#[test]
fn create_classification_is_dialect_agnostic_for_mysql_style_sql() {
    let kinds = kinds(
        "CREATE TABLE `users` (id int);
         CREATE INDEX `users_email_idx` ON `users`(`email`);
         DELIMITER //
         CREATE FUNCTION `audit_users`() RETURNS INT BEGIN RETURN 1; END//
         CREATE TRIGGER `users_ai` AFTER INSERT ON `users` FOR EACH ROW BEGIN SET @x = 1; END//",
        Dialect::Mysql,
    );
    assert_eq!(
        kinds,
        vec![
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Table,
                name: Some(name_obj_parts("`users`", vec!["users"])),
            })),
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Index,
                name: Some(name_obj_parts("`users_email_idx`", vec!["users_email_idx"])),
            })),
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Function,
                name: Some(name_obj_parts("`audit_users`", vec!["audit_users"])),
            })),
            Some(SqlStatementKind::Ddl(DdlStatementKind {
                entity: EntityKind::Trigger,
                name: Some(name_obj_parts("`users_ai`", vec!["users_ai"])),
            })),
        ]
    );
}

/// Verifies all supported DML forms classify, including CTE-backed variants.
#[test]
fn dml_classification_covers_all_supported_forms() {
    let kinds = kinds(
        "SELECT * FROM users;
         INSERT INTO users(id) VALUES (1);
         UPDATE users SET id = id;
         DELETE FROM users;
         WITH u AS (SELECT * FROM users) SELECT * FROM u;
         WITH u AS (SELECT * FROM users) INSERT INTO audit SELECT * FROM u;
         WITH u AS (SELECT * FROM users) UPDATE users SET id = id;
         WITH u AS (SELECT * FROM users) DELETE FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            dml(DmlStatementKind::Select),
            dml(DmlStatementKind::Insert),
            dml(DmlStatementKind::Update),
            dml(DmlStatementKind::Delete),
            with(DmlStatementKind::Select),
            with(DmlStatementKind::Insert),
            with(DmlStatementKind::Update),
            with(DmlStatementKind::Delete),
        ]
    );
}

/// Verifies unsupported broad SQL verbs remain unclassified by the narrow public taxonomy.
#[test]
fn unsupported_statement_kinds_remain_unclassified() {
    let kinds = kinds(
        "ALTER TABLE users ADD COLUMN name text;
         DROP TABLE users;
         COMMENT ON TABLE users IS 'x';
         GRANT SELECT ON users TO app;
         REVOKE SELECT ON users FROM app;
         CALL refresh_users();
         VALUES (1);
         MERGE INTO users USING incoming ON true WHEN MATCHED THEN UPDATE SET id = id;
         EXPLAIN SELECT * FROM users;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![None, None, None, None, None, None, None, None, None]
    );
}

/// Verifies keywords hidden inside bodies and literals do not affect classification.
#[test]
fn classification_ignores_nested_keywords() {
    let kinds = kinds(
        "CREATE FUNCTION f() RETURNS text LANGUAGE sql AS $$ SELECT 'DELETE FROM users'; $$;
         SELECT 'CREATE TABLE nope (id int)' AS text;",
        Dialect::Postgres,
    );
    assert_eq!(
        kinds,
        vec![
            ddl(EntityKind::Function, "f"),
            dml(DmlStatementKind::Select),
        ]
    );
}

/// Verifies unclassified segments are still returned for downstream parsing.
#[test]
fn unclassified_segments_still_segment() {
    let segments = segment_sql(
        "ALTER TABLE users ADD COLUMN name text; DROP TABLE old_users;",
        Dialect::Postgres,
    )
    .unwrap();
    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].kind, None);
    assert_eq!(segments[1].kind, None);
}
