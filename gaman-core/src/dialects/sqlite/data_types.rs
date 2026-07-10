//! SQLite declared-type recognition and affinity rules.
//!
//! SQLite accepts arbitrary declared type names. This module preserves those
//! declarations while deriving the documented affinity used for comparison and
//! cast safety. Its known-name list exists only for suggestions and TOFU UX.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Affinity {
    Blob,
    Integer,
    Numeric,
    Real,
    Text,
}

const KNOWN_DECLARED_TYPES: &[&str] = &[
    "bigint",
    "blob",
    "bool",
    "boolean",
    "char",
    "character",
    "character varying",
    "clob",
    "date",
    "datetime",
    "decimal",
    "double",
    "double precision",
    "float",
    "int",
    "int2",
    "int4",
    "int8",
    "integer",
    "mediumint",
    "native character",
    "nchar",
    "numeric",
    "nvarchar",
    "real",
    "smallint",
    "text",
    "tinyint",
    "unsigned big int",
    "varchar",
    "varying character",
];

pub fn canonical_type(t: &str) -> String {
    t.to_string()
}

pub fn normalize_type(t: &str) -> &str {
    t
}

pub fn affinity(t: &str) -> Affinity {
    let normalized = normalize_type_text(t);
    if normalized.is_empty() || normalized.contains("blob") {
        Affinity::Blob
    } else if normalized.contains("int") {
        Affinity::Integer
    } else if normalized.contains("char")
        || normalized.contains("clob")
        || normalized.contains("text")
    {
        Affinity::Text
    } else if normalized.contains("real")
        || normalized.contains("floa")
        || normalized.contains("doub")
    {
        Affinity::Real
    } else {
        Affinity::Numeric
    }
}

pub fn affinity_key(t: &str) -> String {
    match affinity(t) {
        Affinity::Blob => "blob",
        Affinity::Integer => "integer",
        Affinity::Numeric => "numeric",
        Affinity::Real => "real",
        Affinity::Text => "text",
    }
    .to_string()
}

pub fn canonical_known_type(t: &str) -> Option<String> {
    let normalized = declared_type_base(t);
    KNOWN_DECLARED_TYPES
        .binary_search(&normalized.as_str())
        .is_ok()
        .then(|| t.to_string())
}

pub fn known_type_names() -> impl Iterator<Item = &'static str> {
    KNOWN_DECLARED_TYPES.iter().copied()
}

pub fn strict_type_allowed(t: &str) -> bool {
    matches!(
        normalize_type_text(t).as_str(),
        "int" | "integer" | "real" | "text" | "blob" | "any"
    )
}

pub(crate) fn normalize_type_text(t: &str) -> String {
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

fn declared_type_base(t: &str) -> String {
    let normalized = normalize_type_text(t);
    normalized
        .split_once('(')
        .map_or(normalized.as_str(), |(base, _)| base)
        .trim()
        .to_string()
}
