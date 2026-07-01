use super::{SqlParseError, parse_sql};
use crate::states::{Constraint, Schema, Volatility};
#[cfg(feature = "fs")]
use std::io::Write;
#[cfg(feature = "fs")]
use tempfile::NamedTempFile;
#[cfg(feature = "fs")]
use tempfile::TempDir;

// Helpers for SQL parser tests.

fn table<'a>(schema: &'a Schema, name: &str) -> &'a crate::states::Table {
    schema
        .tables
        .get(name)
        .unwrap_or_else(|| panic!("table '{name}' not found"))
}

fn col<'a>(schema: &'a Schema, tbl: &str, col: &str) -> &'a crate::states::Column {
    let t = table(schema, tbl);
    t.columns
        .iter()
        .find(|c| c.name == col)
        .unwrap_or_else(|| panic!("column '{col}' not found on table '{tbl}'"))
}

/// Verifies a minimal CREATE TABLE with a bigserial PK and a text column parses correctly.
#[test]
fn test_simple_table() {
    let sql = "CREATE TABLE users (id bigserial PRIMARY KEY, name text NOT NULL);";
    let schema = parse_sql(sql).unwrap();

    assert!(schema.tables.contains_key("users"));
    let id = col(&schema, "users", "id");
    assert!(id.primary_key);
    assert!(!id.nullable);

    let name = col(&schema, "users", "name");
    assert_eq!(name.col_type, "text");
    assert!(!name.nullable);
}

/// Confirms NULL and NOT NULL column modifiers set `nullable` correctly.
#[test]
fn test_column_nullability() {
    let sql = "CREATE TABLE t (
        a integer NOT NULL,
        b integer NULL,
        c integer
    );";
    let schema = parse_sql(sql).unwrap();
    assert!(!col(&schema, "t", "a").nullable);
    assert!(col(&schema, "t", "b").nullable);
    // Postgres default: nullable when neither NULL nor NOT NULL is specified
    assert!(col(&schema, "t", "c").nullable);
}

/// Checks that a column DEFAULT expression is captured verbatim.
#[test]
fn test_column_default() {
    let sql = "CREATE TABLE t (created_at timestamptz NOT NULL DEFAULT now());";
    let schema = parse_sql(sql).unwrap();
    let c = col(&schema, "t", "created_at");
    assert_eq!(c.default.as_deref(), Some("now()"));
}

/// Ensures a table-level PRIMARY KEY constraint marks the right column.
#[test]
fn test_table_level_primary_key() {
    let sql = "CREATE TABLE orders (
        id bigint,
        PRIMARY KEY (id)
    );";
    let schema = parse_sql(sql).unwrap();
    let table = table(&schema, "orders");
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.name, "orders_pkey");
    assert_eq!(pk.columns, ["id"]);
    assert!(col(&schema, "orders", "id").primary_key);
}

#[test]
fn test_named_composite_primary_key_preserves_order() {
    let sql = "CREATE TABLE order_lines (
        order_id bigint,
        tenant_id bigint,
        CONSTRAINT order_lines_identity PRIMARY KEY (tenant_id, order_id)
    );";
    let schema = parse_sql(sql).unwrap();
    let table = table(&schema, "order_lines");
    let pk = table.primary_key.as_ref().expect("primary key");
    assert_eq!(pk.name, "order_lines_identity");
    assert_eq!(pk.columns, ["tenant_id", "order_id"]);
    assert_eq!(table.primary_key_column_names(), ["tenant_id", "order_id"]);
    assert!(col(&schema, "order_lines", "tenant_id").primary_key);
    assert!(col(&schema, "order_lines", "order_id").primary_key);
}

/// Parses an inline column-level FOREIGN KEY reference.
#[test]
fn test_column_level_foreign_key() {
    let sql = "CREATE TABLE posts (
        id bigserial PRIMARY KEY,
        user_id bigint NOT NULL REFERENCES users(id)
    );";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "posts");
    assert_eq!(t.foreign_keys.len(), 1);
    let fk = &t.foreign_keys[0];
    assert_eq!(fk.from_column, "user_id");
    assert_eq!(fk.to_table, "users");
    assert_eq!(fk.to_column, "id");
    assert_eq!(fk.name, "posts_user_id_fkey");
}

