use std::fs;
use std::io::Write;
use std::sync::Arc;

use thiserror::Error;

use gaman_core::migrations::Migration;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("IO error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
}

/// Loads and saves migrations from a caller-defined storage backend.
///
/// Implementations may be file-backed, embedded, in-memory, database-backed, or
/// browser-buffer-backed. The engine treats this as its storage boundary and
/// does not require migrations to live on a filesystem. Sources must be
/// `Send + Sync` so async live migration futures can safely borrow the migrator
/// across runtime threads.
pub trait MigrationSource: Send + Sync {
    /// Return all migrations known to this source.
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError>;

    /// Persist a newly generated migration.
    fn save(&self, migration: &Migration) -> Result<(), AdapterError>;
}

impl<T: MigrationSource + ?Sized> MigrationSource for Arc<T> {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        (**self).load_all()
    }

    fn save(&self, migration: &Migration) -> Result<(), AdapterError> {
        (**self).save(migration)
    }
}

/// File-backed migration source.
/// Stores one `.yaml` file per migration and loads them in lexicographic order.
pub struct YamlAdapter {
    pub directory: std::path::PathBuf,
}

impl MigrationSource for YamlAdapter {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        if !self.directory.exists() {
            return Ok(vec![]);
        }

        let mut paths: Vec<_> = fs::read_dir(&self.directory)
            .map_err(|e| AdapterError::Io {
                path: self.directory.display().to_string(),
                message: e.to_string(),
            })?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();

        paths.sort();

        let mut migrations = Vec::with_capacity(paths.len());
        for path in paths {
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or_default()
                .to_string();
            let content = fs::read_to_string(&path).map_err(|e| AdapterError::Io {
                path: path.display().to_string(),
                message: e.to_string(),
            })?;
            let mut migration: Migration =
                serde_yaml::from_str(&content).map_err(|e| AdapterError::Parse {
                    path: path.display().to_string(),
                    message: e.to_string(),
                })?;
            migration.id = id;
            migrations.push(migration);
        }
        Ok(migrations)
    }

    fn save(&self, migration: &Migration) -> Result<(), AdapterError> {
        fs::create_dir_all(&self.directory).map_err(|e| AdapterError::Io {
            path: self.directory.display().to_string(),
            message: e.to_string(),
        })?;
        let path = self.directory.join(format!("{}.yaml", migration.id));
        if path.exists() {
            return Err(AdapterError::Io {
                path: path.display().to_string(),
                message: "migration file already exists".to_string(),
            });
        }
        let content = serde_yaml::to_string(migration).map_err(|e| AdapterError::Parse {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let tmp_path = self
            .directory
            .join(format!(".{}.{}.tmp", migration.id, std::process::id()));
        let write_result = (|| {
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&tmp_path)?;
            file.write_all(content.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp_path, &path)
        })();

        if let Err(e) = write_result {
            let _ = fs::remove_file(&tmp_path);
            return Err(AdapterError::Io {
                path: path.display().to_string(),
                message: e.to_string(),
            });
        }
        Ok(())
    }
}

/// In-memory migration source for tests and programmatic use.
pub struct VecAdapter {
    migrations: Vec<Migration>,
}

impl VecAdapter {
    /// Create a read-only in-memory migration source.
    pub fn new(migrations: Vec<Migration>) -> Self {
        Self { migrations }
    }
}

impl MigrationSource for VecAdapter {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        Ok(self.migrations.clone())
    }

    fn save(&self, _migration: &Migration) -> Result<(), AdapterError> {
        Err(AdapterError::Io {
            path: "<vec>".into(),
            message: "VecAdapter is read-only".into(),
        })
    }
}
