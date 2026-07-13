use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use gaman_core::{BoxFuture, MigrationStore, StoreError};
/// Async filesystem-backed storage for canonical migration YAML.
#[derive(Clone)]
pub struct DirectoryMigrationStore {
    directory: PathBuf,
}

impl DirectoryMigrationStore {
    /// Creates a migration store rooted at one native filesystem directory.
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// Returns the backing migration directory.
    pub fn directory(&self) -> &Path {
        &self.directory
    }
}

impl MigrationStore for DirectoryMigrationStore {
    fn load_all<'a>(&'a self) -> BoxFuture<'a, Result<Vec<gaman_core::Migration>, StoreError>> {
        Box::pin(async move {
            if !tokio::fs::try_exists(&self.directory)
                .await
                .map_err(store_io(&self.directory))?
            {
                return Ok(Vec::new());
            }
            let mut entries = tokio::fs::read_dir(&self.directory)
                .await
                .map_err(store_io(&self.directory))?;
            let mut paths = Vec::new();
            while let Some(entry) = entries
                .next_entry()
                .await
                .map_err(store_io(&self.directory))?
            {
                let path = entry.path();
                if path.extension().and_then(|extension| extension.to_str()) == Some("yaml") {
                    let file_type = entry.file_type().await.map_err(store_io(&path))?;
                    if file_type.is_symlink() || !file_type.is_file() {
                        return Err(StoreError::InvalidMigration {
                            message: format!(
                                "migration entry '{}' is not a regular file",
                                store_path_label(&path)
                            ),
                        });
                    }
                    paths.push(path);
                }
            }
            paths.sort();
            let mut migrations = Vec::with_capacity(paths.len());
            for path in paths {
                let content = tokio::fs::read_to_string(&path)
                    .await
                    .map_err(store_io(&path))?;
                let mut migration =
                    gaman_core::Migration::from_yaml_str(&content).map_err(|error| {
                        StoreError::unavailable(format!(
                            "failed to parse '{}': {error}",
                            path.display()
                        ))
                    })?;
                migration.id = path
                    .file_stem()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| {
                        StoreError::unavailable(format!(
                            "invalid migration filename '{}",
                            path.display()
                        ))
                    })?
                    .to_string();
                migrations.push(migration);
            }
            Ok(migrations)
        })
    }

    fn save<'a>(
        &'a self,
        migration: &'a gaman_core::Migration,
    ) -> BoxFuture<'a, Result<(), StoreError>> {
        Box::pin(async move {
            tokio::fs::create_dir_all(&self.directory)
                .await
                .map_err(store_io(&self.directory))?;
            let path = self.directory.join(format!("{}.yaml", migration.id));
            if tokio::fs::try_exists(&path)
                .await
                .map_err(store_io(&path))?
            {
                return Err(StoreError::unavailable(format!(
                    "migration already exists: {}",
                    path.display()
                )));
            }
            let content = migration
                .to_yaml_string()
                .map_err(|error| StoreError::Save {
                    id: migration.id.clone(),
                    message: error.to_string(),
                })?;
            let directory = self.directory.clone();
            let id = migration.id.clone();
            tokio::task::spawn_blocking(move || durable_save(&directory, &id, content))
                .await
                .map_err(|error| StoreError::Save {
                    id: migration.id.clone(),
                    message: format!("migration write task failed: {error}"),
                })?
        })
    }
}

/// Persists one migration with atomic visibility, no overwrite, and durable metadata.
fn durable_save(directory: &Path, id: &str, content: String) -> Result<(), StoreError> {
    validate_migration_id(id)?;
    let destination = directory.join(format!("{id}.yaml"));
    let mut temporary = tempfile::Builder::new()
        .prefix(".gaman-")
        .suffix(".tmp")
        .tempfile_in(directory)
        .map_err(|error| save_error(id, error))?;
    temporary
        .write_all(content.as_bytes())
        .and_then(|_| temporary.flush())
        .and_then(|_| temporary.as_file().sync_all())
        .map_err(|error| save_error(id, error))?;
    temporary
        .persist_noclobber(&destination)
        .map_err(|error| match error.error.kind() {
            ErrorKind::AlreadyExists => StoreError::Conflict { id: id.to_string() },
            _ => save_error(id, error.error),
        })?;
    sync_directory(directory).map_err(|error| save_error(id, error))
}

/// Rejects identities that could escape or create nested paths in a migration store.
fn validate_migration_id(id: &str) -> Result<(), StoreError> {
    let mut components = Path::new(id).components();
    let valid = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if valid && !id.is_empty() {
        Ok(())
    } else {
        Err(StoreError::Save {
            id: id.to_string(),
            message: "migration id must be a single filesystem-safe component".to_string(),
        })
    }
}

/// Synchronizes a migration directory so a completed rename survives a system failure.
fn sync_directory(directory: &Path) -> std::io::Result<()> {
    std::fs::File::open(directory)?.sync_all()
}

fn save_error(id: &str, error: std::io::Error) -> StoreError {
    StoreError::Save {
        id: id.to_string(),
        message: error.to_string(),
    }
}

/// Converts one filesystem error into the host-neutral migration-store error.
fn store_io(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |error| {
        StoreError::unavailable(format!(
            "filesystem error at '{}': {error}",
            store_path_label(path)
        ))
    }
}

fn store_path_label(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("migration store")
        .to_string()
}
