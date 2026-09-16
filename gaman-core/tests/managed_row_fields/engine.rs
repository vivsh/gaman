//! Ownership guards use the normal runner lifecycle and fail closed on query errors.

use super::*;
use gaman_core::operations::Operation;

/// Builds an owned defaulted field and a separate migration relinquishing that field only.
fn migrations() -> (Migration, Migration) {
    let mut initial =
        Migration::from_yaml_str(include_str!("../fixtures/managed_row_fields.yaml")).unwrap();
    initial.id = "0001_profiles".into();
    initial.atomic = false;
    initial.operations.truncate(2);
    if let Operation::CreateTable { table } = &mut initial.operations[0] {
        table.columns.push(
            serde_yaml::from_str("{name: auxiliary_output_limit, type: integer, default: '2048'}")
                .unwrap(),
        );
    }
    let Operation::InsertRow { row, .. } = &mut initial.operations[1] else {
        panic!("insert fixture")
    };
    let new = row.clone();
    row.values
        .insert("auxiliary_output_limit".into(), ManagedValue(3072.into()));
    let mut release = Migration::from_yaml_str("atomic: true\noperations: []").unwrap();
    release.id = "0002_release".into();
    release.dependencies = vec![initial.id.clone()];
    release.operations = vec![Operation::UpdateRow {
        table_name: "model_profiles".into(),
        key: vec!["id".into()],
        old: row.clone(),
        new,
    }];
    (initial, release)
}

/// Every dialect requires exactly one guard result and propagates query failures without recording.
#[test]
fn ownership_guards_enforce_cardinality_and_query_errors_through_runner() {
    for dialect in [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ] {
        for outcome in [
            Ok(vec!["1".into()]),
            Ok(vec![]),
            Ok(vec!["1".into(), "1".into()]),
            Err(ExecutorError::Fetch("guard query failed".into())),
        ] {
            guard_outcome(dialect, outcome);
        }
    }
}

/// Checks the real runner's commit/tracking decision without allowing a guard to become DML.
fn guard_outcome(dialect: Dialect, outcome: Result<Vec<String>, ExecutorError>) {
    block_on(async {
        let succeeds = matches!(&outcome, Ok(rows) if rows.len() == 1);
        let (initial, release) = migrations();
        let tracking = tracking([initial.id.clone()]);
        let records = Arc::clone(&tracking.records);
        let log = Arc::new(Mutex::new(Vec::new()));
        let executor = executor(
            [Err(ExecutorError::Unsupported(
                "guard must not issue DML".into(),
            ))],
            [outcome],
            Arc::clone(&log),
        );
        let mut runner = MigrationRunner::new(
            dialect,
            SharedStore(Arc::new(Mutex::new(vec![initial, release.clone()]))),
            tracking,
            executor,
        );
        let result = runner.run_command(&apply_command(None)).await;
        assert_eq!(result.is_ok(), succeeds, "{result:?}");
        assert_eq!(records.lock().unwrap().len(), usize::from(succeeds));
        if succeeds {
            assert_eq!(records.lock().unwrap().as_slice(), [release.id]);
        } else {
            let message = result.unwrap_err().to_string();
            assert!(
                message.contains("precondition")
                    || message.contains("integrity")
                    || message.contains("guard query failed"),
                "{message}"
            );
            if dialect.supports_transactional_ddl() {
                assert!(log.lock().unwrap().iter().any(|sql| sql == "ROLLBACK"));
            }
        }
        assert!(
            log.lock()
                .unwrap()
                .iter()
                .any(|sql| sql.starts_with("SELECT '1' FROM "))
        );
    });
}
