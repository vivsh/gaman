use crate::dialects::Dialect;
use crate::states::{FunctionBuilder, FunctionDef, Schema};

/// Verifies the fluent API preserves typed parameter defaults and exact dependencies.
#[test]
fn function_builder_preserves_defaults_and_dependencies() {
    let schema = Schema::builder(Dialect::Postgres)
        .function(
            FunctionBuilder::new("providers")
                .returns("integer")
                .language("sql")
                .body("SELECT 1"),
        )
        .function(
            FunctionBuilder::new("daily_report")
                .parameter("start_date", "date")
                .parameter_default("provider_id", "integer", "NULL")
                .returns("jsonb")
                .language("sql")
                .body("SELECT '{}'::jsonb")
                .depends_on(["function::providers"]),
        )
        .build()
        .expect("valid typed function schema");
    let function = schema
        .functions
        .get("daily_report(date, integer)")
        .expect("function");
    assert_eq!(function.parameters[1].default.as_deref(), Some("NULL"));
    assert_eq!(function.depends_on[0].target, "providers");
}

/// Verifies legacy and typed argument representations cannot be mixed ambiguously.
#[test]
fn rejects_mixed_legacy_and_typed_parameters() {
    let function = FunctionDef {
        name: "daily_report".to_string(),
        schema: None,
        parameters: vec![crate::states::FunctionParameter {
            name: "day".to_string(),
            type_name: "date".to_string(),
            default: None,
        }],
        arguments: "day date".to_string(),
        returns: "jsonb".to_string(),
        language: "sql".to_string(),
        body: "SELECT '{}'::jsonb".to_string(),
        depends_on: Vec::new(),
        volatility: Default::default(),
        security_definer: false,
        opaque: Default::default(),
    };
    let error = Schema::builder(Dialect::Postgres)
        .function(function)
        .build()
        .expect_err("mixed parameters must fail");
    assert!(
        error
            .to_string()
            .contains("both legacy arguments and typed parameters")
    );
}

/// Verifies function dependency cycles fail during whole-schema preparation.
#[test]
fn rejects_function_dependency_cycles() {
    let yaml = r#"
functions:
  first:
    name: first
    parameters: []
    returns: integer
    language: sql
    body: SELECT 1
    depends_on: [function::second]
  second:
    name: second
    parameters: []
    returns: integer
    language: sql
    body: SELECT 1
    depends_on: [function::first]
"#;
    let error =
        Schema::from_yaml_str(yaml, Dialect::Postgres).expect_err("dependency cycle must fail");
    assert!(error.to_string().contains("function dependency cycle"));
}

/// Verifies PostgreSQL SQL parsing preserves defaults and resolves leading annotations after all declarations load.
#[test]
fn sql_functions_preserve_defaults_and_forward_dependencies() {
    let sql = r#"
-- @depends-on function::helper
CREATE FUNCTION report(p_day date, p_provider integer DEFAULT NULL)
RETURNS integer LANGUAGE sql AS $$ SELECT helper() $$;

CREATE FUNCTION helper()
RETURNS integer LANGUAGE sql AS $$ SELECT 1 $$;
"#;
    let schema = Schema::from_sql_str(sql, Dialect::Postgres).expect("parse function schema");
    let report = schema
        .functions
        .get("report(date, integer)")
        .expect("typed report identity");
    assert_eq!(report.parameters[1].default.as_deref(), Some("NULL"));
    assert_eq!(report.depends_on[0].target, "helper");
}
