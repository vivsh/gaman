use sqlparser::ast::{
    CreateFunction, CreateFunctionBody, FunctionBehavior, FunctionReturnType, FunctionSecurity,
    Statement,
};

use super::error::SqlParseError;
use super::util::{data_type_to_str, extract_string_literal, object_name_parts};
use crate::states::{FunctionDef, Volatility, schema_qualified_key};

pub(super) fn parse_create_function(
    cf: &CreateFunction,
    stmt: &Statement,
) -> Result<(String, FunctionDef), SqlParseError> {
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

    Ok((
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
        },
    ))
}
