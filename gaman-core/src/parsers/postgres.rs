use sqlparser::ast::{
    CreateFunction, CreateFunctionBody, FunctionBehavior, FunctionReturnType, FunctionSecurity,
    Statement, TriggerEvent as AstTriggerEvent, TriggerExecBodyType, TriggerObject,
    TriggerObjectKind, TriggerPeriod, UserDefinedTypeRepresentation,
};

use super::common::{
    data_type_to_str, extract_string_literal, object_name_parts, parse_create_index,
    parse_create_table, unsupported_statement,
};
use super::error::ParseError;
use super::sql::ParseContext;
use crate::dialects::Dialect;
use crate::states::{
    EnumDef, ExtensionDef, FunctionDef, OpaqueMeta, TriggerDef, TriggerEvent, TriggerScope,
    TriggerTiming, ViewDef, Volatility, schema_qualified_key,
};

pub(super) fn lower_statement(stmt: &Statement, ctx: &mut ParseContext) -> Result<(), ParseError> {
    match stmt {
        Statement::CreateTable(ct) => ctx.insert_table(parse_create_table(ct, Dialect::Postgres)?),
        Statement::CreateIndex(ci) => {
            ctx.push_index(parse_create_index(ci));
            Ok(())
        }
        Statement::CreateView(cv) => {
            let (name, schema) = object_name_parts(&cv.name);
            let key = schema_qualified_key(&name, schema.as_deref());
            ctx.schema.views.insert(
                key,
                ViewDef {
                    name,
                    schema,
                    definition: cv.query.to_string(),
                    opaque: OpaqueMeta::default(),
                },
            );
            Ok(())
        }
        Statement::CreateExtension(ce) => {
            let name = ce.name.value.clone();
            let schema = ce.schema.as_ref().map(|i| i.value.clone());
            let key = schema_qualified_key(&name, schema.as_deref());
            ctx.schema.extensions.insert(
                key,
                ExtensionDef {
                    name,
                    schema,
                    version: ce.version.as_ref().map(|i| i.value.clone()),
                    opaque: OpaqueMeta::default(),
                },
            );
            Ok(())
        }
        Statement::CreateType {
            name,
            representation,
        } => lower_create_type(name, representation, ctx),
        Statement::CreateFunction(cf) => {
            let (key, func) = parse_create_function(cf, stmt);
            ctx.schema.functions.insert(key, func);
            Ok(())
        }
        Statement::CreateTrigger(trigger) => lower_create_trigger(trigger, ctx),
        other => Err(unsupported_statement(
            Dialect::Postgres,
            other,
            "statement is not represented in Schema",
        )),
    }
}

fn lower_create_type(
    name: &sqlparser::ast::ObjectName,
    representation: &Option<UserDefinedTypeRepresentation>,
    ctx: &mut ParseContext,
) -> Result<(), ParseError> {
    let (type_name, schema) = object_name_parts(name);
    let key = schema_qualified_key(&type_name, schema.as_deref());
    match representation {
        Some(UserDefinedTypeRepresentation::Enum { labels }) => {
            ctx.schema.enums.insert(
                key,
                EnumDef {
                    name: type_name,
                    schema,
                    values: labels.iter().map(|l| l.value.clone()).collect(),
                    opaque: OpaqueMeta::default(),
                },
            );
            Ok(())
        }
        other => Err(ParseError::unsupported(
            Dialect::Postgres,
            format!(
                "CREATE TYPE {}",
                other.as_ref().map(|r| r.to_string()).unwrap_or_default()
            ),
            "only enum types are represented in Schema",
        )),
    }
}

