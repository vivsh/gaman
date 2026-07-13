use thiserror::Error;

use crate::drift::{DriftPropertyDoc, DriftRegistry};
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::parsers::tokens::SqlTokenizer;
use crate::states::types::EntityKind;
use crate::states::{Column, Schema, SchemaValidationError};

#[derive(Debug, Error)]
pub enum DialectError {
    #[error("unsupported operation '{0}': {1}")]
    Unsupported(String, String),
}

/// Error returned when a database URL cannot be mapped to a supported dialect.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DialectParseError {
    #[error("database URL is empty")]
    EmptyUrl,
    #[error("database URL '{0}' does not contain a dialect scheme")]
    MissingScheme(String),
    #[error("unsupported database URL dialect scheme '{0}'")]
    UnsupportedScheme(String),
}

/// Database dialect selection and dialect-specific behavior.
///
/// Type catalogs under each dialect are intentionally incomplete and frequently updated.
/// `data_types.rs` lists native built-in types, aliases, and canonicalization rules;
/// `extension_types.rs` lists popular extension or externally provided types. These
/// catalogs exist for deterministic canonicalization, typo suggestions, and helpful
/// prompts. They are not a claim that Gaman knows the full database type universe.
/// Unknown types must not be rejected solely because they are absent from these catalogs;
/// migration history and explicit user decisions are the project-local trust boundary.
/// New dialect implementations must keep native data types and extension/popular
/// external types in separate files following this layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    Postgres,
    Sqlite,
    Mysql,
    Mariadb,
}

/// Internal dialect behavior boundary.
///
/// Shared planning code asks a processor to canonicalize, validate, finalize,
/// and render schema changes. Dialect-specific SQL and schema quirks should
/// live behind this trait instead of leaking into shared code as variant
/// matches.
pub(crate) trait DialectProcessor: Sync {
    fn tokenizer(&self) -> &'static dyn SqlTokenizer;

    fn migration_to_sql(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<Vec<String>, DialectError>;

    fn finalize_diff_operations(
        &self,
        ops: Vec<Operation>,
        previous: &Schema,
        current: &Schema,
    ) -> Vec<Operation>;

    fn should_merge(&self, table_name: &str, op: &Operation) -> bool;

    fn canonicalize_schema_name(&self, object: EntityKind, schema: Option<&str>) -> Option<String>;

    /// Returns a fast equivalence-normalized view of a type name.
    ///
    /// The returned value is a borrowed `&str` into either an alias table entry or
    /// the original input. It should be used for type-compatibility checks and
    /// diff/noise reduction, not as a stable stored representation.
    fn normalize_type<'a>(&self, t: &'a str) -> &'a str;

    /// Returns the canonical, persistent representation for a type name.
    ///
    /// This allocates and rewrites known aliases/modifiers into the dialect's
    /// preferred catalog shape, while preserving unknown/extension types. Use this
    /// when persisting or normalizing schema state before comparison/validation.
    fn canonical_type(&self, t: &str) -> String;

    /// Returns the dialect's semantic comparison key for a declared type.
    ///
    /// This key never becomes authored schema state. PostgreSQL uses it to
    /// compare native aliases, while SQLite uses its declared-type affinity.
    fn type_comparison_key(&self, t: &str) -> String;

    fn is_catalog_type(&self, t: &str) -> bool;

    fn type_suggestions(&self, t: &str) -> Vec<String>;

    fn validate_schema(&self, schema: &Schema) -> Result<(), SchemaValidationError>;

    fn validate_migration(&self, migration: &Migration) -> Result<(), DialectError>;

    fn validate_migration_with_state(
        &self,
        migration: &Migration,
        start: &Schema,
    ) -> Result<(), DialectError>;

    fn drift_registry(&self) -> &'static DriftRegistry;

    fn normalize_inspected_schema(&self, schema: Schema) -> Result<Schema, SchemaValidationError>;

    /// Preserves dialect metadata required by full-definition column repair SQL.
    fn column_for_repair(&self, expected: &Column, _: &Column) -> Column {
        expected.clone()
    }

    /// Returns product-owned tracking installation SQL when generic schema rendering is unsuitable.
    fn tracking_install_sql(&self, _: &str) -> Option<Vec<String>> {
        None
    }

    /// Returns product-owned SQL for listing applied migration identifiers.
    fn tracking_list_sql(&self, _: &str) -> Option<String> {
        None
    }

