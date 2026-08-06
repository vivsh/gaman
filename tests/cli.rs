use std::fs;
use std::process::Command;

#[cfg(feature = "postgres")]
use sqlx::Connection;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

fn gaman_command(dir: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gaman"));
    command
        .current_dir(dir.path())
        .env("DATABASE_URL", "postgres://localhost/gaman_cli_test");
    command
}

#[cfg(feature = "sqlite")]
fn sqlite_gaman_command(dir: &tempfile::TempDir) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_gaman"));
    command
        .current_dir(dir.path())
        .env("DATABASE_URL", "sqlite::memory:");
    command
}

fn write_migration(dir: &tempfile::TempDir, id: &str) {
    let migrations = dir.path().join("migrations");
    fs::create_dir_all(&migrations).expect("create migrations directory");
    fs::write(
        migrations.join(format!("{id}.yaml")),
        format!("id: {id}\ndependencies: []\noperations: []\natomic: true\n"),
    )
    .expect("write migration");
}

/// Verifies entity filters are rejected for global check, empty, and merge
/// generation modes before migration planning begins.
#[test]
fn make_filters_reject_incompatible_modes() {
    let cases = [
        vec!["make", "--check", "--filter", "users"],
        vec!["make", "empty", "--empty", "--filter", "users"],
        vec!["make", "merge", "--merge", "--filter", "users"],
    ];
    for arguments in cases {
        let dir = tempfile::tempdir().expect("create temp directory");
        fs::write(dir.path().join("schema.yaml"), "tables: {}\n").expect("write empty schema");
        let output = gaman_command(&dir)
            .args(&arguments)
            .output()
            .expect("run filtered make");

        assert!(
            !output.status.success(),
            "{} unexpectedly accepted filters",
            arguments[1]
        );
        let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
        assert!(stderr.contains("filter"), "{}: {stderr}", arguments[1]);
    }
}

/// Verifies the no-argument banner describes Gaman without privileging PostgreSQL.
#[test]
fn banner_is_dialect_neutral() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = gaman_command(&dir).output().expect("run gaman");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("Offline-first migration tool"));
    assert!(!stderr.contains("PostgreSQL-first"));
}

/// Verifies show reads migration artifacts without opening the configured database.
#[test]
fn show_is_offline_and_omits_application_status() {
    let dir = tempfile::tempdir().expect("create temp directory");
    write_migration(&dir, "0001_init");
    let output = gaman_command(&dir)
        .args(["show", "0001"])
        .output()
        .expect("run gaman show");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("--- 0001_init"));
    assert!(!stdout.contains("(applied)"));
    assert!(!stdout.contains("(pending)"));
}

/// Verifies check_schema prepares SQL while reporting non-SQL schema files as ignored.
#[cfg(feature = "sqlite")]
#[test]
fn check_schema_reports_checked_and_ignored_files_without_migrations() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let schema = dir.path().join("schema");
    fs::create_dir_all(&schema).expect("create schema directory");
    fs::write(
        schema.join("schema.sql"),
        "CREATE TABLE users (id integer);",
    )
    .expect("write SQL schema");
    fs::write(schema.join("legacy.yaml"), "tables: {}\n").expect("write YAML schema");

    let output = sqlite_gaman_command(&dir)
        .args(["--schema", "schema", "check_schema"])
        .output()
        .expect("run check_schema");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("schema.sql passed (1)"), "{stdout}");
    assert!(
        stdout.contains("legacy.yaml ignored (YAML schema input)"),
        "{stdout}"
    );
}

/// Verifies YAML-only schema input is ignored without opening the configured database.
#[test]
fn check_schema_ignores_yaml_without_a_database_connection() {
    let dir = tempfile::tempdir().expect("create temp directory");
    fs::write(dir.path().join("schema.yaml"), "tables: {}\n").expect("write YAML schema");

    let output = gaman_command(&dir)
        .arg("check_schema")
        .output()
        .expect("run check_schema");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(
        stdout.contains("schema.yaml ignored (YAML schema input)"),
        "{stdout}"
    );
}

