use gaman::Migration;
use gaman::core::{MigrationSource, YamlAdapter};

#[test]
fn yaml_adapter_refuses_to_overwrite_existing_migration_file() {
    let dir = tempfile::tempdir().unwrap();
    let adapter = YamlAdapter {
        directory: dir.path().to_path_buf(),
    };
    let migration = Migration {
        id: "0001_initial".to_string(),
        dependencies: vec![],
        operations: vec![],
        atomic: true,
    };

    adapter.save(&migration).unwrap();
    let err = adapter.save(&migration).unwrap_err();

    assert!(err.to_string().contains("already exists"));
}