/// Parses a table-level FOREIGN KEY with an explicit constraint name.
#[test]
fn test_table_level_foreign_key_named() {
    let sql = "CREATE TABLE posts (
        id bigserial PRIMARY KEY,
        user_id bigint NOT NULL,
        CONSTRAINT fk_posts_user FOREIGN KEY (user_id) REFERENCES users(id)
    );";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "posts");
    assert_eq!(t.foreign_keys.len(), 1);
    assert_eq!(t.foreign_keys[0].name, "fk_posts_user");
}

/// Parses a table-level UNIQUE constraint and maps it to `Constraint::Unique`.
#[test]
fn test_table_level_unique_constraint() {
    let sql = "CREATE TABLE users (
        id bigserial PRIMARY KEY,
        email text NOT NULL,
        CONSTRAINT users_email_unique UNIQUE (email)
    );";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "users");
    let unique = t
        .constraints
        .iter()
        .find(|c| c.name() == "users_email_unique")
        .expect("unique constraint not found");
    assert!(matches!(unique, Constraint::Unique { columns, .. } if columns == &["email"]));
}

/// Parses a table-level CHECK constraint and maps it to `Constraint::Check`.
#[test]
fn test_table_level_check_constraint() {
    let sql = "CREATE TABLE products (
        id bigserial PRIMARY KEY,
        price numeric NOT NULL,
        CONSTRAINT products_price_positive CHECK (price > 0)
    );";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "products");
    let chk = t
        .constraints
        .iter()
        .find(|c| c.name() == "products_price_positive")
        .expect("check constraint not found");
    assert!(matches!(chk, Constraint::Check { expression, .. } if expression.contains("price")));
}

/// Parses a column-level CHECK constraint and auto-generates a name.
#[test]
fn test_column_level_check_constraint() {
    let sql = "CREATE TABLE products (
        price numeric CHECK (price > 0)
    );";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "products");
    assert_eq!(t.constraints.len(), 1);
    assert_eq!(t.constraints[0].name(), "products_price_check");
}

/// Verifies mixed nullability across multiple columns.
#[test]
fn test_mixed_nullability() {
    let sql = "CREATE TABLE t (
        a text NOT NULL,
        b text NULL,
        c text NOT NULL,
        d text
    );";
    let schema = parse_sql(sql).unwrap();
    assert!(!col(&schema, "t", "a").nullable);
    assert!(col(&schema, "t", "b").nullable);
    assert!(!col(&schema, "t", "c").nullable);
    // Postgres default: nullable when neither NULL nor NOT NULL is specified
    assert!(col(&schema, "t", "d").nullable);
}

/// A plain CREATE INDEX is attached to the target table.
#[test]
fn test_create_index() {
    let sql = "
        CREATE TABLE articles (id bigserial PRIMARY KEY, title text);
        CREATE INDEX articles_title_idx ON articles (title);
    ";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "articles");
    assert_eq!(t.indexes.len(), 1);
    assert_eq!(t.indexes[0].name, "articles_title_idx");
    assert!(!t.indexes[0].unique);
    assert_eq!(t.indexes[0].columns, ["title"]);
}

/// CREATE UNIQUE INDEX sets `unique = true` on the resulting `Index`.
#[test]
fn test_create_unique_index() {
    let sql = "
        CREATE TABLE users (id bigserial PRIMARY KEY, email text);
        CREATE UNIQUE INDEX users_email_idx ON users (email);
    ";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "users");
    assert!(t.indexes[0].unique);
}

/// A partial index with a WHERE clause captures the predicate expression.
#[test]
fn test_partial_index_with_predicate() {
    let sql = "
        CREATE TABLE events (id bigserial PRIMARY KEY, active boolean);
        CREATE INDEX events_active_idx ON events (id) WHERE active = true;
    ";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "events");
    let idx = &t.indexes[0];
    assert!(
        idx.predicate.is_some(),
        "expected a predicate on the partial index"
    );
    assert!(idx.predicate.as_deref().unwrap().contains("active"));
}

/// CREATE INDEX that references a table not in the SQL returns an error.
#[test]
fn test_index_unknown_table_is_error() {
    let sql = "CREATE INDEX missing_idx ON ghost (id);";
    let err = parse_sql(sql).unwrap_err();
    assert!(
        matches!(err, SqlParseError::UnknownTable { .. }),
        "expected UnknownTable error, got: {err}"
    );
}

