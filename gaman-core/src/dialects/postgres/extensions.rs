//! Curated PostgreSQL extension metadata for diagnostics and future UX.
//!
//! Extension names are never validation gates. PostgreSQL accepts extensions
//! outside this list through the normal opaque extension lifecycle.

#[derive(Debug, Clone, Copy)]
pub struct KnownExtension {
    pub name: &'static str,
    pub bundled: bool,
    pub description: &'static str,
}

const KNOWN_EXTENSIONS: &[KnownExtension] = &[
    KnownExtension {
        name: "citext",
        bundled: true,
        description: "case-insensitive text",
    },
    KnownExtension {
        name: "cube",
        bundled: true,
        description: "multidimensional cubes",
    },
    KnownExtension {
        name: "earthdistance",
        bundled: true,
        description: "earth-distance calculations",
    },
    KnownExtension {
        name: "hstore",
        bundled: true,
        description: "key-value pairs",
    },
    KnownExtension {
        name: "isn",
        bundled: true,
        description: "international product numbering",
    },
    KnownExtension {
        name: "lo",
        bundled: true,
        description: "large-object domains",
    },
    KnownExtension {
        name: "ltree",
        bundled: true,
        description: "hierarchical labels",
    },
    KnownExtension {
        name: "pgcrypto",
        bundled: true,
        description: "cryptographic functions and UUID generation",
    },
    KnownExtension {
        name: "pgvector",
        bundled: false,
        description: "vector similarity search",
    },
    KnownExtension {
        name: "postgis",
        bundled: false,
        description: "spatial and raster types",
    },
    KnownExtension {
        name: "seg",
        bundled: true,
        description: "line-segment types",
    },
];

pub fn known_extensions() -> &'static [KnownExtension] {
    KNOWN_EXTENSIONS
}

pub fn is_known_extension(name: &str) -> bool {
    KNOWN_EXTENSIONS
        .iter()
        .any(|extension| extension.name.eq_ignore_ascii_case(name.trim()))
}

pub fn extension_description(name: &str) -> Option<&'static str> {
    KNOWN_EXTENSIONS
        .iter()
        .find(|extension| extension.name.eq_ignore_ascii_case(name.trim()))
        .map(|extension| extension.description)
}
