use std::path::PathBuf;

use gaman_core::dialects::Dialect;

#[derive(Clone, Copy, Default)]
pub enum TlsMode {
    #[default]
    NoTls,
}

/// Runtime configuration for gaman.
/// Loaded once at startup and passed through the call chain.
#[derive(Clone)]
pub struct Config {
    pub database_url: Option<String>,
    pub migrations_dir: PathBuf,
    pub schema_file: PathBuf,
    pub tls: TlsMode,
}

impl Config {
    pub fn new(
        database_url: Option<String>,
        migrations_dir: PathBuf,
        schema_file: PathBuf,
    ) -> Self {
        Self {
            database_url,
            migrations_dir,
            schema_file,
            tls: TlsMode::NoTls,
        }
    }

    pub fn dialect(&self) -> Option<Dialect> {
        let url = self.database_url.as_deref()?;
        let scheme = url
            .split_once("://")
            .map(|(scheme, _)| scheme)
            .or_else(|| url.split_once(':').map(|(scheme, _)| scheme))?;

        Dialect::parse(scheme)
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
            tls: TlsMode::NoTls,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Config;
    use gaman_core::dialects::Dialect;

    fn config_with_url(database_url: Option<&str>) -> Config {
        Config::new(
            database_url.map(str::to_owned),
            PathBuf::from("migrations"),
            PathBuf::from("schema.yaml"),
        )
    }

    use std::path::PathBuf;

    /// Verifies that `postgres://` URLs resolve to the PostgreSQL dialect.
    #[test]
    fn parses_postgres_dialect_from_database_url() {
        let config = config_with_url(Some("postgres://localhost/app"));

        assert!(matches!(config.dialect(), Some(Dialect::Postgres)));
    }

    /// Verifies that `postgresql://` URLs resolve to the PostgreSQL dialect.
    #[test]
    fn parses_postgresql_dialect_from_database_url() {
        let config = config_with_url(Some("postgresql://localhost/app"));

        assert!(matches!(config.dialect(), Some(Dialect::Postgres)));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn parses_sqlite_dialect_from_database_url() {
        let config = config_with_url(Some("sqlite://app.db"));

        assert!(matches!(config.dialect(), Some(Dialect::Sqlite)));
    }

    #[cfg(feature = "sqlite")]
    #[test]
    fn parses_sqlite_memory_dialect_from_database_url() {
        let config = config_with_url(Some("sqlite::memory:"));

        assert!(matches!(config.dialect(), Some(Dialect::Sqlite)));
    }

    /// Verifies that unsupported URL schemes do not resolve to a known dialect.
    #[test]
    fn returns_none_for_unsupported_database_url_scheme() {
        let config = config_with_url(Some("mysql://localhost/app"));

        assert!(config.dialect().is_none());
    }

    /// Verifies that a missing database URL leaves the dialect unspecified.
    #[test]
    fn returns_none_without_database_url() {
        let config = config_with_url(None);

        assert!(config.dialect().is_none());
    }
}
