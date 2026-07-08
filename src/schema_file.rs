//! Native schema file loading helpers.
//!
//! These helpers read filesystem paths in the root `gaman` crate and then call
//! `gaman-core` string parsers. Keeping them here preserves CLI ergonomics
//! without adding filesystem access to the offline core.

use std::fs;
use std::path::{Path, PathBuf};

use gaman_core::states::{Schema, SchemaLoadError};

/// Load a schema from a native filesystem path.
///
/// Files are parsed by extension and directories are merged in deterministic
/// path order. This helper belongs to the root crate so `gaman-core` can remain
/// string-only and usable in offline/WASM contexts.
pub fn load_schema_path(path: impl AsRef<Path>) -> Result<Schema, SchemaLoadError> {
    let path = path.as_ref();
    if path.is_dir() {
        load_schema_dir(path)
    } else {
        load_schema_file(path)
    }
}

/// Load one schema file and delegate parsing to `gaman-core` string APIs.
///
/// `.sql` files use the SQL parser, `.json` files use JSON, and all other file
/// extensions are treated as YAML to preserve the legacy CLI behavior.
pub fn load_schema_file(path: impl AsRef<Path>) -> Result<Schema, SchemaLoadError> {
    let path = path.as_ref();
    let raw =
        fs::read_to_string(path).map_err(|e| SchemaLoadError::Io(path.display().to_string(), e))?;
    match path.extension().and_then(|ext| ext.to_str()) {
        Some("sql") => Schema::from_sql_str(&raw),
        Some("json") => Schema::from_json_str(&raw),
        _ => Schema::from_yaml_str(&raw),
    }
}

/// Load and merge every schema file in a native filesystem directory.
///
/// Supported file extensions are `.yaml`, `.yml`, `.json`, and `.sql`. Table
/// name collisions are rejected; other top-level objects use the same
/// last-writer-wins behavior as schema merging.
pub fn load_schema_dir(dir: impl AsRef<Path>) -> Result<Schema, SchemaLoadError> {
    let dir = dir.as_ref();
    let mut entries = schema_entries(dir)?;
    entries.sort();

    let mut merged = Schema::default();
    for path in entries {
        let fragment = load_schema_file(&path)?;
        merged = merge_fragment(merged, fragment, dir, &path)?;
    }
    Ok(merged)
}

fn schema_entries(dir: &Path) -> Result<Vec<PathBuf>, SchemaLoadError> {
    Ok(fs::read_dir(dir)
        .map_err(|e| SchemaLoadError::Io(dir.display().to_string(), e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_schema_file(path))
        .collect())
}

fn is_schema_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "yml" | "json" | "sql")
    )
}

fn merge_fragment(
    mut merged: Schema,
    fragment: Schema,
    base: &Path,
    path: &Path,
) -> Result<Schema, SchemaLoadError> {
    for (name, table) in fragment.tables {
        if merged.tables.contains_key(&name) {
            return Err(SchemaLoadError::Merge {
                table: name,
                a: base.display().to_string(),
                b: path.display().to_string(),
            });
        }
        merged.tables.insert(name, table);
    }
    merged.views.extend(fragment.views);
    merged.functions.extend(fragment.functions);
    merged.extensions.extend(fragment.extensions);
    merged.enums.extend(fragment.enums);
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies that SQL files are read natively and parsed through core's SQL string parser.
    #[test]
    fn load_schema_file_parses_sql() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("schema.sql");
        fs::write(
            &path,
            "CREATE TABLE items (id bigserial PRIMARY KEY, label text NOT NULL);",
        )
        .expect("write sql schema");

        let schema = load_schema_file(&path).expect("load sql schema");

        assert!(schema.tables.contains_key("items"));
    }

    /// Verifies deterministic directory loading across YAML, JSON, and SQL fragments.
    #[test]
    fn load_schema_dir_merges_supported_file_types() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("a.yaml"),
            "tables:\n  categories:\n    columns:\n      - name: id\n        type: integer\n        primary_key: true\n",
        )
        .expect("write yaml schema");
        fs::write(
            dir.path().join("b.json"),
            r#"{"tables":{"labels":{"columns":[{"name":"id","type":"integer","primary_key":true}]}}}"#,
        )
        .expect("write json schema");
        fs::write(
            dir.path().join("c.sql"),
            "CREATE TABLE tags (id bigserial PRIMARY KEY, name text NOT NULL);",
        )
        .expect("write sql schema");

        let schema = load_schema_dir(dir.path()).expect("load schema dir");

        assert!(schema.tables.contains_key("categories"));
        assert!(schema.tables.contains_key("labels"));
        assert!(schema.tables.contains_key("tags"));
    }

    /// Verifies that directory merging rejects duplicate table definitions across files.
    #[test]
    fn load_schema_dir_rejects_duplicate_table_names() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("a.sql"),
            "CREATE TABLE things (id bigserial PRIMARY KEY);",
        )
        .expect("write first sql schema");
        fs::write(dir.path().join("b.sql"), "CREATE TABLE things (name text);")
            .expect("write second sql schema");

        let err = load_schema_dir(dir.path()).expect_err("duplicate table should fail");

        assert!(
            err.to_string().contains("things"),
            "expected error mentioning 'things', got: {err}"
        );
    }
}