/// Verifies check_schema reports a prepare failure and exits non-zero without migration storage.
#[cfg(feature = "sqlite")]
#[test]
fn check_schema_reports_prepare_failures() {
    let dir = tempfile::tempdir().expect("create temp directory");
    fs::write(dir.path().join("schema.sql"), "SELECT FROM;").expect("write malformed SQL schema");

    let output = sqlite_gaman_command(&dir)
        .args(["--schema", "schema.sql", "check_schema"])
        .output()
        .expect("run check_schema");

    assert!(!output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(
        stdout.contains("schema.sql passed (0), failed (1)"),
        "{stdout}"
    );
    assert!(
        stdout.contains("statement 1 (line 1): prepare failed:"),
        "{stdout}"
    );
    assert!(stderr.contains("schema check failed"), "{stderr}");
}

/// Verifies PostgreSQL prepare validation does not execute an otherwise valid schema statement.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn check_schema_postgres_does_not_create_prepared_table() {
    let Ok(database_url) = std::env::var("POSTGRES_DATABASE_URL") else {
        return;
    };
    let dir = tempfile::tempdir().expect("create temp directory");
    let table = format!("gaman_check_schema_{}", std::process::id());
    fs::write(
        dir.path().join("schema.sql"),
        format!("CREATE TABLE {table} (id integer);"),
    )
    .expect("write SQL schema");

    let output = Command::new(env!("CARGO_BIN_EXE_gaman"))
        .current_dir(dir.path())
        .env("DATABASE_URL", &database_url)
        .args(["--schema", "schema.sql", "check_schema"])
        .output()
        .expect("run PostgreSQL check_schema");

    assert!(output.status.success());
    let mut connection = sqlx::PgConnection::connect(&database_url)
        .await
        .expect("connect PostgreSQL");
    let exists: Option<String> = sqlx::query_scalar("SELECT to_regclass($1)::text")
        .bind(&table)
        .fetch_one(&mut connection)
        .await
        .expect("query prepared table");
    assert!(
        exists.is_none(),
        "check_schema unexpectedly created {table}"
    );
}

/// Verifies ambiguous migration id prefixes report each graph-ordered candidate.
#[test]
fn show_reports_ambiguous_prefixes() {
    let dir = tempfile::tempdir().expect("create temp directory");
    write_migration(&dir, "0001_users");
    write_migration(&dir, "0001_posts");
    let output = gaman_command(&dir)
        .args(["show", "0001"])
        .output()
        .expect("run gaman show");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("migration id prefix '0001' is ambiguous"));
    assert!(stderr.contains("0001_posts, 0001_users"));
}

/// Verifies replay failures retain migration and operation context in normal CLI output.
#[test]
fn make_reports_actionable_replay_failures() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let migrations = dir.path().join("migrations");
    fs::create_dir_all(&migrations).expect("create migrations directory");
    fs::write(
        migrations.join("0001_drop_missing.yaml"),
        "id: 0001_drop_missing\ndependencies: []\noperations:\n  - type: drop_table\n    table:\n      name: missing\n      columns: []\natomic: true\n",
    )
    .expect("write invalid replay migration");
    fs::write(dir.path().join("schema.yaml"), "tables: {}\n").expect("write schema");

    let output = gaman_command(&dir)
        .arg("make")
        .output()
        .expect("run gaman make");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("cannot replay migration '0001_drop_missing'"));
    assert!(stderr.contains("operation 1 (drop table missing): table 'missing' not found"));
    assert!(stderr.contains("hint: correct the migration order"));
}

