//! MariaDB 11.4 and 11.8 native type catalog.

pub(super) const TYPES: &[&str] = &[
    "bigint",
    "binary",
    "bit",
    "blob",
    "bool",
    "boolean",
    "char",
    "date",
    "datetime",
    "decimal",
    "double",
    "enum",
    "float",
    "geometry",
    "inet4",
    "inet6",
    "int",
    "integer",
    "json",
    "linestring",
    "longblob",
    "longtext",
    "mediumblob",
    "mediumint",
    "mediumtext",
    "multilinestring",
    "multipoint",
    "multipolygon",
    "numeric",
    "point",
    "polygon",
    "real",
    "serial",
    "set",
    "smallint",
    "text",
    "time",
    "timestamp",
    "tinyblob",
    "tinyint",
    "tinytext",
    "uuid",
    "varbinary",
    "varchar",
    "year",
];

/// Reports whether the native type base is known to supported MariaDB releases.
pub(super) fn contains(value: &str) -> bool {
    TYPES.contains(
        &value
            .trim()
            .split(|ch: char| ch.is_whitespace() || ch == '(')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
    )
}

/// Returns deterministic MariaDB-specific type suggestions.
pub(super) fn suggestions(value: &str) -> Vec<String> {
    let first = value
        .trim()
        .chars()
        .next()
        .map(|ch| ch.to_ascii_lowercase());
    TYPES
        .iter()
        .filter(|candidate| candidate.chars().next() == first)
        .take(8)
        .map(|value| (*value).to_string())
        .collect()
}
