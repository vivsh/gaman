use std::fs;

use thiserror::Error;

use crate::migrations::Migration;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("IO error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
}

/// Trait for loading and saving migrations.
/// Implementations are format- and storage-agnostic.
pub trait MigrationSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError>;
    fn save(&self, migration: &Migration) -> Result<(), AdapterError>;
}

/// YAML-backed migration source.
/// Reads and writes one `.yaml` file per migration in `directory`.
/// Files are loaded in lexicographic order — migration ids should be prefixed
/// with a sortable sequence number (e.g. `0001_`, `0002_`) to guarantee a
/// stable load order.
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
        let content = serde_yaml::to_string(migration).map_err(|e| AdapterError::Parse {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        fs::write(&path, content).map_err(|e| AdapterError::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        Ok(())
    }
}

/// In-memory migration source backed by a `Vec`.
/// No filesystem access. Useful for unit and integration testing.
pub struct VecAdapter {
    migrations: Vec<Migration>,
}

impl VecAdapter {
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
