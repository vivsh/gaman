//! PostgreSQL pseudo-types valid only in function signatures or implementation APIs.
//!
//! These are deliberately separate from `data_types`: Gaman must not treat a
//! function-only pseudo-type as a recognized column declaration.

const PSEUDO_TYPES: &[&str] = &[
    "any",
    "anyarray",
    "anycompatible",
    "anycompatiblearray",
    "anycompatiblemultirange",
    "anycompatiblenonarray",
    "anycompatiblerange",
    "anyelement",
    "anyenum",
    "anymultirange",
    "anynonarray",
    "anyrange",
    "cstring",
    "event_trigger",
    "fdw_handler",
    "index_am_handler",
    "internal",
    "language_handler",
    "pg_ddl_command",
    "pg_node_tree",
    "record",
    "refcursor",
    "table_am_handler",
    "trigger",
    "tsm_handler",
    "unknown",
    "void",
];

/// Returns pseudo-types excluded from column-type recognition.
pub fn pseudo_types() -> &'static [&'static str] {
    PSEUDO_TYPES
}
