//! Native schema file loading helpers.
//!
//! These helpers read filesystem paths in the root `gaman` crate and then call
//! `gaman-core` string parsers. Keeping them here preserves CLI ergonomics
//! without adding filesystem access to the offline core.

#[cfg(feature = "db")]
use gaman_core::SqlInput;
use gaman_core::dialects::Dialect;
use gaman_core::states::{Schema, SchemaLoadError};
use std::fs;
use std::path::{Path, PathBuf};

/// One native schema input selected for `check_schema` output.
#[cfg(feature = "db")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SchemaCheckPathEntry {
    /// SQL source to prepare through the selected database executor.
    Sql(SqlInput),
    /// A non-SQL schema source intentionally skipped by `check_schema`.
    Ignored { name: String, reason: String },
}

/// Load a schema from a native filesystem path.
///
/// Files are parsed by extension and directories are merged in deterministic
/// path order. This helper belongs to the root crate so `gaman-core` can remain
/// string-only and usable in offline/WASM contexts.
pub fn load_schema_path(
    path: impl AsRef<Path>,
    dialect: Dialect,
) -> Result<Schema, SchemaLoadError> {
    let path = path.as_ref();
    if path.is_dir() {
        load_schema_dir(path, dialect)
    } else {
        load_schema_file(path, dialect)
    }
}

/// Load one schema file and delegate parsing to `gaman-core` string APIs.
///
/// `.sql` files use the SQL parser, `.json` files use JSON, and all other file
/// extensions are treated as YAML to preserve the legacy CLI behavior.
pub fn load_schema_file(
    path: impl AsRef<Path>,
    dialect: Dialect,
) -> Result<Schema, SchemaLoadError> {
    let path = path.as_ref();
    let raw =
        fs::read_to_string(path).map_err(|e| SchemaLoadError::Io(path.display().to_string(), e))?;
    let loaded = match path.extension().and_then(|ext| ext.to_str()) {
        Some("sql") => Schema::from_sql_str(&raw, dialect),
        Some("json") => Schema::from_json_str(&raw, dialect),
        _ => Schema::from_yaml_str(&raw, dialect),
    };
    loaded.map_err(|err| schema_error_with_path(path, err))
}

/// Load and merge every schema file in a native filesystem directory.
///
/// Supported file extensions are `.yaml`, `.yml`, `.json`, and `.sql`. Table
/// name collisions are rejected; other top-level objects use the same
/// last-writer-wins behavior as schema merging.
pub fn load_schema_dir(dir: impl AsRef<Path>, dialect: Dialect) -> Result<Schema, SchemaLoadError> {
    let dir = dir.as_ref();
    let mut entries = schema_entries(dir)?;
    entries.sort();

    let mut merged = Schema::default();
    for path in entries {
        let fragment = load_schema_file_raw(&path, dialect)?;
        merged = merge_fragment(merged, fragment, dir, &path)?;
    }
    merged.prepare_loaded(dialect)
}

fn schema_entries(dir: &Path) -> Result<Vec<PathBuf>, SchemaLoadError> {
    Ok(fs::read_dir(dir)
        .map_err(|e| SchemaLoadError::Io(dir.display().to_string(), e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| is_schema_file(path))
        .collect())
}

/// Collects immediate schema entries for live SQL preparation validation.
///
/// Files are ordered by descending modification time and then ascending path
/// for deterministic ties. YAML and JSON inputs remain visible as ignored
/// entries; only SQL source is read and supplied to the engine.
#[cfg(feature = "db")]
pub(crate) fn collect_schema_check_entries(
    path: impl AsRef<Path>,
) -> Result<Vec<SchemaCheckPathEntry>, SchemaLoadError> {
    let paths = schema_check_paths(path.as_ref())?;
    sort_schema_check_paths(paths)?
        .into_iter()
        .map(schema_check_entry)
        .collect()
}

#[cfg(feature = "db")]
fn schema_check_paths(path: &Path) -> Result<Vec<PathBuf>, SchemaLoadError> {
    let metadata = fs::metadata(path)
        .map_err(|error| SchemaLoadError::Io(path.display().to_string(), error))?;
    if metadata.is_dir() {
        schema_entries(path)
    } else if metadata.is_file() {
        Ok(vec![path.to_path_buf()])
    } else {
        Err(SchemaLoadError::Io(
            path.display().to_string(),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "not a regular file or directory",
            ),
        ))
    }
}

#[cfg(feature = "db")]
fn sort_schema_check_paths(paths: Vec<PathBuf>) -> Result<Vec<PathBuf>, SchemaLoadError> {
    let mut dated_paths = Vec::with_capacity(paths.len());
    for path in paths {
        let modified = fs::metadata(&path)
            .and_then(|metadata| metadata.modified())
            .map_err(|error| SchemaLoadError::Io(path.display().to_string(), error))?;
        dated_paths.push((path, modified));
    }
    Ok(sort_dated_schema_check_paths(dated_paths))
}

