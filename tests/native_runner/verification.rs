//! Native managed-row inspection and repair must use the same checked execution path.

use super::lifecycle::{assert_items, inserted};
use super::support::*;
use gaman_core::operations::Operation;
use gaman_core::{Command, CommandResult, RepairOptions};

/// Verifies targeted reads and checked repairs preserve unmanaged data without changing history.
#[tokio::test]
#[ignore = "requires dbharness; run with --include-ignored"]
async fn native_verification_and_repair_preserve_ownership() {
    with_database(|config, mut observer| async move {
        let mut runner = inserted(&config).await;
        verified(&mut runner).await;
        sql(&mut observer, "UPDATE items SET name = 'changed', external_note = 'keep-owned' WHERE id = 'managed'").await;
        let result = runner.run_command(&Command::Repair {
            schemas: vec!["public".into()], options: RepairOptions { sql_only: true, ..Default::default() },
        }).await.expect("repair preview");
        let CommandResult::Repair(preview) = result else { panic!("expected repair preview") };
        assert!(!preview.applied);
        assert_eq!(preview.verification.findings.len(), 1);
        assert_eq!(preview.operations.len(), 1);
        let Operation::UpdateRow { old, new, .. } = &preview.operations[0] else { panic!("expected checked repair") };
        assert_eq!(old.values["name"].0.as_str(), Some("changed"));
        assert_eq!(new.values["name"].0.as_str(), Some("first"));
        assert!(!preview.sql[0].contains("external_note"));
        assert!(preview.sql[0].contains("IS NOT DISTINCT FROM"));
        assert_items(&mut observer, &["external:outside:keep-external", "managed:changed:keep-owned"]).await;
        let result = runner.run_command(&Command::Repair {
            schemas: vec!["public".into()], options: RepairOptions { apply: true, ..Default::default() },
        }).await.expect("apply checked repair");
        assert!(matches!(result, CommandResult::Repair(report) if report.applied && report.verification.findings.is_empty()));
        assert_items(&mut observer, &["external:outside:keep-external", "managed:first:keep-owned"]).await;
        records(&mut observer, &["0001_items", "0002_insert"]).await;
        verified(&mut runner).await;
    }).await;
}
