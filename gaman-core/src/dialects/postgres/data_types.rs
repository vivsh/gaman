const TYPE_ALIASES: &[(&str, &str)] = &[
    ("int", "integer"),
    ("int2", "smallint"),
    ("int4", "integer"),
    ("int8", "bigint"),
    ("bool", "boolean"),
    ("float4", "real"),
    ("float8", "double precision"),
    ("bpchar", "char"),
    ("character", "char"),
    ("character varying", "varchar"),
    ("timestamp", "timestamp without time zone"),
    ("timestamptz", "timestamp with time zone"),
    ("decimal", "numeric"),
];

const BUILTIN_TYPES: &[&str] = &[
    "bigint",
    "bigserial",
    "bit",
    "bit varying",
    "boolean",
    "box",
    "bytea",
    "char",
    "cidr",
    "circle",
    "date",
    "double precision",
    "inet",
    "integer",
    "interval",
    "json",
    "jsonb",
    "line",
    "lseg",
    "macaddr",
    "macaddr8",
    "money",
    "numeric",
    "path",
    "point",
    "polygon",
    "real",
    "serial",
    "smallint",
    "smallserial",
    "text",
    "time with time zone",
    "time without time zone",
    "timestamp with time zone",
    "timestamp without time zone",
    "tsquery",
    "tsvector",
    "uuid",
    "varchar",
    "xml",
];

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
    if let Some((base, suffix)) = normalized.strip_suffix("[]").map(|base| (base, "[]")) {
        return canonical_known_type(base).map(|base| format!("{base}{suffix}"));
    }

    let (base, modifier) = split_type_modifier(&normalized);
    let canonical_base = TYPE_ALIASES
        .iter()
        .find_map(|(alias, canonical)| (*alias == base).then_some(*canonical))
        .unwrap_or(base);

    is_builtin_type_name(canonical_base).then(|| format!("{canonical_base}{modifier}"))
}

pub fn known_type_names() -> impl Iterator<Item = &'static str> {
    BUILTIN_TYPES
        .iter()
        .copied()
        .chain(TYPE_ALIASES.iter().map(|(alias, _)| *alias))
}

fn is_builtin_type_name(base: &str) -> bool {
    BUILTIN_TYPES.binary_search(&base).is_ok()
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
