use std::path::PathBuf;

/// Runtime configuration for gaman.
/// Loaded once at startup and passed through the call chain.
pub struct Config {
    pub database_url: Option<String>,
    pub migrations_dir: PathBuf,
    pub schema_file: PathBuf,
}

impl Config {
    pub fn new(database_url: Option<String>, migrations_dir: PathBuf, schema_file: PathBuf) -> Self {
        Self {
            database_url,
            migrations_dir,
            schema_file,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            database_url: std::env::var("DATABASE_URL").ok(),
            migrations_dir: std::env::var("MIGRATIONS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("migrations")),
            schema_file: std::env::var("SCHEMA_FILE")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("schema.yaml")),
        }
    }
}