    /// Returns product-owned SQL for recording one applied migration identifier.
    fn tracking_record_sql(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    /// Returns product-owned SQL for removing one applied migration identifier.
    fn tracking_unrecord_sql(&self, _: &str, _: &str) -> Option<String> {
        None
    }

    /// Whether schema DDL can participate in a transaction spanning multiple statements.
    fn supports_transactional_ddl(&self) -> bool {
        true
    }

    fn default_expressions_equal(&self, left: &str, right: &str) -> bool {
        crate::parsers::tokens::expressions_equal(self.tokenizer(), left, right)
    }
}

impl Dialect {
    /// Returns the stable lowercase name for this dialect.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Postgres => "postgres",
            Self::Sqlite => "sqlite",
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }

    /// Parses a dialect name such as `postgres`, `postgresql`, `sqlite`, `sqlite3`, `mysql`, or `mariadb`.
    pub fn parse(value: &str) -> Option<Self> {
        if value.eq_ignore_ascii_case("postgres") || value.eq_ignore_ascii_case("postgresql") {
            Some(Self::Postgres)
        } else if value.eq_ignore_ascii_case("sqlite") || value.eq_ignore_ascii_case("sqlite3") {
            Some(Self::Sqlite)
        } else if value.eq_ignore_ascii_case("mysql") {
            Some(Self::Mysql)
        } else if value.eq_ignore_ascii_case("mariadb") {
            Some(Self::Mariadb)
        } else {
            None
        }
    }

    /// Infers the dialect from a database URL scheme.
    ///
    /// This accepts normal URL forms such as `postgres://...`, `postgresql://...`,
    /// `sqlite://...`, `mysql://...`, and `mariadb://...`, plus SQLx's SQLite
    /// shorthand such as `sqlite::memory:`.
    pub fn parse_from_url(database_url: &str) -> Result<Self, DialectParseError> {
        let value = database_url.trim();
        if value.is_empty() {
            return Err(DialectParseError::EmptyUrl);
        }

        let scheme = if value.starts_with("sqlite:") {
            "sqlite"
        } else if let Some((scheme, _)) = value.split_once("://") {
            scheme
        } else if let Some((scheme, _)) = value.split_once(':') {
            scheme
        } else {
            return Err(DialectParseError::MissingScheme(value.to_string()));
        };

        Self::parse(scheme).ok_or_else(|| DialectParseError::UnsupportedScheme(scheme.to_string()))
    }