/// A CREATE VIEW is stored in `schema.views` with its SELECT definition.
#[test]
fn test_create_view() {
    let sql = "CREATE VIEW active_users AS SELECT id, name FROM users WHERE active = true;";
    let schema = parse_sql(sql).unwrap();
    assert!(schema.views.contains_key("active_users"));
    let v = &schema.views["active_users"];
    assert_eq!(v.name, "active_users");
    assert!(v.definition.contains("active"));
}

/// CREATE EXTENSION is stored with name and optional version.
#[test]
fn test_create_extension() {
    let sql = "CREATE EXTENSION IF NOT EXISTS \"uuid-ossp\";";
    let schema = parse_sql(sql).unwrap();
    assert!(schema.extensions.contains_key("uuid-ossp"));
}

/// A CREATE TYPE AS ENUM is parsed into `schema.enums` with its label values.
#[test]
fn test_create_enum() {
    let sql = "CREATE TYPE mood AS ENUM ('happy', 'sad', 'neutral');";
    let schema = parse_sql(sql).unwrap();
    assert!(schema.enums.contains_key("mood"));
    let e = &schema.enums["mood"];
    assert_eq!(e.values, ["happy", "sad", "neutral"]);
}

/// A basic PL/pgSQL function is parsed into `schema.functions`.
#[test]
fn test_create_function() {
    let sql = r#"
        CREATE FUNCTION add_numbers(a integer, b integer) RETURNS integer
        LANGUAGE sql
        AS $$ SELECT a + b; $$;
    "#;
    let schema = parse_sql(sql).unwrap();
    assert!(schema.functions.contains_key("add_numbers"));
    let f = &schema.functions["add_numbers"];
    assert_eq!(f.language, "sql");
    assert!(f.arguments.contains("integer"));
    assert_eq!(f.returns, "integer");
}

/// IMMUTABLE / STABLE / VOLATILE function attributes are mapped to `Volatility`.
#[test]
fn test_function_volatility() {
    let sql = r#"
        CREATE FUNCTION pure_fn() RETURNS integer LANGUAGE sql IMMUTABLE AS $$ SELECT 1; $$;
    "#;
    let schema = parse_sql(sql).unwrap();
    let f = &schema.functions["pure_fn"];
    assert_eq!(f.volatility, Volatility::Immutable);
}

/// SECURITY DEFINER is captured on the `FunctionDef`.
#[test]
fn test_function_security_definer() {
    let sql = r#"
        CREATE FUNCTION privileged() RETURNS void LANGUAGE sql SECURITY DEFINER AS $$ SELECT 1; $$;
    "#;
    let schema = parse_sql(sql).unwrap();
    let f = &schema.functions["privileged"];
    assert!(f.security_definer);
}

/// `public.tablename` resolves to schema = None (treated as default schema).
#[test]
fn test_public_schema_is_none() {
    let sql = "CREATE TABLE public.users (id bigserial PRIMARY KEY);";
    let schema = parse_sql(sql).unwrap();
    // Key should be just "users"
    assert!(schema.tables.contains_key("users"), "expected key 'users'");
    assert_eq!(schema.tables["users"].schema, None);
}

/// `myschema.tablename` resolves to schema = Some("myschema").
#[test]
fn test_custom_schema_is_preserved() {
    let sql = "CREATE TABLE analytics.events (id bigserial PRIMARY KEY);";
    let schema = parse_sql(sql).unwrap();
    assert!(schema.tables.contains_key("analytics.events"));
    assert_eq!(
        schema.tables["analytics.events"].schema,
        Some("analytics".to_string())
    );
}

/// Multiple CREATE statements in one SQL string are all loaded into the schema.
#[test]
fn test_multi_statement_sql() {
    let sql = "
        CREATE TABLE users (id bigserial PRIMARY KEY, name text NOT NULL);
        CREATE TABLE posts (id bigserial PRIMARY KEY, user_id bigint NOT NULL);
        CREATE INDEX posts_user_idx ON posts (user_id);
    ";
    let schema = parse_sql(sql).unwrap();
    assert!(schema.tables.contains_key("users"));
    assert!(schema.tables.contains_key("posts"));
    assert_eq!(table(&schema, "posts").indexes.len(), 1);
}

/// An unsupported statement (INSERT) produces `SqlParseError::Unsupported`.
#[test]
fn test_unsupported_statement_is_error() {
    let sql = "INSERT INTO users (name) VALUES ('alice');";
    let err = parse_sql(sql).unwrap_err();
    assert!(
        matches!(err, SqlParseError::Unsupported { .. }),
        "expected Unsupported error, got: {err}"
    );
}

