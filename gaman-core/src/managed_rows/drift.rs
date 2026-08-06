//! Targeted live managed-row inspection and repair projection.

use crate::dialects::Dialect;
use crate::drift::{DriftFinding, VerificationReport};
use crate::migration_engine::{Executor, ExecutorError};
use crate::operations::Operation;
use crate::states::{EntityKind, Schema};

/// Compares replay-owned rows with live values through bounded SELECT queries.
pub async fn verify(
    expected: &Schema,
    dialect: Dialect,
    executor: &mut dyn Executor,
) -> Result<VerificationReport, ExecutorError> {
    let mut report = VerificationReport::default();
    for (table_name, declaration) in &expected.managed_rows {
        let key = expected
            .tables
            .get(table_name)
            .and_then(|table| super::validation::resolve_key(table, declaration))
            .ok_or_else(|| {
                ExecutorError::Fetch(format!(
                    "managed rows for '{table_name}' have no eligible table identity"
                ))
            })?;
        for row in &declaration.rows {
            let identity = row.identity(&key).map_err(ExecutorError::Fetch)?;
            let sql = select_sql(dialect, table_name, &key, row);
            let observed = executor.fetch_strings(&sql).await?;
            match observed.as_slice() {
                [] => missing(&mut report, table_name, &identity, &key, row),
                [value] => {
                    let observed_row = parse_row(value).ok_or_else(|| {
                        ExecutorError::Fetch(format!(
                            "managed row query returned invalid JSON for '{table_name}[{identity}]'"
                        ))
                    })?;
                    if !rows_equal(row, &observed_row, dialect) {
                        changed(&mut report, table_name, &identity, &key, row, observed_row);
                    }
                }
                _ => {
                    return Err(ExecutorError::Fetch(format!(
                        "managed row query returned multiple rows for '{table_name}[{identity}]'"
                    )));
                }
            }
        }
    }
    Ok(report)
}

fn missing(
    report: &mut VerificationReport,
    table: &str,
    identity: &str,
    key: &[String],
    row: &super::ManagedRow,
) {
    report.findings.push(finding(
        table,
        identity,
        "presence",
        "present".to_string(),
        "missing".to_string(),
    ));
    report.operations.push(Operation::InsertRow {
        table_name: table.to_string(),
        key: key.to_vec(),
        row: row.clone(),
    });
}

fn changed(
    report: &mut VerificationReport,
    table: &str,
    identity: &str,
    key: &[String],
    row: &super::ManagedRow,
    observed: super::ManagedRow,
) {
    report.findings.push(finding(
        table,
        identity,
        "values",
        canonical_row(row),
        canonical_row(&observed),
    ));
    report.operations.push(Operation::UpdateRow {
        table_name: table.to_string(),
        key: key.to_vec(),
        old: observed,
        new: row.clone(),
    });
}

fn finding(
    table: &str,
    identity: &str,
    property: &'static str,
    expected: String,
    observed: String,
) -> DriftFinding {
    DriftFinding {
        operation: "repair_managed_row",
        entity_kind: EntityKind::Row,
        entity_name: format!("{table}[{identity}]"),
        property,
        expected,
        observed,
        note: None,
    }
}

fn canonical_row(row: &super::ManagedRow) -> String {
    serde_json::to_string(
        &row.values
            .iter()
            .map(|(name, value)| (name, &value.0))
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string())
}

fn parse_row(value: &str) -> Option<super::ManagedRow> {
    let values: std::collections::BTreeMap<String, serde_json::Value> =
        serde_json::from_str(value).ok()?;
    Some(super::ManagedRow {
        values: values
            .into_iter()
            .map(|(name, value)| (name, super::ManagedValue(value)))
            .collect(),
    })
}

fn rows_equal(
    expected: &super::ManagedRow,
    observed: &super::ManagedRow,
    dialect: Dialect,
) -> bool {
    expected.values.len() == observed.values.len()
        && expected.values.iter().all(|(name, expected)| {
            observed
                .values
                .get(name)
                .is_some_and(|observed| values_equal(&expected.0, &observed.0, dialect))
        })
}

fn values_equal(
    expected: &serde_json::Value,
    observed: &serde_json::Value,
    dialect: Dialect,
) -> bool {
    if expected == observed {
        return true;
    }
    matches!(
        (dialect, expected, observed),
        (Dialect::Sqlite, serde_json::Value::Bool(true), serde_json::Value::Number(number))
            if number.as_i64() == Some(1)
    ) || matches!(
        (dialect, expected, observed),
        (Dialect::Sqlite, serde_json::Value::Bool(false), serde_json::Value::Number(number))
            if number.as_i64() == Some(0)
    )
}

fn select_sql(dialect: Dialect, table: &str, key: &[String], row: &super::ManagedRow) -> String {
    let pairs = row
        .values
        .iter()
        .map(|(name, value)| {
            let quoted = super::sql::quote_ident(dialect, name);
            let expression = if dialect == Dialect::Sqlite
                && matches!(
                    value.0,
                    serde_json::Value::Array(_) | serde_json::Value::Object(_)
                ) {
                format!("json({quoted})")
            } else {
                quoted
            };
            match dialect {
                Dialect::Postgres => format!("'{}', {expression}", name.replace('\'', "''")),
                Dialect::Sqlite | Dialect::Mysql | Dialect::Mariadb => {
                    format!("'{}', {expression}", name.replace('\'', "''"))
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ");
    let json = match dialect {
        Dialect::Postgres => format!("json_build_object({pairs})::text"),
        Dialect::Sqlite => format!("json_object({pairs})"),
        Dialect::Mysql | Dialect::Mariadb => format!("CAST(JSON_OBJECT({pairs}) AS CHAR)"),
    };
    let predicate = key
        .iter()
        .map(|name| {
            let column = super::sql::quote_ident(dialect, name);
            let value = row
                .values
                .get(name)
                .map(|value| super::sql::literal(dialect, value))
                .unwrap_or_else(|| "NULL".to_string());
            match dialect {
                Dialect::Mysql | Dialect::Mariadb => format!("{column} <=> {value}"),
                Dialect::Postgres => format!("{column} IS NOT DISTINCT FROM {value}"),
                Dialect::Sqlite => format!("{column} IS {value}"),
            }
        })
        .collect::<Vec<_>>()
        .join(" AND ");
    format!(
        "SELECT {json} FROM {} WHERE {predicate}",
        super::sql::quote_qualified(dialect, table)
    )
}
