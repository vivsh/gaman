use std::path::{Path, PathBuf};

use gaman_core::dialects::{Dialect, DialectParseError};
use thiserror::Error;

#[derive(Clone, Copy, Default)]
pub enum TlsMode {
    #[default]
    NoTls,
}

/// Runtime configuration for gaman.
/// Loaded once at startup and passed through the call chain.
#[derive(Clone)]
pub struct Config {
    pub database_url: String,
    pub migrations_dir: PathBuf,
    pub schema_file: PathBuf,
    pub tls: TlsMode,
    pub dialect: Dialect,
}

/// Errors returned when runtime configuration is internally inconsistent.
#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("database_url must not be empty")]
    EmptyDatabaseUrl,
    #[error("invalid database_url: {0}")]
    InvalidDatabaseUrl(#[from] DialectParseError),
    #[error(
        "configured dialect {configured:?} does not match database_url dialect {inferred:?} for '{database_url}'"
    )]
    DialectMismatch {
        database_url: String,
        configured: Dialect,
        inferred: Dialect,
    },
    #[error("migrations_dir exists but is not a directory: {0}")]
    MigrationsDirNotDirectory(String),
    #[error("migrations_dir exists but is not writable: {0}")]
    MigrationsDirNotWritable(String),
    #[error("migrations_dir parent does not exist: {0}")]
    MigrationsDirParentMissing(String),
    #[error("migrations_dir parent is not writable: {0}")]
    MigrationsDirParentNotWritable(String),
    #[error("schema path exists but is neither a file nor a directory: {0}")]
    SchemaPathInvalid(String),
}

impl Config {
    /// Infers the dialect from a database URL using the shared core dialect parser.
    pub fn dialect_from_database_url(database_url: &str) -> Result<Dialect, DialectParseError> {
        Dialect::parse_from_url(database_url)
    }

    /// Loads configuration from environment variables and validates the database URL dialect.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("DATABASE_URL").unwrap_or("postgres:///".to_string());
        let dialect = Dialect::parse_from_url(&database_url)?;
        let config = Self {
            database_url,
            migrations_dir: std::env::var("MIGRATIONS_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("migrations")),
            schema_file: std::env::var("SCHEMA")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("schema.yaml")),
            tls: TlsMode::NoTls,
            dialect,
        };
        config.validate()?;
        Ok(config)
    }

    pub fn new(
        database_url: String,
        migrations_dir: PathBuf,
        schema_file: PathBuf,
        dialect: Dialect,
    ) -> Self {
        Self {
            database_url,
            migrations_dir,
            schema_file,
            tls: TlsMode::NoTls,
            dialect,
        }
    }

    /// Validates that configuration fields are usable and mutually consistent.
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.database_url.trim().is_empty() {
            return Err(ConfigError::EmptyDatabaseUrl);
        }

        let inferred = Dialect::parse_from_url(&self.database_url)?;
        if inferred != self.dialect {
            return Err(ConfigError::DialectMismatch {
                database_url: self.database_url.clone(),
                configured: self.dialect,
                inferred,
            });
        }

        validate_migrations_dir(&self.migrations_dir)?;
        validate_schema_file(&self.schema_file)?;
        Ok(())
    }

    pub fn with_dialect(self, dialect: Dialect) -> Self {
        Self { dialect, ..self }
    }
}

fn validate_migrations_dir(path: &Path) -> Result<(), ConfigError> {
    if path.exists() {
        if !path.is_dir() {
            return Err(ConfigError::MigrationsDirNotDirectory(
                path.display().to_string(),
            ));
        }
        if !is_writable_path(path) {
            return Err(ConfigError::MigrationsDirNotWritable(
                path.display().to_string(),
            ));
        }
        return Ok(());
    }

    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.exists() {
        return Err(ConfigError::MigrationsDirParentMissing(
            parent.display().to_string(),
        ));
    }
    if !is_writable_path(parent) {
        return Err(ConfigError::MigrationsDirParentNotWritable(
            parent.display().to_string(),
        ));
    }
    Ok(())
}

fn is_writable_path(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| !metadata.permissions().readonly())
        .unwrap_or(false)
}

fn validate_schema_file(path: &Path) -> Result<(), ConfigError> {
    if path.exists() && !path.is_file() && !path.is_dir() {
        return Err(ConfigError::SchemaPathInvalid(path.display().to_string()));
    }
    Ok(())
}