/// Duplicate table names produce `SqlParseError::DuplicateTable`.
#[test]
fn test_duplicate_table_is_error() {
    let sql = "
        CREATE TABLE users (id bigserial PRIMARY KEY);
        CREATE TABLE users (email text);
    ";
    let err = parse_sql(sql).unwrap_err();
    assert!(
        matches!(err, SqlParseError::DuplicateTable(ref name) if name == "users"),
        "expected DuplicateTable error, got: {err}"
    );
}

/// `Schema::load()` dispatches to the SQL parser for `.sql` files.
#[test]
#[cfg(feature = "fs")]
fn test_schema_load_sql_file() {
    let mut f = NamedTempFile::with_suffix(".sql").unwrap();
    write!(
        f,
        "CREATE TABLE items (id bigserial PRIMARY KEY, label text NOT NULL);"
    )
    .unwrap();
    let schema = Schema::from_file(f.path()).unwrap();
    assert!(schema.tables.contains_key("items"));
}

/// `Schema::from_dir()` picks up both `.yaml` and `.sql` files in a directory.
#[test]
#[cfg(feature = "fs")]
fn test_from_dir_mixed_yaml_and_sql() {
    let dir = TempDir::new().unwrap();

    // Write a YAML schema fragment
    let yaml_path = dir.path().join("a.yaml");
    std::fs::write(
        &yaml_path,
        "tables:\n  categories:\n    columns:\n      - name: id\n        type: bigserial\n        primary_key: true\n",
    )
    .unwrap();

    // Write a SQL schema fragment
    let sql_path = dir.path().join("b.sql");
    std::fs::write(
        &sql_path,
        "CREATE TABLE tags (id bigserial PRIMARY KEY, name text NOT NULL);",
    )
    .unwrap();

    let schema = Schema::from_dir(dir.path()).unwrap();
    assert!(
        schema.tables.contains_key("categories"),
        "YAML table missing"
    );
    assert!(schema.tables.contains_key("tags"), "SQL table missing");
}

/// `Schema::from_dir()` returns an error when the same table name appears in
/// two different files.
#[test]
#[cfg(feature = "fs")]
fn test_from_dir_duplicate_table_error() {
    let dir = TempDir::new().unwrap();

    std::fs::write(
        dir.path().join("a.sql"),
        "CREATE TABLE things (id bigserial PRIMARY KEY);",
    )
    .unwrap();
    std::fs::write(dir.path().join("b.sql"), "CREATE TABLE things (name text);").unwrap();

    let err = Schema::from_dir(dir.path()).unwrap_err();
    // Either SqlParseError::DuplicateTable (same file) or SchemaLoadError::Merge (cross-file)
    // Here it's cross-file so we get SchemaLoadError::Merge.
    assert!(
        err.to_string().contains("things"),
        "expected error mentioning 'things', got: {err}"
    );
}

/// Auto-generated index name is derived from table and columns when no name is provided.
#[test]
fn test_index_auto_name() {
    let sql = "
        CREATE TABLE orders (id bigserial PRIMARY KEY, status text);
        CREATE INDEX ON orders (status);
    ";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "orders");
    assert_eq!(t.indexes.len(), 1);
    assert_eq!(t.indexes[0].name, "orders_status_idx");
}

/// A column-level UNIQUE constraint generates an auto-name from table + column.
#[test]
fn test_column_level_unique_auto_name() {
    let sql = "CREATE TABLE users (id bigserial PRIMARY KEY, email text UNIQUE);";
    let schema = parse_sql(sql).unwrap();
    let t = table(&schema, "users");
    assert_eq!(t.constraints.len(), 1);
    assert_eq!(t.constraints[0].name(), "users_email_key");
}

/// Function argument modes (IN, OUT, INOUT) are included in the argument string.
#[test]
fn test_function_argument_modes() {
    let sql = r#"
        CREATE FUNCTION compute(IN a integer, OUT b integer)
        RETURNS integer LANGUAGE sql AS $$ SELECT a; $$;
    "#;
    let schema = parse_sql(sql).unwrap();
    let f = &schema.functions["compute"];
    assert!(f.arguments.contains("IN"), "expected IN mode in arguments");
    assert!(
        f.arguments.contains("OUT"),
        "expected OUT mode in arguments"
    );
}
