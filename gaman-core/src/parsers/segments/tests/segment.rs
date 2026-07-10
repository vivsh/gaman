use super::super::segment_sql;
use crate::dialects::Dialect;
use crate::parsers::ParseError;

fn sqls(sql: &str, dialect: Dialect) -> Vec<String> {
    segment_sql(sql, dialect)
        .unwrap()
        .into_iter()
        .map(|segment| segment.sql.trim().to_string())
        .collect()
}

/// Verifies ordinary semicolon-separated PostgreSQL statements split at top-level semicolons.
#[test]
fn postgres_semicolon_separated_statements() {
    let statements = sqls(
        "CREATE TABLE users (id int); CREATE INDEX users_id_idx ON users(id);",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[0].starts_with("CREATE TABLE users"));
    assert!(statements[1].starts_with("CREATE INDEX users_id_idx"));
}

/// Verifies the final statement does not require a trailing semicolon.
#[test]
fn final_statement_without_semicolon() {
    let statements = sqls(
        "CREATE TABLE users (id int); CREATE VIEW user_ids AS SELECT id FROM users",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[1].starts_with("CREATE VIEW user_ids"));
}

/// Verifies a new top-level CREATE can start a new segment when a semicolon is omitted.
#[test]
fn missing_semicolon_before_new_top_level_statement() {
    let statements = sqls(
        "CREATE TABLE users (id int)\nCREATE INDEX users_id_idx ON users(id)",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[0].starts_with("CREATE TABLE users"));
    assert!(statements[1].starts_with("CREATE INDEX users_id_idx"));
}

/// Verifies unknown CREATE modifiers do not hide a view determinant from segmentation.
#[test]
fn unknown_create_modifiers_do_not_split_view_body() {
    let source = "CREATE FUTURE OPTION VIEW active_users AS\nSELECT id FROM users;\nCREATE TABLE audit (id int);";
    let segments = segment_sql(source, Dialect::Postgres).unwrap();

    assert_eq!(segments.len(), 2);
    assert!(segments[0].sql.contains("VIEW active_users AS\nSELECT"));
    assert!(segments[1].sql.contains("CREATE TABLE audit"));
}

/// Verifies unknown CREATE modifiers do not disable SQLite trigger body tracking.
#[test]
fn unknown_create_modifiers_do_not_split_sqlite_trigger_body() {
    let source = "CREATE FUTURE TRIGGER users_audit AFTER INSERT ON users BEGIN\nINSERT INTO audit(id) VALUES (NEW.id);\nEND;\nCREATE TABLE posts (id int);";
    let segments = segment_sql(source, Dialect::Sqlite).unwrap();

    assert_eq!(segments.len(), 2);
    assert!(segments[0].sql.contains("INSERT INTO audit"));
    assert!(segments[1].sql.contains("CREATE TABLE posts"));
}

/// Verifies semicolons inside strings, comments, and expressions do not split statements.
#[test]
fn no_split_inside_strings_comments_or_brackets() {
    let statements = sqls(
        "CREATE TABLE users (note text DEFAULT 'a; b', value int CHECK (value IN (1, 2))); -- ;\nCREATE TABLE posts (id int);",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[0].contains("'a; b'"));
}

/// Verifies PostgreSQL dollar-quoted function bodies keep internal semicolons in one segment.
#[test]
fn postgres_dollar_quoted_function_body() {
    let statements = sqls(
        "CREATE FUNCTION f() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO audit VALUES (1); RETURN NEW; END; $$;\nCREATE TABLE users (id int);",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[0].contains("RETURN NEW;"));
    assert!(statements[1].starts_with("CREATE TABLE users"));
}

/// Verifies PostgreSQL nested block comments do not terminate the outer comment early.
#[test]
fn postgres_nested_block_comments() {
    let statements = sqls(
        "/* outer /* inner ; */ still outer */ CREATE TABLE users (id int);",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 1);
    assert!(statements[0].contains("CREATE TABLE users"));
}

/// Verifies CREATE VIEW AS SELECT remains one segment despite SELECT being a statement starter.
#[test]
fn postgres_create_view_as_select_is_one_segment() {
    let statements = sqls(
        "CREATE VIEW active_users AS\nSELECT id FROM users\nCREATE TABLE posts (id int)",
        Dialect::Postgres,
    );
    assert_eq!(statements.len(), 1);
}

/// Verifies SQLite trigger bodies keep internal semicolon statements in one segment.
#[test]
fn sqlite_trigger_body_internal_semicolons() {
    let statements = sqls(
        "CREATE TABLE users (id integer);\nCREATE TRIGGER users_ai AFTER INSERT ON users BEGIN INSERT INTO audit VALUES (NEW.id); UPDATE users SET id = id; END;\nCREATE VIEW user_ids AS SELECT id FROM users;",
        Dialect::Sqlite,
    );
    assert_eq!(statements.len(), 3);
    assert!(statements[1].contains("UPDATE users SET id = id;"));
}

/// Verifies SQLite CREATE VIEW AS SELECT remains one segment.
#[test]
fn sqlite_create_view_as_select_is_one_segment() {
    let statements = sqls(
        "CREATE VIEW active_users AS\nSELECT id FROM users\nCREATE TABLE posts (id integer)",
        Dialect::Sqlite,
    );
    assert_eq!(statements.len(), 1);
}

