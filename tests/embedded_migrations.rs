use std::path::{Path, PathBuf};

use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("tests/fixtures/embedded_migrations");

#[test]
fn embedded_migrations_macro_stores_absolute_source_dir() {
    let dir = Path::new(MIGRATIONS.dir);

    assert!(dir.is_absolute());
    assert!(dir.ends_with("tests/fixtures/embedded_migrations"));
}

#[test]
fn make_empty_migration_writes_to_embedded_dir_for_relative_config_from_other_cwd() {
    let temp = tempfile::tempdir().unwrap();
    let original_cwd = std::env::current_dir().unwrap();
    std::env::set_current_dir(temp.path()).unwrap();

    let result = MigrationEngine::new(
        Config::new(
            None,
            PathBuf::from("tests/fixtures/embedded_migrations"),
            PathBuf::from("schema.yaml"),
        ),
        &MIGRATIONS,
    )
    .make_empty_migration("from_relative_config");

    std::env::set_current_dir(original_cwd).unwrap();

    let migration = result.unwrap();
    let written = Path::new(MIGRATIONS.dir).join(format!("{}.yaml", migration.id));
    assert!(written.exists());
    std::fs::remove_file(written).unwrap();
}
