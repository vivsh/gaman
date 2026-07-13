use std::path::{Path, PathBuf};

use gaman_core::{BoxFuture, EmbeddedMigrations, MigrationStore, StoreError};

use super::directory::DirectoryMigrationStore;
use crate::runner_factory::NativeRunnerError;
/// Compiled migration storage that writes newly generated files to the source directory.
#[derive(Clone)]
pub struct EmbeddedMigrationStore {
    migrations: &'static EmbeddedMigrations,
    writer: DirectoryMigrationStore,
}

impl EmbeddedMigrationStore {
    /// Creates compiled migration storage with a directory writer for new migrations.
    pub fn new(migrations: &'static EmbeddedMigrations, writer: DirectoryMigrationStore) -> Self {
        Self { migrations, writer }
    }
}

impl MigrationStore for EmbeddedMigrationStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<gaman_core::Migration>, StoreError>> {
        Box::pin(async move { collect_embedded_migrations(self.migrations) })
    }

    fn save<'a>(
        &'a self,
        migration: &'a gaman_core::Migration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        self.writer.save(migration)
    }
}

/// Loads a complete embedded migration tree and rejects duplicate qualified IDs.
fn collect_embedded_migrations(
    root: &'static EmbeddedMigrations,
) -> Result<Vec<gaman_core::Migration>, StoreError> {
    let mut migrations = Vec::new();
    let mut ids = std::collections::HashSet::new();
    collect_embedded_node(root, None, &mut migrations, &mut ids)?;
    Ok(migrations)
}

/// Recursively qualifies one embedded migration namespace and its dependencies.
fn collect_embedded_node(
    node: &'static EmbeddedMigrations,
    namespace: Option<&str>,
    migrations: &mut Vec<gaman_core::Migration>,
    ids: &mut std::collections::HashSet<String>,
) -> Result<(), StoreError> {
    for (name, content) in node.files {
        let local_id = Path::new(name)
            .file_stem()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                StoreError::unavailable(format!("invalid embedded migration filename '{name}'"))
            })?;
        let mut migration = gaman_core::Migration::from_yaml_str(content).map_err(|error| {
            StoreError::unavailable(format!("failed to parse embedded '{name}': {error}"))
        })?;
        migration.id = qualify_embedded_id(namespace, local_id);
        migration.dependencies = migration
            .dependencies
            .into_iter()
            .map(|dependency| qualify_embedded_dependency(namespace, dependency))
            .collect();
        if !ids.insert(migration.id.clone()) {
            return Err(StoreError::unavailable(format!(
                "duplicate embedded migration id '{}'",
                migration.id
            )));
        }
        migrations.push(migration);
    }
    for (child, child_node) in node.children {
        let child_namespace = namespace
            .map(|parent| format!("{parent}/{child}"))
            .unwrap_or_else(|| (*child).to_string());
        collect_embedded_node(child_node, Some(&child_namespace), migrations, ids)?;
    }
    Ok(())
}

fn qualify_embedded_id(namespace: Option<&str>, id: &str) -> String {
    namespace
        .map(|namespace| format!("{namespace}/{id}"))
        .unwrap_or_else(|| id.to_string())
}

fn qualify_embedded_dependency(namespace: Option<&str>, dependency: String) -> String {
    if dependency.contains('/') {
        dependency
    } else {
        qualify_embedded_id(namespace, &dependency)
    }
}

/// Ensures generated files are written back to the directory used at compilation time.
pub(crate) fn validate_embedded_directory(
    configured: &Path,
    embedded: &str,
) -> Result<(), NativeRunnerError> {
    let current = std::env::current_dir()
        .map_err(|error| NativeRunnerError::CurrentDirectory(error.to_string()))?;
    let configured = normalize_path(&current, configured);
    let embedded_path = normalize_path(&current, Path::new(embedded));
    if configured == embedded_path {
        Ok(())
    } else {
        Err(NativeRunnerError::EmbeddedDirectoryMismatch {
            configured: configured.display().to_string(),
            embedded: embedded_path.display().to_string(),
        })
    }
}

fn normalize_path(current: &Path, path: &Path) -> PathBuf {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        current.join(path)
    };
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                normalized.pop();
            }
            component => normalized.push(component.as_os_str()),
        }
    }
    normalized
}
