use super::*;

use serde::Deserialize;

#[derive(Deserialize)]
struct AffinityExample {
    declared: String,
    affinity: String,
}

#[derive(Deserialize)]
struct AffinityManifest {
    examples: Vec<AffinityExample>,
}

/// Verifies SQLite preserves declared types while deriving documented affinities.
#[test]
fn sqlite_type_catalog_preserves_declarations_and_suggests_typos() {
    assert_eq!(canonical_type("int4"), "int4");
    assert_eq!(canonical_type("varchar(100)"), "varchar(100)");
    assert_eq!(canonical_type("email_address"), "email_address");
    assert!(is_catalog_type("integer"));
    assert!(type_suggestions("inteer").contains(&"integer".to_string()));
}

/// Verifies SQLite affinity follows the documented ordered substring rules.
#[test]
fn sqlite_affinity_matches_documented_examples_and_precedence() {
    assert_eq!(data_types::affinity_key("UNSIGNED BIG INT"), "integer");
    assert_eq!(data_types::affinity_key("NCHAR(20)"), "text");
    assert_eq!(data_types::affinity_key("DOUBLE PRECISION"), "real");
    assert_eq!(data_types::affinity_key("DECIMAL(10,5)"), "numeric");
    assert_eq!(data_types::affinity_key("BOOLEAN"), "numeric");
    assert_eq!(data_types::affinity_key("DATE"), "numeric");
    assert_eq!(data_types::affinity_key("DATETIME"), "numeric");
    assert_eq!(data_types::affinity_key("FLOATING POINT"), "integer");
    assert_eq!(data_types::affinity_key("STRING"), "numeric");
    assert_eq!(data_types::affinity_key(""), "blob");
}

/// Verifies the checked SQLite affinity examples remain aligned with its documented rules.
#[test]
fn sqlite_affinity_matches_the_checked_reference_manifest() {
    let manifest: AffinityManifest = serde_yaml::from_str(include_str!(
        "../../../../tests/catalogs/sqlite-affinity.yaml"
    ))
    .expect("checked SQLite affinity manifest must parse");

    for example in manifest.examples {
        assert_eq!(
            data_types::affinity_key(&example.declared),
            example.affinity
        );
    }
}

/// Verifies SQLite STRICT tables use the database's exact permitted type names.
#[test]
fn sqlite_strict_type_allowlist_is_not_affinity_based() {
    for declared in ["INT", "INTEGER", "REAL", "TEXT", "BLOB", "ANY"] {
        assert!(data_types::strict_type_allowed(declared), "{declared}");
    }
    for declared in ["BOOLEAN", "VARCHAR(255)", "NUMERIC", "DOUBLE PRECISION"] {
        assert!(!data_types::strict_type_allowed(declared), "{declared}");
    }
}
