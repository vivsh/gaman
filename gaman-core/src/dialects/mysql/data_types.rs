//! MySQL 8.4 native type catalog used for validation and suggestions.

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
    "varbinary",
    "varchar",
    "year",
];

/// Reports whether the native type base is known to MySQL 8.4.
pub(super) fn contains(value: &str) -> bool {
    TYPES.contains(&base(value).as_str())
}

/// Returns deterministic type suggestions without defining a closed type universe.
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

fn base(value: &str) -> String {
    value
        .trim()
        .split(|ch: char| ch.is_whitespace() || ch == '(')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
}
