const TYPE_ALIASES: &[(&str, &str)] = &[
    ("bigint", "integer"),
    ("bool", "integer"),
    ("boolean", "integer"),
    ("character varying", "text"),
    ("clob", "text"),
    ("double", "real"),
    ("double precision", "real"),
    ("float", "real"),
    ("int", "integer"),
    ("int2", "integer"),
    ("int4", "integer"),
    ("int8", "integer"),
    ("mediumint", "integer"),
    ("native character", "text"),
    ("nvarchar", "text"),
    ("smallint", "integer"),
    ("varchar", "text"),
    ("varying character", "text"),
];

const STORAGE_CLASSES: &[&str] = &["blob", "integer", "numeric", "real", "text"];

pub fn normalize_type(t: &str) -> &str {
    TYPE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == t).then_some(*canonical))
        .unwrap_or(t)
}

pub fn canonical_type(t: &str) -> String {
    canonical_known_type(t).unwrap_or_else(|| t.to_string())
}

pub fn canonical_known_type(t: &str) -> Option<String> {
    let normalized = normalize_type_text(t);
    let (base, _) = split_type_modifier(&normalized);
    let canonical_base = TYPE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == base).then_some(*canonical))
        .unwrap_or(base);

    STORAGE_CLASSES
        .binary_search(&canonical_base)
        .is_ok()
        .then(|| canonical_base.to_string())
}

pub fn known_type_names() -> impl Iterator<Item = &'static str> {
    STORAGE_CLASSES
        .iter()
        .copied()
        .chain(TYPE_ALIASES.iter().map(|(alias, _)| *alias))
}

fn split_type_modifier(t: &str) -> (&str, &str) {
    match t.find('(') {
        Some(start) if t.ends_with(')') => (&t[..start], &t[start..]),
        _ => (t, ""),
    }
}

pub(crate) fn normalize_type_text(t: &str) -> String {
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