#[cfg(feature = "db")]
fn sort_dated_schema_check_paths(
    mut dated_paths: Vec<(PathBuf, std::time::SystemTime)>,
) -> Vec<PathBuf> {
    dated_paths.sort_by(|(left_path, left_modified), (right_path, right_modified)| {
        right_modified
            .cmp(left_modified)
            .then_with(|| left_path.cmp(right_path))
    });
    dated_paths.into_iter().map(|(path, _)| path).collect()
}

#[cfg(feature = "db")]
fn schema_check_entry(path: PathBuf) -> Result<SchemaCheckPathEntry, SchemaLoadError> {
    let label = path.display().to_string();
    match path.extension().and_then(|extension| extension.to_str()) {
        Some("sql") => {
            let source = fs::read_to_string(&path)
                .map_err(|error| SchemaLoadError::Io(label.clone(), error))?;
            Ok(SchemaCheckPathEntry::Sql(SqlInput {
                name: label,
                sql: source,
            }))
        }
        Some("yaml" | "yml") => Ok(SchemaCheckPathEntry::Ignored {
            name: label,
            reason: "YAML schema input".to_string(),
        }),
        Some("json") => Ok(SchemaCheckPathEntry::Ignored {
            name: label,
            reason: "JSON schema input".to_string(),
        }),
        _ => Ok(SchemaCheckPathEntry::Ignored {
            name: label,
            reason: "not an SQL schema input".to_string(),
        }),
    }
}

fn is_schema_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yaml" | "yml" | "json" | "sql")
    )
}

fn load_schema_file_raw(path: &Path, dialect: Dialect) -> Result<Schema, SchemaLoadError> {
    let raw =
        fs::read_to_string(path).map_err(|e| SchemaLoadError::Io(path.display().to_string(), e))?;
    let loaded = match path.extension().and_then(|ext| ext.to_str()) {
        Some("sql") => Schema::from_sql_str_raw(&raw, dialect),
        Some("json") => Schema::from_json_str_input_raw(&raw),
        _ => Schema::from_yaml_str_input_raw(&raw),
    };
    loaded.map_err(|err| schema_error_with_path(path, err))
}

fn schema_error_with_path(path: &Path, err: SchemaLoadError) -> SchemaLoadError {
    match err {
        SchemaLoadError::Io(_, _) | SchemaLoadError::Path { .. } => err,
        other => SchemaLoadError::Path {
            path: path.display().to_string(),
            source: Box::new(other),
        },
    }
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

        let schema = load_schema_file(&path, Dialect::Postgres).expect("load sql schema");

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

        let schema = load_schema_dir(dir.path(), Dialect::Postgres).expect("load schema dir");

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

        let err = load_schema_dir(dir.path(), Dialect::Postgres)
            .expect_err("duplicate table should fail");

        assert!(
            err.to_string().contains("things"),
            "expected error mentioning 'things', got: {err}"
        );
    }

    /// Verifies schema checking reads SQL while retaining YAML and JSON as ignored inputs.
    #[cfg(feature = "db")]
    #[test]
    fn collect_schema_check_entries_keeps_non_sql_schema_inputs_visible() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(
            dir.path().join("schema.sql"),
            "CREATE TABLE users (id integer);",
        )
        .expect("write SQL schema");
        fs::write(dir.path().join("schema.yaml"), "tables: {}\n").expect("write YAML schema");
        fs::write(dir.path().join("schema.json"), "{}\n").expect("write JSON schema");

        let entries = collect_schema_check_entries(dir.path()).expect("collect schema checks");

        assert_eq!(entries.len(), 3);
        assert!(
            entries
                .iter()
                .any(|entry| matches!(entry, SchemaCheckPathEntry::Sql(_)))
        );
        assert!(entries.iter().any(|entry| matches!(
            entry,
            SchemaCheckPathEntry::Ignored { reason, .. } if reason == "YAML schema input"
        )));
        assert!(entries.iter().any(|entry| matches!(
            entry,
            SchemaCheckPathEntry::Ignored { reason, .. } if reason == "JSON schema input"
        )));
    }

    /// Verifies schema check ordering is newest-first with a deterministic path tie-breaker.
    #[cfg(feature = "db")]
    #[test]
    fn schema_check_ordering_breaks_equal_timestamps_by_path() {
        let old = std::time::UNIX_EPOCH;
        let new = old + std::time::Duration::from_secs(1);
        let paths = sort_dated_schema_check_paths(vec![
            (PathBuf::from("z.sql"), old),
            (PathBuf::from("a.sql"), old),
            (PathBuf::from("new.sql"), new),
        ]);

        assert_eq!(
            paths,
            vec![
                PathBuf::from("new.sql"),
                PathBuf::from("a.sql"),
                PathBuf::from("z.sql")
            ]
        );
    }

    /// Verifies missing schema paths produce a path-specific collection error.
    #[cfg(feature = "db")]
    #[test]
    fn collect_schema_check_entries_reports_missing_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing.sql");
        let error = collect_schema_check_entries(&path).expect_err("missing schema must fail");

        assert!(error.to_string().contains(&path.display().to_string()));
    }
}
