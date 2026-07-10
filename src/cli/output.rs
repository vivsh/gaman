use crate::cli::diagnostic::{CliDiagnostic, CommandError};
use crate::migrator::{MigrationArtifact, MigrationListing, MigrationMovement, RepairReport};
use gaman_core::migrations::Migration;

/// Filters live status listings by id or canonical migration content.
pub(crate) fn filter_status_listings(rows: &mut Vec<MigrationListing>, search: Option<&str>) {
    if let Some(search) = search {
        let needle = search.to_lowercase();
        rows.retain(|row| migration_matches(&row.id, &row.content, &needle));
    }
}

/// Filters offline migration artifacts by id or canonical migration content.
pub(crate) fn filter_artifacts(rows: &mut Vec<MigrationArtifact>, search: Option<&str>) {
    if let Some(search) = search {
        let needle = search.to_lowercase();
        rows.retain(|row| migration_matches(&row.id, &row.content, &needle));
    }
}

fn migration_matches(id: &str, content: &str, needle: &str) -> bool {
    id.to_lowercase().contains(needle) || content.to_lowercase().contains(needle)
}

/// Prints one live migration-status row.
pub(crate) fn print_migration_row(row: &MigrationListing) {
    let marker = if row.applied { "[X]" } else { "[ ]" };
    println!("  {marker} {}", row.id);
}

/// Prints canonical migration artifacts without live application state.
pub(crate) fn print_migration_contents(rows: &[MigrationArtifact]) {
    for (index, row) in rows.iter().enumerate() {
        if index > 0 {
            println!();
        }
        println!("--- {}", row.id);
        print!("{}", row.content);
        if !row.content.ends_with('\n') {
            println!();
        }
    }
}

/// Prints a migration generation result, including canonical YAML for dry-runs.
pub(crate) fn print_migration_result(
    result: Option<Migration>,
    dry_run: bool,
) -> Result<(), CommandError> {
    match result {
        Some(migration) if dry_run => {
            let yaml = migration.to_yaml_string().map_err(|error| {
                CommandError::diagnostic("failed to serialize generated migration")
                    .detail(error.to_string())
            })?;
            println!("--- {}", migration.id);
            print!("{yaml}");
        }
        Some(migration) => println!("Created: {}", migration.id),
        None => println!("No changes detected."),
    }
    Ok(())
}

/// Prints an accurate forward/backward migration movement summary.
pub(crate) fn print_migration_movement(movement: MigrationMovement, fake: bool) {
    for line in migration_movement_lines(movement, fake) {
        println!("{line}");
    }
}

pub(crate) fn migration_movement_lines(movement: MigrationMovement, fake: bool) -> Vec<String> {
    let mut lines = Vec::new();
    if movement == MigrationMovement::default() {
        lines.push("No migration state changes.".to_string());
        return lines;
    }
    if movement.applied > 0 {
        lines.push(movement_count(
            movement.applied,
            if fake { "marked applied" } else { "applied" },
        ));
    }
    if movement.reverted > 0 {
        lines.push(movement_count(
            movement.reverted,
            if fake { "marked reverted" } else { "reverted" },
        ));
    }
    lines
}

fn movement_count(count: usize, action: &str) -> String {
    if count == 1 {
        format!("1 migration {action}.")
    } else {
        format!("{count} migrations {action}.")
    }
}

/// Prints repair output and returns an error when unrepaired drift remains.
pub(crate) fn print_repair_report(
    report: &RepairReport,
    sql_only: bool,
) -> Result<(), CommandError> {
    if sql_only {
        print_sql_statements(&report.sql);
        return Ok(());
    }
    if report.verification.findings.is_empty()
        && report.verification.pending_migrations.is_empty()
        && report.skipped_findings.is_empty()
        && report.sql.is_empty()
    {
        println!("No drift detected.");
        return Ok(());
    }
    if report.applied {
        println!("Repair applied.");
    } else {
        println!("Repair dry-run: use --apply to execute this SQL.");
    }
    for line in gaman_core::drift::format_report(&report.verification) {
        println!("{line}");
    }
    if !report.skipped_findings.is_empty() {
        println!(
            "  skipped repair finding(s): {}",
            report.skipped_findings.len()
        );
    }
    if report.sql.is_empty() {
        println!("-- No repair SQL.");
    } else {
        println!("repair sql:");
        print_sql_statements(&report.sql);
    }
    if report.verification.findings.is_empty()
        && report.verification.pending_migrations.is_empty()
        && report.skipped_findings.is_empty()
    {
        Ok(())
    } else {
        Err(repair_remaining_error(report))
    }
}

/// Prints SQL statements using one executable statement terminator each.
pub(crate) fn print_sql_statements(statements: &[String]) {
    if statements.is_empty() {
        println!("-- No operations.");
    } else {
        for statement in statements {
            println!("{}", sql_statement_for_cli(statement));
        }
    }
}

/// Formats a statement for CLI output without adding duplicate terminators.
pub(crate) fn sql_statement_for_cli(statement: &str) -> String {
    if statement.trim_end().ends_with(';') {
        statement.to_string()
    } else {
        format!("{statement};")
    }
}

/// Builds the non-zero-exit diagnostic for verify drift.
pub(crate) fn drift_detected_error(findings: usize, pending: usize) -> CommandError {
    CommandError::Diagnostic(
        CliDiagnostic::new(format!(
            "{findings} drift finding(s), {pending} pending migration(s) detected"
        ))
        .hint("review the reported properties; run `gaman repair` only for local drift recovery"),
    )
}

fn repair_remaining_error(report: &RepairReport) -> CommandError {
    let mut diagnostic = CliDiagnostic::new(format!(
        "{} drift finding(s), {} pending migration(s), {} skipped repair finding(s)",
        report.verification.findings.len(),
        report.verification.pending_migrations.len(),
        report.skipped_findings.len()
    ));
    if !report.verification.pending_migrations.is_empty() {
        diagnostic = diagnostic.hint("run `gaman apply` first or pass --allow-pending");
    }
    if !report.skipped_findings.is_empty() {
        diagnostic = diagnostic.hint("pass --allow-partial to repair only supported drift");
    }
    CommandError::Diagnostic(diagnostic)
}
