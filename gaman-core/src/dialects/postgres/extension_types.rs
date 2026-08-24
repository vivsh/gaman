//! Curated PostgreSQL extension-provided column types.
//!
//! This is UX metadata, not a complete extension registry and never a
//! validation gate. Unknown types continue through TOFU.

#[derive(Debug, Clone, Copy)]
pub struct ExtensionTypeCatalog {
    #[allow(dead_code)]
    pub extension: &'static str,
    pub types: &'static [&'static str],
}

const EXTENSION_TYPE_CATALOGS: &[ExtensionTypeCatalog] = &[
    ExtensionTypeCatalog {
        extension: "citext",
        types: &["citext"],
    },
    ExtensionTypeCatalog {
        extension: "cube",
        types: &["cube"],
    },
    ExtensionTypeCatalog {
        extension: "earthdistance",
        types: &["earth"],
    },
    ExtensionTypeCatalog {
        extension: "hstore",
        types: &["hstore"],
    },
    ExtensionTypeCatalog {
        extension: "isn",
        types: &[
            "ean13", "isbn", "isbn13", "ismn", "ismn13", "issn", "issn13", "upc",
        ],
    },
    ExtensionTypeCatalog {
        extension: "lo",
        types: &["lo"],
    },
    ExtensionTypeCatalog {
        extension: "ltree",
        types: &["lquery", "ltree", "ltxtquery"],
    },
    ExtensionTypeCatalog {
        extension: "vector",
        types: &["halfvec", "sparsevec", "vector"],
    },
    ExtensionTypeCatalog {
        extension: "postgis",
        types: &["box2d", "box3d", "geography", "geometry", "raster"],
    },
    ExtensionTypeCatalog {
        extension: "seg",
        types: &["seg"],
    },
];

#[cfg(test)]
pub fn extension_type_catalogs() -> &'static [ExtensionTypeCatalog] {
    EXTENSION_TYPE_CATALOGS
}

pub fn is_extension_type(t: &str) -> bool {
    let base = extension_type_base(t);
    EXTENSION_TYPE_CATALOGS
        .iter()
        .flat_map(|catalog| catalog.types)
        .any(|known| *known == base)
}

#[cfg(test)]
pub fn extension_for_type(t: &str) -> Option<&'static str> {
    let base = extension_type_base(t);
    EXTENSION_TYPE_CATALOGS
        .iter()
        .find(|catalog| catalog.types.iter().any(|known| *known == base))
        .map(|catalog| catalog.extension)
}

pub fn extension_type_names() -> impl Iterator<Item = &'static str> {
    EXTENSION_TYPE_CATALOGS
        .iter()
        .flat_map(|catalog| catalog.types.iter().copied())
}

fn extension_type_base(t: &str) -> String {
    let normalized = super::data_types::normalize_type_text(t);
    let mut base = normalized.trim();
    while let Some(prefix) = base.strip_suffix("[]") {
        base = prefix.trim_end();
    }
    let base = base.split_once('(').map_or(base, |(name, _)| name.trim());
    base.rsplit_once('.')
        .map_or(base, |(_, name)| name)
        .to_string()
}
