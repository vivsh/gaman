use crate::adapters::{AdapterError, MigrationSource};
use crate::migrations::Migration;

/// In-memory migration source backed by static content, typically produced by
/// `include_migrations!` from the `gaman-macros` crate.
pub struct EmbedSource {
    migrations: &'static [(&'static str, &'static str)],
}

impl EmbedSource {
    pub fn new(migrations: &'static [(&'static str, &'static str)]) -> Self {
        Self { migrations }
    }
}

impl MigrationSource for EmbedSource {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        self.migrations
            .iter()
            .map(|(id, content)| {
                let mut m: Migration =
                    serde_yaml::from_str(content).map_err(|e| AdapterError::Parse {
                        path: id.to_string(),
                        message: e.to_string(),
                    })?;
                m.id = id.to_string();
                Ok(m)
            })
            .collect()
    }

    fn save(&self, _migration: &Migration) -> Result<(), AdapterError> {
        Err(AdapterError::Io {
            path: "<embedded>".to_string(),
            message: "cannot save to an embedded migration source".to_string(),
        })
    }
}
