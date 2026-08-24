//! PostgreSQL drift-repair regression coverage for explicit typed casts.

use gaman_core::Dialect;
use gaman_core::drift;
use gaman_core::schema::{Operation, Schema};

fn schema(column_type: &str) -> Schema {
    Schema::from_yaml_str(
        &format!(
            "tables:\n  crawler_source:\n    columns:\n      - name: health_fault\n        type: {column_type}\n        nullable: true\n"
        ),
        Dialect::Postgres,
    )
    .expect("schema should prepare")
}

fn repair_cast(report: &drift::VerificationReport) -> Option<&str> {
    report
        .operations
        .first()
        .and_then(|operation| match operation {
            Operation::AlterColumn { cast_expr, .. } => cast_expr.as_deref(),
            _ => None,
        })
}

/// Projects a verified PostgreSQL text-to-jsonb drift as a typed explicit cast.
#[test]
fn text_to_jsonb_repair_uses_quoted_column_cast() {
    let report = drift::diff(schema("jsonb"), schema("text"), "public", Dialect::Postgres);

    assert_eq!(report.operations.len(), 1);
    assert_eq!(repair_cast(&report), Some("\"health_fault\"::jsonb"));
}

/// Preserves direct PostgreSQL repair rendering when no explicit cast is required.
#[test]
fn varchar_to_text_repair_keeps_implicit_conversion() {
    let report = drift::diff(
        schema("text"),
        schema("varchar(64)"),
        "public",
        Dialect::Postgres,
    );

    assert_eq!(report.operations.len(), 1);
    assert_eq!(repair_cast(&report), None);
}
