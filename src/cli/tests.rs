use super::args::{ApplyCmd, Command, MakeCmd, RepairCmd, ShowConfigCmd};
use super::diagnostic::CommandError;
use super::output::{migration_movement_lines, sql_statement_for_cli};
use crate::conf::{Config, ConfigError};
use crate::migrator::MigrationMovement;
use gaman_core::dialects::Dialect;

fn test_config() -> Config {
    Config::new(
        "postgres:///".to_string(),
        "migrations".into(),
        "schema.yaml".into(),
        Dialect::Postgres,
    )
}

/// Verifies make special modes cannot be combined.
#[test]
fn make_rejects_multiple_special_modes() {
    let error = Command::Make(MakeCmd {
        name: Some("users".to_string()),
        empty: true,
        merge: true,
        check: false,
        dry_run: false,
        non_interactive: false,
    })
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("mutually exclusive"));
}

/// Verifies check mode rejects migration-generation flags and names.
#[test]
fn make_check_rejects_generation_flags() {
    let error = Command::Make(MakeCmd {
        name: None,
        empty: false,
        merge: false,
        check: true,
        dry_run: true,
        non_interactive: false,
    })
    .validate()
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("apply only to normal migration generation")
    );
}

/// Verifies empty and merge modes require an explicit migration name.
#[test]
fn make_special_modes_require_names() {
    let error = Command::Make(MakeCmd {
        name: None,
        empty: true,
        merge: false,
        check: false,
        dry_run: false,
        non_interactive: false,
    })
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("a name is required"));
}

/// Verifies check mode does not accept an ignored migration name.
#[test]
fn make_check_rejects_name() {
    let error = Command::Make(MakeCmd {
        name: Some("ignored".to_string()),
        empty: false,
        merge: false,
        check: true,
        dry_run: false,
        non_interactive: false,
    })
    .validate()
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("does not accept a migration name")
    );
}

/// Verifies only migration-writing make modes require writable storage.
#[test]
fn command_write_requirement_matches_make_mode() {
    let normal = Command::Make(MakeCmd {
        name: None,
        empty: false,
        merge: false,
        check: false,
        dry_run: false,
        non_interactive: false,
    });
    let dry_run = Command::Make(MakeCmd {
        name: None,
        empty: false,
        merge: false,
        check: false,
        dry_run: true,
        non_interactive: false,
    });

    assert!(normal.requires_writable_migrations());
    assert!(!dry_run.requires_writable_migrations());
}

/// Verifies apply execution modes do not silently override each other.
#[test]
fn apply_rejects_multiple_modes() {
    let error = Command::Apply(ApplyCmd {
        target: None,
        fake: true,
        plan: true,
        check: false,
    })
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("--fake, --plan, and --check"));
}

/// Verifies planning and checking cannot accept an ignored migration target.
#[test]
fn apply_plan_rejects_target() {
    let error = Command::Apply(ApplyCmd {
        target: Some("0001".to_string()),
        fake: false,
        plan: true,
        check: false,
    })
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("a target id is supported only"));
}

/// Verifies check mode also rejects a migration target it would otherwise ignore.
#[test]
fn apply_check_rejects_target() {
    let error = Command::Apply(ApplyCmd {
        target: Some("0001".to_string()),
        fake: false,
        plan: false,
        check: true,
    })
    .validate()
    .unwrap_err();

    assert!(error.to_string().contains("a target id is supported only"));
}

/// Verifies repair cannot execute while requesting SQL-only output.
#[test]
fn repair_rejects_apply_with_sql_only() {
    let error = Command::Repair(RepairCmd {
        apply: true,
        allow_pending: false,
        allow_partial: false,
        sql_only: true,
        schema: Vec::new(),
    })
    .validate()
    .unwrap_err();

    assert!(
        error
            .to_string()
            .contains("--apply cannot be combined with --sql-only")
    );
}

/// Verifies CLI SQL formatting does not duplicate an existing statement terminator.
#[test]
fn sql_statement_for_cli_does_not_duplicate_semicolon() {
    assert_eq!(sql_statement_for_cli("SELECT 1;"), "SELECT 1;");
}

/// Verifies CLI SQL formatting preserves multiline SQL while adding one terminator.
#[test]
fn sql_statement_for_cli_preserves_multiline_statement() {
    assert_eq!(
        sql_statement_for_cli("CREATE VIEW v AS\nSELECT 1"),
        "CREATE VIEW v AS\nSELECT 1;"
    );
}

/// Verifies target movement reports backward work rather than a zero forward count.
#[test]
fn migration_movement_reports_reverted_migrations() {
    let lines = migration_movement_lines(
        MigrationMovement {
            applied: 0,
            reverted: 2,
        },
        false,
    );

    assert_eq!(lines, vec!["2 migrations reverted."]);
}

/// Verifies CLI configuration reports a missing database URL with a direct hint.
#[test]
fn missing_database_url_has_a_cli_hint() {
    let message = CommandError::from_config_error(ConfigError::MissingDatabaseUrl).to_string();

    assert!(message.contains("DATABASE_URL is required"));
    assert!(message.contains("--database-url"));
}

/// Verifies configuration URLs redact credentials by default.
#[test]
fn config_redacts_database_passwords() {
    let config = Config::new(
        "postgres://gaman:secret@localhost/app".to_string(),
        "migrations".into(),
        "schema.yaml".into(),
        Dialect::Postgres,
    );

    assert_eq!(
        config.redacted_database_url(),
        "postgres://gaman:***@localhost/app"
    );
}

/// Verifies a complete configuration command remains constructible without hidden defaults.
#[test]
fn config_command_accepts_explicit_url_output_flag() {
    let command = Command::Config(ShowConfigCmd {
        show_database_url: true,
    });

    assert!(command.validate().is_ok());
    assert_eq!(test_config().dialect, Dialect::Postgres);
}
