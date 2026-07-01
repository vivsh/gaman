const EXTENSION_TYPES: &[&str] = &[];

pub fn is_extension_type(t: &str) -> bool {
    let normalized = super::data_types::normalize_type_text(t);
    EXTENSION_TYPES.binary_search(&normalized.as_str()).is_ok()
}

pub fn extension_type_names() -> impl Iterator<Item = &'static str> {
    EXTENSION_TYPES.iter().copied()
}
