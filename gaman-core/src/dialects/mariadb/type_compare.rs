//! Token-based MariaDB native type comparison.

/// Produces a MariaDB comparison key without borrowing MySQL JSON semantics.
pub(in crate::dialects) fn key(value: &str) -> String {
    super::super::mysql::type_compare::token_key(value, crate::dialects::Dialect::Mariadb, false)
}

/// Preserves authored MariaDB type spelling after trimming outer whitespace.
pub(super) fn canonical(value: &str) -> String {
    value.trim().to_string()
}