    pub(crate) fn processor(&self) -> &'static dyn DialectProcessor {
        match self {
            Dialect::Postgres => &postgres::POSTGRES,
            Dialect::Sqlite => &sqlite::SQLITE,
            Dialect::Mysql => &mysql::MYSQL,
            Dialect::Mariadb => &mariadb::MARIADB,
        }
    }

    pub(crate) fn tokenizer(&self) -> &'static dyn SqlTokenizer {
        self.processor().tokenizer()
    }

    pub(crate) fn default_expressions_equal(&self, left: &str, right: &str) -> bool {
        self.processor().default_expressions_equal(left, right)
    }

    pub(crate) fn column_for_repair(&self, expected: &Column, observed: &Column) -> Column {
        self.processor().column_for_repair(expected, observed)
    }

    #[doc(hidden)]
    pub fn migration_to_sql(
        &self,
        migration: &Migration,
        _start: &Schema,
    ) -> Result<Vec<String>, DialectError> {
        self.processor().migration_to_sql(migration, _start)
    }

    /// Reorders operations to satisfy database-specific execution constraints.
    /// The default is a no-op — only databases with ordering requirements need to override.
    /// Called once per migration after diffing, before SQL generation or writing.
    pub fn reorder(
        &self,
        ops: Vec<Operation>,
        previous: &Schema,
        current: &Schema,
    ) -> Vec<Operation> {
        self.processor()
            .finalize_diff_operations(ops, previous, current)
    }

    /// Whether a decomposed sub-entity op should be folded back into its
    /// parent CreateTable. Postgres can inline everything; other dialects
    /// (e.g. SQLite) may need FKs kept inline while indexes stay separate.
    pub fn should_merge(&self, _table_name: &str, _op: &Operation) -> bool {
        self.processor().should_merge(_table_name, _op)
    }

    #[doc(hidden)]
    pub fn canonicalize_schema_name(
        &self,
        object: EntityKind,
        schema: Option<&str>,
    ) -> Option<String> {
        self.processor().canonicalize_schema_name(object, schema)
    }

    pub fn normalize_type<'a>(&self, t: &'a str) -> &'a str {
        self.processor().normalize_type(t)
    }

    pub fn canonical_type(&self, t: &str) -> String {
        self.processor().canonical_type(t)
    }

    #[doc(hidden)]
    pub fn type_comparison_key(&self, t: &str) -> String {
        self.processor().type_comparison_key(t)
    }

    pub fn is_catalog_type(&self, t: &str) -> bool {
        self.processor().is_catalog_type(t)
    }

    pub fn type_suggestions(&self, t: &str) -> Vec<String> {
        self.processor().type_suggestions(t)
    }

    pub fn validate_schema(&self, schema: &Schema) -> Result<(), SchemaValidationError> {
        self.processor().validate_schema(schema)
    }

    /// Normalizes a live inspected schema before semantic drift comparison.
    pub fn normalize_inspected_schema(
        &self,
        schema: Schema,
    ) -> Result<Schema, SchemaValidationError> {
        self.processor().normalize_inspected_schema(schema)
    }

    /// Returns the documented semantic drift properties for this dialect.
    pub fn drift_contract(&self) -> &'static [DriftPropertyDoc] {
        crate::drift::contract_for(*self)
    }

    /// Returns the comparator registry that defines semantic drift for this dialect.
    pub(crate) fn drift_registry(&self) -> &'static DriftRegistry {
        self.processor().drift_registry()
    }

    pub fn validate_migration(&self, m: &Migration) -> Result<(), DialectError> {
        self.processor().validate_migration(m)
    }

    /// Reports whether multi-statement schema migrations are transactionally atomic.
    pub fn supports_transactional_ddl(&self) -> bool {
        self.processor().supports_transactional_ddl()
    }

    /// Returns dialect-owned tracking installation SQL when required by the backend.
    pub fn tracking_install_sql(&self, table: &str) -> Option<Vec<String>> {
        self.processor().tracking_install_sql(table)
    }

    /// Returns dialect-owned SQL for listing applied migration identifiers.
    pub fn tracking_list_sql(&self, table: &str) -> Option<String> {
        self.processor().tracking_list_sql(table)
    }

    /// Returns dialect-owned SQL for recording one applied migration identifier.
    pub fn tracking_record_sql(&self, table: &str, id: &str) -> Option<String> {
        self.processor().tracking_record_sql(table, id)
    }

    /// Returns dialect-owned SQL for removing one applied migration identifier.
    pub fn tracking_unrecord_sql(&self, table: &str, id: &str) -> Option<String> {
        self.processor().tracking_unrecord_sql(table, id)
    }

    #[doc(hidden)]
    pub fn validate_migration_with_state(
        &self,
        m: &Migration,
        _start: &Schema,
    ) -> Result<(), DialectError> {
        self.processor().validate_migration_with_state(m, _start)
    }
}

mod mariadb;
mod mysql;
mod mysql_family;
mod postgres;
mod sqlite;

#[cfg(test)]
mod tests {
    use super::{Dialect, DialectParseError};

    /// Verifies PostgreSQL URLs infer the PostgreSQL dialect.
    #[test]
    fn parse_from_url_accepts_postgres_urls() {
        assert_eq!(
            Dialect::parse_from_url("postgres://localhost/app"),
            Ok(Dialect::Postgres)
        );
        assert_eq!(
            Dialect::parse_from_url("postgresql://localhost/app"),
            Ok(Dialect::Postgres)
        );
    }

    /// Verifies SQLite URLs infer the SQLite dialect.
    #[test]
    fn parse_from_url_accepts_sqlite_urls() {
        assert_eq!(
            Dialect::parse_from_url("sqlite://app.db"),
            Ok(Dialect::Sqlite)
        );
        assert_eq!(
            Dialect::parse_from_url("sqlite::memory:"),
            Ok(Dialect::Sqlite)
        );
    }

    /// Verifies MySQL and MariaDB URLs resolve to separate dialect contracts.
    #[test]
    fn parse_from_url_accepts_mysql_urls() {
        assert_eq!(
            Dialect::parse_from_url("mysql://localhost/app"),
            Ok(Dialect::Mysql)
        );
        assert_eq!(
            Dialect::parse_from_url("mariadb://localhost/app"),
            Ok(Dialect::Mariadb)
        );
    }

    /// Verifies unsupported URL schemes return a structured error.
    #[test]
    fn parse_from_url_rejects_unsupported_schemes() {
        assert_eq!(
            Dialect::parse_from_url("oracle://localhost/app"),
            Err(DialectParseError::UnsupportedScheme("oracle".to_string()))
        );
    }

    /// Verifies URLs without schemes fail before selecting a dialect.
    #[test]
    fn parse_from_url_rejects_missing_schemes() {
        assert_eq!(
            Dialect::parse_from_url("localhost/app"),
            Err(DialectParseError::MissingScheme(
                "localhost/app".to_string()
            ))
        );
    }
}
