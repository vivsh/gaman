//! PostgreSQL's stable, user-declarable native type catalog.
//!
//! This catalog intentionally excludes pseudo-types and automatically generated
//! catalog row/array types. Unknown user-defined, domain, composite, and
//! extension types remain valid through Gaman's TOFU flow.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeFamily {
    Bit,
    Boolean,
    Binary,
    Character,
    DateTime,
    Geometric,
    Identifier,
    Integer,
    Json,
    Money,
    Network,
    Numeric,
    Range,
    TextSearch,
    Uuid,
    Xml,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModifierPolicy {
    None,
    Parenthesized,
    Interval,
    Time,
    Timestamp,
}

#[derive(Debug, Clone, Copy)]
pub struct NativeTypeSpec {
    pub canonical: &'static str,
    pub aliases: &'static [&'static str],
    #[allow(dead_code)]
    pub family: TypeFamily,
    pub modifiers: ModifierPolicy,
    #[allow(dead_code)]
    pub minimum_postgres: u16,
}

const NATIVE_TYPES: &[NativeTypeSpec] = &[
    NativeTypeSpec {
        canonical: "aclitem",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "bigint",
        aliases: &["int8"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "bigserial",
        aliases: &["serial8"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "bit",
        aliases: &[],
        family: TypeFamily::Bit,
        modifiers: ModifierPolicy::Parenthesized,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "bit varying",
        aliases: &["varbit"],
        family: TypeFamily::Bit,
        modifiers: ModifierPolicy::Parenthesized,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "boolean",
        aliases: &["bool"],
        family: TypeFamily::Boolean,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "box",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "bytea",
        aliases: &[],
        family: TypeFamily::Binary,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "char",
        aliases: &["character", "bpchar"],
        family: TypeFamily::Character,
        modifiers: ModifierPolicy::Parenthesized,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "cid",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "cidr",
        aliases: &[],
        family: TypeFamily::Network,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "circle",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "date",
        aliases: &[],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "datemultirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "daterange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "double precision",
        aliases: &["float", "float8"],
        family: TypeFamily::Numeric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "inet",
        aliases: &[],
        family: TypeFamily::Network,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "int2vector",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "int4multirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "int4range",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "int8multirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "int8range",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "integer",
        aliases: &["int", "int4"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "interval",
        aliases: &[],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::Interval,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "json",
        aliases: &[],
        family: TypeFamily::Json,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "jsonb",
        aliases: &[],
        family: TypeFamily::Json,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "jsonpath",
        aliases: &[],
        family: TypeFamily::Json,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "line",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "lseg",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "macaddr",
        aliases: &[],
        family: TypeFamily::Network,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "macaddr8",
        aliases: &[],
        family: TypeFamily::Network,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "money",
        aliases: &[],
        family: TypeFamily::Money,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "name",
        aliases: &[],
        family: TypeFamily::Character,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "numeric",
        aliases: &["decimal"],
        family: TypeFamily::Numeric,
        modifiers: ModifierPolicy::Parenthesized,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "nummultirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "numrange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "oid",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "oidvector",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "path",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "pg_lsn",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "pg_snapshot",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "point",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "polygon",
        aliases: &[],
        family: TypeFamily::Geometric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "real",
        aliases: &["float4"],
        family: TypeFamily::Numeric,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regclass",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regcollation",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regconfig",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regdictionary",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regnamespace",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regoper",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regoperator",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regproc",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regprocedure",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regrole",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "regtype",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "serial",
        aliases: &["serial4"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "smallint",
        aliases: &["int2"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "smallserial",
        aliases: &["serial2"],
        family: TypeFamily::Integer,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "text",
        aliases: &[],
        family: TypeFamily::Character,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tid",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "time without time zone",
        aliases: &["time"],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::Time,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "time with time zone",
        aliases: &["timetz"],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::Time,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "timestamp without time zone",
        aliases: &["timestamp"],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::Timestamp,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "timestamp with time zone",
        aliases: &["timestamptz"],
        family: TypeFamily::DateTime,
        modifiers: ModifierPolicy::Timestamp,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tsmultirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tsquery",
        aliases: &[],
        family: TypeFamily::TextSearch,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tsrange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tsvector",
        aliases: &[],
        family: TypeFamily::TextSearch,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tstzmultirange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "tstzrange",
        aliases: &[],
        family: TypeFamily::Range,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "txid_snapshot",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "uuid",
        aliases: &[],
        family: TypeFamily::Uuid,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "varchar",
        aliases: &["character varying"],
        family: TypeFamily::Character,
        modifiers: ModifierPolicy::Parenthesized,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "xid",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "xid8",
        aliases: &[],
        family: TypeFamily::Identifier,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
    NativeTypeSpec {
        canonical: "xml",
        aliases: &[],
        family: TypeFamily::Xml,
        modifiers: ModifierPolicy::None,
        minimum_postgres: 14,
    },
];

#[cfg(test)]
pub fn native_type_specs() -> &'static [NativeTypeSpec] {
    NATIVE_TYPES
}

pub fn normalize_type(t: &str) -> &str {
    let normalized = normalize_type_text(t);
    if normalized == "int" || normalized == "int4" {
        "integer"
    } else if normalized == "int2" {
        "smallint"
    } else if normalized == "int8" {
        "bigint"
    } else if normalized == "bool" {
        "boolean"
    } else if normalized == "float4" {
        "real"
    } else if normalized == "float8" || normalized == "float" {
        "double precision"
    } else if normalized == "decimal" {
        "numeric"
    } else {
        t
    }
}

pub fn canonical_type(t: &str) -> String {
    canonical_known_type(t).unwrap_or_else(|| t.to_string())
}

pub fn canonical_known_type(t: &str) -> Option<String> {
    let normalized = normalize_type_text(t);
    let (base, array_suffix) = split_arrays(&normalized);
    let base = base.strip_prefix("pg_catalog.").unwrap_or(base);
    let canonical = canonical_base(base)?;
    Some(format!("{canonical}{array_suffix}"))
}

#[cfg(test)]
pub fn type_family(t: &str) -> Option<TypeFamily> {
    let normalized = normalize_type_text(t);
    let (base, _) = split_arrays(&normalized);
    let base = base.strip_prefix("pg_catalog.").unwrap_or(base);
    let (name, _) = split_known_prefix(base)?;
    NATIVE_TYPES
        .iter()
        .find(|spec| spec.canonical == name)
        .map(|spec| spec.family)
}

pub fn known_type_names() -> impl Iterator<Item = &'static str> {
    NATIVE_TYPES
        .iter()
        .flat_map(|spec| std::iter::once(spec.canonical).chain(spec.aliases.iter().copied()))
}

fn canonical_base(t: &str) -> Option<String> {
    if t == "timestamptz" {
        return Some("timestamp with time zone".to_string());
    }
    if t == "timetz" {
        return Some("time with time zone".to_string());
    }
    if let Some(value) = canonical_temporal(t, "timestamp") {
        return Some(value);
    }
    if let Some(value) = canonical_temporal(t, "time") {
        return Some(value);
    }
    if let Some(value) = canonical_float(t) {
        return Some(value);
    }

    let (canonical, suffix) = split_known_prefix(t)?;
    let spec = NATIVE_TYPES
        .iter()
        .find(|spec| spec.canonical == canonical)?;
    match spec.modifiers {
        ModifierPolicy::None if suffix.trim().is_empty() => Some(canonical.to_string()),
        ModifierPolicy::Parenthesized if suffix.trim().is_empty() || is_parenthesized(suffix) => {
            Some(format!("{canonical}{suffix}"))
        }
        ModifierPolicy::Interval if valid_interval_suffix(suffix) => {
            Some(format!("{canonical}{suffix}"))
        }
        _ => None,
    }
}

fn canonical_temporal(t: &str, base: &str) -> Option<String> {
    let remainder = t.strip_prefix(base)?;
    if !remainder.is_empty()
        && !remainder.starts_with('(')
        && !remainder.starts_with(' ')
        && base != "time"
    {
        return None;
    }
    let (precision, zone) = split_temporal_suffix(remainder)?;
    let zone = match (base, zone) {
        ("timestamp", None) => "without time zone",
        ("time", None) => "without time zone",
        (_, Some(zone)) => zone,
        _ => return None,
    };
    Some(format!("{base}{precision} {zone}"))
}

fn split_temporal_suffix(remainder: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = remainder.trim();
    let (precision, zone) = if trimmed.starts_with('(') {
        let end = trimmed.find(')')?;
        let precision = &trimmed[..=end];
        let zone = trimmed[end + 1..].trim();
        (precision, zone)
    } else {
        ("", trimmed)
    };
    let zone = match zone {
        "" => None,
        "with time zone" => Some("with time zone"),
        "without time zone" => Some("without time zone"),
        _ => return None,
    };
    Some((precision, zone))
}

fn canonical_float(t: &str) -> Option<String> {
    let remainder = t.strip_prefix("float")?;
    if remainder.is_empty() {
        return Some("double precision".to_string());
    }
    if !is_parenthesized(remainder) {
        return None;
    }
    let precision = remainder[1..remainder.len() - 1]
        .trim()
        .parse::<u8>()
        .ok()?;
    match precision {
        1..=24 => Some("real".to_string()),
        25..=53 => Some("double precision".to_string()),
        _ => None,
    }
}

fn split_known_prefix(t: &str) -> Option<(&'static str, &str)> {
    let mut candidates = NATIVE_TYPES
        .iter()
        .flat_map(|spec| {
            std::iter::once((spec.canonical, spec.canonical)).chain(
                spec.aliases
                    .iter()
                    .map(move |alias| (*alias, spec.canonical)),
            )
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| right.0.len().cmp(&left.0.len()));
    candidates.into_iter().find_map(|(name, canonical)| {
        (t == name || t.strip_prefix(name).is_some_and(valid_suffix))
            .then(|| (canonical, &t[name.len()..]))
    })
}

fn valid_suffix(suffix: &str) -> bool {
    suffix.starts_with('(') || suffix.starts_with(' ')
}

fn is_parenthesized(suffix: &str) -> bool {
    suffix.starts_with('(') && suffix.ends_with(')')
}

fn valid_interval_suffix(suffix: &str) -> bool {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return true;
    }

    let (fields, precision) = suffix
        .strip_suffix(')')
        .and_then(|value| value.rsplit_once('('))
        .map_or((suffix, None), |(fields, precision)| {
            (fields.trim(), Some(precision.trim()))
        });
    if precision.is_some_and(|value| value.parse::<u8>().is_err()) {
        return false;
    }

    fields.is_empty()
        || matches!(
            fields,
            "year"
                | "month"
                | "day"
                | "hour"
                | "minute"
                | "second"
                | "year to month"
                | "day to hour"
                | "day to minute"
                | "day to second"
                | "hour to minute"
                | "hour to second"
                | "minute to second"
        )
}

fn split_arrays(t: &str) -> (&str, String) {
    let mut base = t;
    let mut suffix = String::new();
    while let Some(prefix) = base.strip_suffix("[]") {
        base = prefix.trim_end();
        suffix.push_str("[]");
    }
    (base, suffix)
}

pub(crate) fn normalize_type_text(t: &str) -> String {
    t.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}
