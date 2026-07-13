#![cfg(feature = "native")]

use gaman::Migration;
use gaman::core::{MigrationStore, StoreError};
use gaman::runner_factory::DirectoryMigrationStore;

/// Creates one minimal migration for filesystem storage tests.
fn migration(id: &str) -> Migration {
    Migration {
        id: id.to_string(),
        dependencies: Vec::new(),
        operations: Vec::new(),
        atomic: true,
    }
}

/// Verifies directory storage saves and loads canonical migration files asynchronously.
#[tokio::test]
async fn directory_store_saves_and_loads_migrations() {
    let dir = tempfile::tempdir().expect("temporary migration directory");
    let store = DirectoryMigrationStore::new(dir.path());
    let expected = migration("0001_initial");

    store.save(&expected).await.expect("save migration");
    let loaded = store.load_all().await.expect("load migrations");

    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, expected.id);
    assert!(loaded[0].operations.is_empty());
}

/// Verifies directory storage refuses to overwrite an existing migration artifact.
#[tokio::test]
async fn directory_store_refuses_overwrite() {
    let dir = tempfile::tempdir().expect("temporary migration directory");
    let store = DirectoryMigrationStore::new(dir.path());
    let migration = migration("0001_initial");

    store.save(&migration).await.expect("first save");
    let error = store
        .save(&migration)
        .await
        .expect_err("overwrite must fail");

    assert!(error.to_string().contains("already exists"), "{error}");
}

/// Verifies directory storage can load migration history from read-only files.
#[tokio::test]
async fn directory_store_loads_read_only_files() {
    let dir = tempfile::tempdir().expect("temporary migration directory");
    let store = DirectoryMigrationStore::new(dir.path());
    let expected = migration("0001_initial");
    store.save(&expected).await.expect("save migration");

    let path = dir.path().join("0001_initial.yaml");
    let mut permissions = std::fs::metadata(&path)
        .expect("migration metadata")
        .permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&path, permissions).expect("mark migration read-only");

    let loaded = store.load_all().await.expect("load read-only migration");
    assert_eq!(loaded.len(), 1);
    assert_eq!(loaded[0].id, expected.id);
    assert!(loaded[0].operations.is_empty());
}

/// Verifies concurrent writers never overwrite and leave one complete migration artifact.
#[tokio::test]
async fn directory_store_concurrent_save_has_one_winner() {
    let dir = tempfile::tempdir().expect("temporary migration directory");
    let store = DirectoryMigrationStore::new(dir.path());
    let expected = migration("0001_initial");

    let (first, second) = tokio::join!(store.save(&expected), store.save(&expected));
    assert_ne!(first.is_ok(), second.is_ok());
    let conflict = first.err().or_else(|| second.err()).expect("one conflict");
    assert!(matches!(conflict, StoreError::Conflict { .. }));

    let loaded = store.load_all().await.expect("load migration");
    assert_eq!(loaded.len(), 1);
    let temporary_count = std::fs::read_dir(dir.path())
        .expect("read migration directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
        .count();
    assert_eq!(temporary_count, 0);
}

/// Verifies migration loading rejects symbolic links instead of escaping the store boundary.
#[cfg(unix)]
#[tokio::test]
async fn directory_store_rejects_symlinked_migration() {
    use std::os::unix::fs::symlink;

    let dir = tempfile::tempdir().expect("temporary migration directory");
    let outside = tempfile::NamedTempFile::new().expect("outside migration file");
    symlink(outside.path(), dir.path().join("0001_initial.yaml")).expect("create symlink");
    let store = DirectoryMigrationStore::new(dir.path());

    let error = store.load_all().await.expect_err("symlink must fail");
    assert!(matches!(error, StoreError::InvalidMigration { .. }));
}