impl Default for Config {
    fn default() -> Self {
        Self::from_env().unwrap_or_else(|_| Self {
            database_url: "postgres:///".to_string(),
            migrations_dir: PathBuf::from("migrations"),
            schema_file: PathBuf::from("schema.yaml"),
            tls: TlsMode::NoTls,
            dialect: Dialect::Postgres,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError};
    use gaman_core::dialects::Dialect;

    fn config_with_url(database_url: &str) -> Config {
        let dialect = Config::dialect_from_database_url(database_url)
            .expect("Failed to read dialect from database url");
        Config::new(
            database_url.to_owned(),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
            dialect,
        )
    }

    use std::path::PathBuf;

    /// Verifies that `postgres://` URLs resolve to the PostgreSQL dialect.
    #[test]
    fn parses_postgres_dialect_from_database_url() {
        let config = config_with_url("postgres://localhost/app");

        assert!(matches!(config.dialect, Dialect::Postgres));
    }

    /// Verifies that `postgresql://` URLs resolve to the PostgreSQL dialect.
    #[test]
    fn parses_postgresql_dialect_from_database_url() {
        let config = config_with_url("postgresql://localhost/app");

        assert!(matches!(config.dialect, Dialect::Postgres));
    }

    /// Verifies that `sqlite://` URLs resolve to the SQLite dialect when the feature is enabled.
    #[cfg(feature = "sqlite")]
    #[test]
    fn parses_sqlite_dialect_from_database_url() {
        let config = config_with_url("sqlite://app.db");

        assert!(matches!(config.dialect, Dialect::Sqlite));
    }

    /// Verifies that SQLx-style in-memory SQLite URLs resolve to the SQLite dialect.
    #[cfg(feature = "sqlite")]
    #[test]
    fn parses_sqlite_memory_dialect_from_database_url() {
        let config = config_with_url("sqlite::memory:");

        assert!(matches!(config.dialect, Dialect::Sqlite));
    }

    /// Verifies valid URL-derived configuration passes validation.
    #[test]
    fn validate_accepts_matching_database_url_and_dialect() {
        let config = config_with_url("postgres://localhost/app");

        config.validate().unwrap();
    }

    /// Verifies validation rejects empty database URLs.
    #[test]
    fn validate_rejects_empty_database_url() {
        let config = Config::new(
            "".to_string(),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
            Dialect::Postgres,
        );

        assert!(matches!(
            config.validate(),
            Err(ConfigError::EmptyDatabaseUrl)
        ));
    }

    /// Verifies validation rejects URL and dialect mismatches.
    #[test]
    fn validate_rejects_mismatched_database_url_and_dialect() {
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
            Dialect::parse("sqlite").unwrap_or(Dialect::Postgres),
        );

        #[cfg(feature = "sqlite")]
        assert!(matches!(
            config.validate(),
            Err(ConfigError::DialectMismatch { .. })
        ));

        #[cfg(not(feature = "sqlite"))]
        config.validate().unwrap();
    }

    /// Verifies validation rejects a migrations path that is already a file.
    #[test]
    fn validate_rejects_file_migrations_dir() {
        let dir = tempfile::tempdir().unwrap();
        let migrations = dir.path().join("migrations.yaml");
        std::fs::write(&migrations, "").unwrap();
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            migrations,
            dir.path().join("schema.yaml"),
            Dialect::Postgres,
        );

        assert!(matches!(
            config.validate(),
            Err(ConfigError::MigrationsDirNotDirectory(_))
        ));
    }

    /// Verifies validation accepts a missing migrations directory with a writable parent.
    #[test]
    fn validate_accepts_missing_migrations_dir_with_writable_parent() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            dir.path().join("migrations"),
            dir.path().join("schema.yaml"),
            Dialect::Postgres,
        );

        config.validate().unwrap();
    }

    /// Verifies validation rejects missing migrations directories with missing parents.
    #[test]
    fn validate_rejects_missing_migrations_parent() {
        let dir = tempfile::tempdir().unwrap();
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            dir.path().join("missing").join("migrations"),
            dir.path().join("schema.yaml"),
            Dialect::Postgres,
        );

        assert!(matches!(
            config.validate(),
            Err(ConfigError::MigrationsDirParentMissing(_))
        ));
    }

    /// Verifies validation accepts a schema path that is already a directory.
    #[test]
    fn validate_accepts_directory_schema_path() {
        let dir = tempfile::tempdir().unwrap();
        let schema_dir = dir.path().join("schema.yaml");
        std::fs::create_dir(&schema_dir).unwrap();
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            dir.path().join("migrations"),
            schema_dir,
            Dialect::Postgres,
        );

        config.validate().unwrap();
    }
}
