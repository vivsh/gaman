
use super::*;

#[test]
fn sqlite_type_catalog_canonicalizes_affinity_aliases_and_suggests_typos() {
    assert_eq!(canonical_type("int4"), "integer");
    assert_eq!(canonical_type("varchar(100)"), "text");
    assert_eq!(canonical_type("email_address"), "email_address");
    assert!(is_catalog_type("integer"));
    assert!(type_suggestions("inteer").contains(&"integer".to_string()));
}
