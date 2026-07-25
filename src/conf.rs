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
    #[error("DATABASE_URL is required")]
    MissingDatabaseUrl,
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
    #[error("{dialect} dialect is not available in this Gaman build: {reason}")]
    DialectUnavailable {
        dialect: &'static str,
        reason: &'static str,
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

    /// Loads configuration from environment variables and infers its dialect.
    ///
    /// Call [`Self::validate`] after applying any command-line path overrides.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_env_with_database_url(None)
    }

    /// Loads configuration from environment variables with an optional CLI URL override.
    pub fn from_env_with_database_url(database_url: Option<String>) -> Result<Self, ConfigError> {
        let database_url = database_url
            .or_else(|| std::env::var("DATABASE_URL").ok())
            .ok_or(ConfigError::MissingDatabaseUrl)?;
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
        Ok(config)
    }

    /// Creates configuration from explicit values.
    ///
    /// Call [`Self::validate`] before use when values may originate outside the application.
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

    /// Returns the configured database URL with a password, if present, redacted.
    pub fn redacted_database_url(&self) -> String {
        gaman_core::redact_diagnostic_text(&self.database_url)
    }

    /// Validates configuration for operations that may write migration files.
    pub fn validate(&self) -> Result<(), ConfigError> {
        self.validate_read_only()?;
        validate_migrations_dir_writable(&self.migrations_dir)
    }

    /// Validates configuration for operations that only read migration files.
    pub fn validate_read_only(&self) -> Result<(), ConfigError> {
        self.validate_runtime()?;
        validate_migrations_dir_readable(&self.migrations_dir)?;
        validate_schema_file(&self.schema_file)?;
        Ok(())
    }

    /// Validates configuration for live SQL schema preparation without migrations.
    ///
    /// `check_schema` connects to the configured dialect but does not read or
    /// write migration storage, so a missing migrations directory is valid.
    pub fn validate_schema_check(&self) -> Result<(), ConfigError> {
        self.validate_runtime()?;
        validate_schema_file(&self.schema_file)
    }

    /// Validates the database URL and selected live dialect shared by native commands.
    fn validate_runtime(&self) -> Result<(), ConfigError> {
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

        validate_dialect_available(self.dialect)?;
        Ok(())
    }
}

fn validate_dialect_available(dialect: Dialect) -> Result<(), ConfigError> {
    match dialect {
        #[cfg(feature = "postgres")]
        Dialect::Postgres => Ok(()),
        #[cfg(not(feature = "postgres"))]
        Dialect::Postgres => Err(ConfigError::DialectUnavailable {
            dialect: "postgres",
            reason: "rebuild with the 'postgres' feature",
        }),
        #[cfg(feature = "sqlite")]
        Dialect::Sqlite => Ok(()),
        #[cfg(not(feature = "sqlite"))]
        Dialect::Sqlite => Err(ConfigError::DialectUnavailable {
            dialect: "sqlite",
            reason: "rebuild with the 'sqlite' feature",
        }),
        #[cfg(not(feature = "mysql"))]
        Dialect::Mysql => Err(ConfigError::DialectUnavailable {
            dialect: "mysql",
            reason: "rebuild with the 'mysql' feature",
        }),
        #[cfg(feature = "mysql")]
        Dialect::Mysql => Ok(()),
        #[cfg(feature = "mariadb")]
        Dialect::Mariadb => Ok(()),
        #[cfg(not(feature = "mariadb"))]
        Dialect::Mariadb => Err(ConfigError::DialectUnavailable {
            dialect: "mariadb",
            reason: "rebuild with the 'mariadb' feature",
        }),
    }
}

fn validate_migrations_dir_readable(path: &Path) -> Result<(), ConfigError> {
    if path.exists() && !path.is_dir() {
        return Err(ConfigError::MigrationsDirNotDirectory(
            path.display().to_string(),
        ));
    }
    Ok(())
}

