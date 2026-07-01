const EXTENSION_TYPES: &[&str] = &[
    "citext",
    "cube",
    "earth",
    "geography",
    "geometry",
    "hstore",
    "ltree",
    "lquery",
    "ltxtquery",
    "sparsevec",
    "vector",
];

pub fn is_extension_type(t: &str) -> bool {
    let normalized = super::data_types::normalize_type_text(t);
    let base = normalized
        .strip_suffix("[]")
        .unwrap_or(&normalized)
        .split_once('(')
        .map(|(base, _)| base)
        .unwrap_or_else(|| normalized.as_str());
    EXTENSION_TYPES.binary_search(&base).is_ok()
}

pub fn extension_type_names() -> impl Iterator<Item = &'static str> {
    EXTENSION_TYPES.iter().copied()
}