/// Verifies normal and verbose CLI failures do not reveal connection credentials.
#[test]
fn verbose_connection_failures_redact_credentials() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let normal = gaman_command(&dir)
        .env("DATABASE_URL", "postgres://gaman:secret@127.0.0.1:1/app")
        .arg("status")
        .output()
        .expect("run normal gaman status");
    let verbose = gaman_command(&dir)
        .env("DATABASE_URL", "postgres://gaman:secret@127.0.0.1:1/app")
        .args(["--verbose", "status"])
        .output()
        .expect("run verbose gaman status");

    let normal_stderr = String::from_utf8(normal.stderr).expect("utf8 normal stderr");
    let verbose_stderr = String::from_utf8(verbose.stderr).expect("utf8 verbose stderr");
    assert!(!normal.status.success());
    assert!(!verbose.status.success());
    assert!(!normal_stderr.contains("secret"));
    assert!(!normal_stderr.contains("causes:"));
    assert!(!verbose_stderr.contains("secret"));
    assert!(verbose_stderr.contains("causes:"), "{verbose_stderr}");
}

/// Verifies SQL rendering uses the shared unique-prefix resolver.
#[test]
fn sql_accepts_a_unique_migration_prefix() {
    let dir = tempfile::tempdir().expect("create temp directory");
    write_migration(&dir, "0001_init");
    let output = gaman_command(&dir)
        .args(["sql", "0001"])
        .output()
        .expect("run gaman sql");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("-- No operations."));
}

/// Verifies apply accepts a Django-style positional target instead of a separate rollback command.
#[test]
fn apply_accepts_a_positional_target() {
    let dir = tempfile::tempdir().expect("create temp directory");
    write_migration(&dir, "0001_init");
    let output = gaman_command(&dir)
        .args(["apply", "0001", "--plan"])
        .output()
        .expect("run gaman apply");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("a target id is supported only"));
}

/// Verifies rollback is not exposed as a duplicate CLI convergence command.
#[test]
fn rollback_is_not_a_cli_subcommand() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = gaman_command(&dir)
        .args(["rollback", "0001"])
        .output()
        .expect("run gaman rollback");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.to_lowercase().contains("unrecognized argument"));
    assert!(stderr.contains("rollback"));
}

/// Verifies config hides credentials unless explicitly requested.
#[test]
fn config_redacts_credentials_by_default() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = Command::new(env!("CARGO_BIN_EXE_gaman"))
        .current_dir(dir.path())
        .env("DATABASE_URL", "postgres://gaman:secret@localhost/app")
        .arg("config")
        .output()
        .expect("run gaman config");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("postgres://gaman:***@localhost/app"));
    assert!(!stdout.contains("secret"));
}

/// Verifies config exposes the full URL only after explicit opt-in.
#[test]
fn config_can_show_full_database_url_explicitly() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = Command::new(env!("CARGO_BIN_EXE_gaman"))
        .current_dir(dir.path())
        .env("DATABASE_URL", "postgres://gaman:secret@localhost/app")
        .args(["config", "--show-database-url"])
        .output()
        .expect("run gaman config");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("postgres://gaman:secret@localhost/app"));
}

/// Verifies conflicting make modes fail before any migration planning occurs.
#[test]
fn make_rejects_conflicting_modes() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = gaman_command(&dir)
        .args(["make", "--check", "--dry-run"])
        .output()
        .expect("run gaman make");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("apply only to normal migration generation"));
}

/// Verifies CLI path overrides are applied before filesystem configuration validation.
#[test]
fn config_validates_overridden_migrations_dir() {
    let dir = tempfile::tempdir().expect("create temp directory");
    fs::write(dir.path().join("migrations"), "not a directory")
        .expect("create invalid default migrations path");
    let output = gaman_command(&dir)
        .args(["--migrations-dir", "custom-migrations", "config"])
        .output()
        .expect("run gaman config");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("custom-migrations"));
}