fn validate_migrations_dir_writable(path: &Path) -> Result<(), ConfigError> {
    validate_migrations_dir_readable(path)?;
    if path.exists() {
        return if is_writable_path(path) {
            Ok(())
        } else {
            Err(ConfigError::MigrationsDirNotWritable(
                path.display().to_string(),
            ))
        };
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

#[cfg(test)]
mod redaction_tests {
    use super::Config;
    use gaman_core::redact_diagnostic_text;

    /// Redacts passwords while retaining enough URL information for diagnostics.
    #[test]
    fn redacted_database_url_hides_password() {
        assert_eq!(
            redact_diagnostic_text("postgres://gaman:secret@localhost/app"),
            "postgres://gaman:***@localhost/app"
        );
    }

    /// Verifies authority-only URLs receive the same password protection as path-bearing URLs.
    #[test]
    fn redacted_database_url_hides_authority_only_password() {
        assert_eq!(
            redact_diagnostic_text("postgres://gaman:secret@localhost"),
            "postgres://gaman:***@localhost"
        );
    }

    /// Leaves URLs without user-info intact.
    #[test]
    fn redacted_database_url_preserves_url_without_credentials() {
        assert_eq!(
            Config::new(
                "sqlite::memory:".to_string(),
                "migrations".into(),
                "schema.yaml".into(),
                gaman_core::dialects::Dialect::Sqlite,
            )
            .redacted_database_url(),
            "sqlite::memory:"
        );
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

        assert!(matches!(
            config.validate(),
            Err(ConfigError::DialectMismatch { .. })
        ));
    }

    /// Verifies the native configuration accepts MySQL when its executor is enabled.
    #[cfg(feature = "mysql")]
    #[test]
    fn validate_accepts_enabled_mysql_dialect() {
        assert!(super::validate_dialect_available(Dialect::Mysql).is_ok());
    }

    /// Verifies the native configuration rejects MySQL when its executor is disabled.
    #[cfg(not(feature = "mysql"))]
    #[test]
    fn validate_rejects_disabled_mysql_dialect() {
        assert!(matches!(
            super::validate_dialect_available(Dialect::Mysql),
            Err(ConfigError::DialectUnavailable {
                dialect: "mysql",
                ..
            })
        ));
    }

    /// Verifies the native configuration accepts MariaDB when its executor is enabled.
    #[cfg(feature = "mariadb")]
    #[test]
    fn validate_accepts_enabled_mariadb_dialect() {
        assert!(super::validate_dialect_available(Dialect::Mariadb).is_ok());
    }

    /// Verifies the native configuration rejects MariaDB when its executor is disabled.
    #[cfg(not(feature = "mariadb"))]
    #[test]
    fn validate_rejects_disabled_mariadb_dialect() {
        assert!(matches!(
            super::validate_dialect_available(Dialect::Mariadb),
            Err(ConfigError::DialectUnavailable {
                dialect: "mariadb",
                ..
            })
        ));
    }

    /// Verifies read-only commands can consume migrations from a read-only directory.
    #[cfg(unix)]
    #[test]
    fn read_only_validation_does_not_require_writable_migrations() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let migrations = dir.path().join("migrations");
        std::fs::create_dir(&migrations).unwrap();
        std::fs::set_permissions(&migrations, std::fs::Permissions::from_mode(0o555)).unwrap();
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            migrations,
            dir.path().join("schema.yaml"),
            Dialect::Postgres,
        );

        config.validate_read_only().unwrap();
        assert!(matches!(
            config.validate(),
            Err(ConfigError::MigrationsDirNotWritable(_))
        ));
    }

    /// Verifies SQL schema checking does not require migration storage to exist.
    #[test]
    fn schema_check_validation_skips_migration_directory_validation() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config = Config::new(
            "postgres://localhost/app".to_string(),
            dir.path().join("missing-migrations"),
            dir.path().join("schema.sql"),
            Dialect::Postgres,
        );

        config
            .validate_schema_check()
            .expect("schema check should not require migrations");
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