/// Verifies MySQL DELIMITER directives split routines by the active delimiter.
#[test]
fn mysql_delimiter_routine_body() {
    let statements = sqls(
        "DELIMITER //\nCREATE PROCEDURE p() BEGIN INSERT INTO audit VALUES (1); UPDATE audit SET id = id; END//\nDELIMITER ;\nCREATE TABLE users (id int);",
        Dialect::Mysql,
    );
    assert_eq!(statements.len(), 2);
    assert!(statements[0].starts_with("CREATE PROCEDURE p()"));
    assert!(statements[0].contains("UPDATE audit SET id = id;"));
    assert!(statements[1].starts_with("CREATE TABLE users"));
}

/// Verifies MySQL default semicolon segmentation works without delimiter directives.
#[test]
fn mysql_default_semicolon_segmentation() {
    let statements = sqls(
        "USE app; CREATE TABLE users (id int); SHOW TABLES;",
        Dialect::Mysql,
    );
    assert_eq!(statements.len(), 3);
}

/// Verifies comment-only and whitespace-only input produces no segments.
#[test]
fn comment_only_input_returns_no_segments() {
    let statements = segment_sql("  -- only comment\n/* block */", Dialect::Postgres).unwrap();
    assert!(statements.is_empty());
}

/// Verifies unterminated strings return a segmentation error with location context.
#[test]
fn unterminated_string_is_segment_error() {
    let err = segment_sql(
        "CREATE TABLE users (name text DEFAULT 'x);",
        Dialect::Postgres,
    )
    .unwrap_err();
    assert!(matches!(err, ParseError::Segment { line: 1, .. }));
}

/// Verifies unterminated dollar quotes return a segmentation error.
#[test]
fn unterminated_dollar_quote_is_segment_error() {
    let err = segment_sql(
        "CREATE FUNCTION f() RETURNS int LANGUAGE sql AS $$ SELECT 1;",
        Dialect::Postgres,
    )
    .unwrap_err();
    assert!(matches!(err, ParseError::Segment { .. }));
}

/// Verifies MySQL dialect names and URLs resolve to the MySQL stub dialect.
#[test]
fn mysql_dialect_names_and_urls_parse() {
    assert_eq!(Dialect::parse("mysql"), Some(Dialect::Mysql));
    assert_eq!(Dialect::parse("mariadb"), Some(Dialect::Mysql));
    assert_eq!(
        Dialect::parse_from_url("mysql://localhost/app"),
        Ok(Dialect::Mysql)
    );
    assert_eq!(
        Dialect::parse_from_url("mariadb://localhost/app"),
        Ok(Dialect::Mysql)
    );
}

/// Verifies MySQL migration rendering remains explicitly unsupported.
#[test]
fn mysql_rendering_is_unsupported() {
    let migration = crate::migrations::Migration {
        id: "m1".to_string(),
        dependencies: Vec::new(),
        operations: Vec::new(),
        atomic: true,
    };
    let err = Dialect::Mysql
        .migration_to_sql(&migration, &crate::states::Schema::default())
        .unwrap_err();
    assert!(err.to_string().contains("MySQL dialect is not implemented"));
}

/// Verifies byte offsets extract the exact returned SQL slice.
#[test]
fn segment_byte_offsets_extract_returned_sql() {
    let source = "-- meta: users\nCREATE TABLE users (id int);  \nCREATE TABLE posts (id int);";
    let segments = segment_sql(source, Dialect::Postgres).unwrap();

    assert_eq!(segments.len(), 2);
    for segment in &segments {
        assert_eq!(&source[segment.start_byte..segment.end_byte], segment.sql);
    }
    assert_eq!(segments[0].start_byte, 0);
    assert!(segments[0].sql.starts_with("-- meta: users"));
    assert!(!segments[0].sql.ends_with(';'));
    assert!(!segments[0].sql.ends_with("  "));
}

/// Verifies comments after a terminator attach to the following statement.
#[test]
fn comment_after_terminator_attaches_to_next_segment() {
    let source = "CREATE TABLE users (id int);\n-- meta: posts\nCREATE TABLE posts (id int);";
    let segments = segment_sql(source, Dialect::Postgres).unwrap();

    assert_eq!(segments.len(), 2);
    assert_eq!(segments[0].sql, "CREATE TABLE users (id int)");
    assert!(
        segments[1]
            .sql
            .starts_with("\n-- meta: posts\nCREATE TABLE posts")
    );
    assert_eq!(
        &source[segments[1].start_byte..segments[1].end_byte],
        segments[1].sql
    );
}

/// Verifies a final comment after a completed statement does not create a segment.
#[test]
fn final_comment_after_terminator_does_not_emit_segment() {
    let source = "CREATE TABLE users (id int);\n-- trailing metadata for future statement";
    let segments = segment_sql(source, Dialect::Postgres).unwrap();

    assert_eq!(segments.len(), 1);
    assert_eq!(segments[0].sql, "CREATE TABLE users (id int)");
}

/// Verifies MySQL DELIMITER directives are excluded while metadata comments after them attach.
#[test]
fn mysql_delimiter_directive_excluded_but_following_comment_preserved() {
    let source = "DELIMITER //\n-- meta: function\nCREATE FUNCTION f() RETURNS INT BEGIN RETURN 1; END//\nDELIMITER ;\nCREATE TABLE users (id int);";
    let segments = segment_sql(source, Dialect::Mysql).unwrap();

    assert_eq!(segments.len(), 2);
    assert!(
        segments[0]
            .sql
            .starts_with("-- meta: function\nCREATE FUNCTION f()")
    );
    assert!(!segments[0].sql.contains("DELIMITER"));
    assert!(!segments[0].sql.ends_with("//"));
    assert_eq!(
        &source[segments[0].start_byte..segments[0].end_byte],
        segments[0].sql
    );
    assert!(segments[1].sql.starts_with("CREATE TABLE users"));
}