fn parse_create_function(cf: &CreateFunction, stmt: &Statement) -> (String, FunctionDef) {
    let (fn_name, schema) = object_name_parts(&cf.name);
    let key = schema_qualified_key(&fn_name, schema.as_deref());

    let arguments = cf
        .args
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .map(|arg| {
            let mode = arg
                .mode
                .as_ref()
                .map(|m| format!("{} ", m))
                .unwrap_or_default();
            let name = arg
                .name
                .as_ref()
                .map(|n| format!("{} ", n.value))
                .unwrap_or_default();
            format!("{}{}{}", mode, name, data_type_to_str(&arg.data_type))
        })
        .collect::<Vec<_>>()
        .join(", ");

    let returns = cf
        .return_type
        .as_ref()
        .map(|return_type| match return_type {
            FunctionReturnType::DataType(data_type) => data_type_to_str(data_type),
            FunctionReturnType::SetOf(data_type) => {
                format!("SETOF {}", data_type_to_str(data_type))
            }
        })
        .unwrap_or_else(|| "void".to_string());

    let language = cf
        .language
        .as_ref()
        .map(|l| l.value.to_lowercase())
        .unwrap_or_else(|| "sql".to_string());

    let body = match &cf.function_body {
        Some(CreateFunctionBody::AsBeforeOptions { body, .. }) => {
            extract_string_literal(body).unwrap_or_else(|| body.to_string())
        }
        Some(CreateFunctionBody::AsAfterOptions(expr)) => {
            extract_string_literal(expr).unwrap_or_else(|| expr.to_string())
        }
        Some(CreateFunctionBody::Return(expr)) => expr.to_string(),
        Some(CreateFunctionBody::AsReturnExpr(expr)) => expr.to_string(),
        Some(CreateFunctionBody::AsReturnSelect(query)) => query.to_string(),
        Some(CreateFunctionBody::AsBeginEnd(_)) => stmt.to_string(),
        None => String::new(),
    };

    let volatility = match &cf.behavior {
        Some(FunctionBehavior::Immutable) => Volatility::Immutable,
        Some(FunctionBehavior::Stable) => Volatility::Stable,
        _ => Volatility::Volatile,
    };

    (
        key,
        FunctionDef {
            name: fn_name,
            schema,
            arguments,
            returns,
            language,
            body,
            volatility,
            security_definer: matches!(&cf.security, Some(FunctionSecurity::Definer)),
            opaque: OpaqueMeta::default(),
        },
    )
}

fn lower_create_trigger(
    trigger: &sqlparser::ast::CreateTrigger,
    ctx: &mut ParseContext,
) -> Result<(), ParseError> {
    if trigger.exec_body.is_none() {
        return Err(ParseError::unsupported(
            Dialect::Postgres,
            trigger.to_string(),
            "only function-backed triggers are represented for PostgreSQL",
        ));
    }
    let exec_body = trigger.exec_body.as_ref().expect("checked above");
    if exec_body.exec_type != TriggerExecBodyType::Function {
        return Err(ParseError::unsupported(
            Dialect::Postgres,
            trigger.to_string(),
            "only EXECUTE FUNCTION triggers are represented",
        ));
    }
    let (table_name, schema) = object_name_parts(&trigger.table_name);
    let table_key = schema_qualified_key(&table_name, schema.as_deref());
    let table =
        ctx.schema
            .tables
            .get_mut(&table_key)
            .ok_or_else(|| ParseError::UnknownTriggerTable {
                table: table_key.clone(),
            })?;
    table.triggers.push(TriggerDef {
        name: Some(object_name_parts(&trigger.name).0),
        timing: trigger_timing(trigger.period),
        events: trigger_events(&trigger.events),
        scope: trigger_scope(trigger.trigger_object),
        function_name: Some(exec_body.func_desc.to_string()),
        when: trigger.condition.as_ref().map(ToString::to_string),
        query: None,
        language: None,
        opaque: OpaqueMeta::default(),
    });
    Ok(())
}

pub(super) fn trigger_timing(period: Option<TriggerPeriod>) -> TriggerTiming {
    match period {
        Some(TriggerPeriod::Before) => TriggerTiming::Before,
        Some(TriggerPeriod::InsteadOf) => TriggerTiming::InsteadOf,
        _ => TriggerTiming::After,
    }
}

pub(super) fn trigger_scope(object: Option<TriggerObjectKind>) -> TriggerScope {
    match object {
        Some(TriggerObjectKind::For(TriggerObject::Statement))
        | Some(TriggerObjectKind::ForEach(TriggerObject::Statement)) => TriggerScope::Statement,
        _ => TriggerScope::Row,
    }
}

pub(super) fn trigger_events(events: &[AstTriggerEvent]) -> Vec<TriggerEvent> {
    events
        .iter()
        .map(|event| match event {
            AstTriggerEvent::Insert => TriggerEvent::Insert,
            AstTriggerEvent::Update(_) => TriggerEvent::Update,
            AstTriggerEvent::Delete => TriggerEvent::Delete,
            AstTriggerEvent::Truncate => TriggerEvent::Truncate,
        })
        .collect()
}