/// Verifies the CLI accepts MySQL configuration when native support is enabled.
#[cfg(feature = "mysql")]
#[test]
fn config_accepts_enabled_mysql() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = Command::new(env!("CARGO_BIN_EXE_gaman"))
        .current_dir(dir.path())
        .args(["--database-url", "mysql://localhost/app", "config"])
        .output()
        .expect("run gaman config");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("mysql"));
}

/// Verifies the CLI rejects MySQL configuration when native support is disabled.
#[cfg(not(feature = "mysql"))]
#[test]
fn config_rejects_disabled_mysql() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = Command::new(env!("CARGO_BIN_EXE_gaman"))
        .current_dir(dir.path())
        .args(["--database-url", "mysql://localhost/app", "status"])
        .output()
        .expect("run gaman config");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");
    assert!(stderr.contains("rebuild with the 'mysql' feature"));
}

/// Verifies dry-run emits canonical migration YAML without a creation claim.
#[test]
fn make_dry_run_prints_generated_migration_yaml() {
    let dir = tempfile::tempdir().expect("create temp directory");
    fs::write(
        dir.path().join("schema.yaml"),
        "tables:\n  users:\n    columns:\n      - name: id\n        type: integer\n",
    )
    .expect("write schema");
    let output = gaman_command(&dir)
        .args(["make", "initial", "--dry-run", "--non-interactive"])
        .output()
        .expect("run gaman make dry-run");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.starts_with("--- 0001_initial\n"), "{stdout}");
    assert!(stdout.contains("type: create_table"));
    assert!(!stdout.contains("Created:"));
}

/// Verifies filtered dry-run and persisted generation produce identical
/// canonical migration content.
#[test]
fn filtered_dry_run_matches_persisted_output() {
    let dir = tempfile::tempdir().expect("create temp directory");
    fs::write(
        dir.path().join("schema.yaml"),
        "tables:\n  users:\n    columns:\n      - name: id\n        type: integer\n  projects:\n    columns:\n      - name: id\n        type: integer\n",
    )
    .expect("write schema");
    let dry_run = gaman_command(&dir)
        .args([
            "make",
            "focused",
            "--filter",
            "users",
            "--dry-run",
            "--non-interactive",
        ])
        .output()
        .expect("run filtered dry-run");
    assert!(dry_run.status.success());
    let stdout = String::from_utf8(dry_run.stdout).expect("utf8 stdout");
    let expected = stdout
        .strip_prefix("--- 0001_focused\n")
        .expect("dry-run migration header");

    let persisted = gaman_command(&dir)
        .args(["make", "focused", "--filter", "users", "--non-interactive"])
        .output()
        .expect("persist filtered migration");
    assert!(persisted.status.success());
    let actual = fs::read_to_string(dir.path().join("migrations/0001_focused.yaml"))
        .expect("read persisted migration");

    assert_eq!(expected, actual);
    assert!(!actual.contains("projects"));
}

/// Verifies artifact inspection accepts a read-only migration directory.
#[cfg(unix)]
#[test]
fn show_accepts_read_only_migration_directory() {
    let dir = tempfile::tempdir().expect("create temp directory");
    write_migration(&dir, "0001_init");
    let migrations = dir.path().join("migrations");
    fs::set_permissions(&migrations, fs::Permissions::from_mode(0o555))
        .expect("make migrations read-only");

    let output = gaman_command(&dir)
        .args(["show", "0001"])
        .output()
        .expect("run gaman show");

    assert!(output.status.success());
}

/// Verifies generated help advertises the migration-prefix lookup contract.
#[test]
fn show_help_documents_unique_prefixes() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = gaman_command(&dir)
        .args(["show", "--help"])
        .output()
        .expect("run gaman show help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("unique id prefix"));
}

/// Verifies top-level help advertises the supported version flag.
#[test]
fn top_level_help_documents_version() {
    let dir = tempfile::tempdir().expect("create temp directory");
    let output = gaman_command(&dir)
        .arg("--help")
        .output()
        .expect("run gaman help");

    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    assert!(stdout.contains("-V, --version"));
}
