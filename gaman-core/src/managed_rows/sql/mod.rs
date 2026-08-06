use crate::dialects::{Dialect, DialectError};
use crate::managed_rows::{ManagedRow, ManagedValue};
use crate::operations::Operation;

/// Renders one managed-row operation for the selected dialect.
pub fn render(dialect: Dialect, operation: &Operation) -> Result<Vec<String>, DialectError> {
    let sql = match operation {
        Operation::InsertRow {
            table_name, row, ..
        } => insert_sql(dialect, table_name, row),
        Operation::UpdateRow {
            table_name,
            key,
            old,
            new,
        } => update_sql(dialect, table_name, key, old, new)?,
        Operation::DeleteRow {
            table_name,
            key,
            row,
        } => delete_sql(dialect, table_name, key, row)?,
        _ => {
            return Err(DialectError::Unsupported(
                operation.type_name().to_string(),
                "not a managed-row operation".to_string(),
            ));
        }
    };
    Ok(vec![sql])
}

fn insert_sql(dialect: Dialect, table: &str, row: &ManagedRow) -> String {
    let columns = row
        .values
        .keys()
        .map(|name| quote_ident(dialect, name))
        .collect::<Vec<_>>()
        .join(", ");
    let values = row
        .values
        .values()
        .map(|value| literal(dialect, value))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "INSERT INTO {} ({columns}) VALUES ({values})",
        quote_qualified(dialect, table)
    )
}

fn update_sql(
    dialect: Dialect,
    table: &str,
    key: &[String],
    old: &ManagedRow,
    new: &ManagedRow,
) -> Result<String, DialectError> {
    let assignments = new
        .values
        .iter()
        .filter(|(name, value)| old.values.get(*name) != Some(*value))
        .map(|(name, value)| {
            format!(
                "{} = {}",
                quote_ident(dialect, name),
                literal(dialect, value)
            )
        })
        .collect::<Vec<_>>();
    if assignments.is_empty() {
        return Err(DialectError::Unsupported(
            "update_row".to_string(),
            "managed row update has no changed values".to_string(),
        ));
    }
    Ok(format!(
        "UPDATE {} SET {} WHERE {}",
        quote_qualified(dialect, table),
        assignments.join(", "),
        predicate(dialect, key, old)?
    ))
}

fn delete_sql(
    dialect: Dialect,
    table: &str,
    key: &[String],
    row: &ManagedRow,
) -> Result<String, DialectError> {
    Ok(format!(
        "DELETE FROM {} WHERE {}",
        quote_qualified(dialect, table),
        predicate(dialect, key, row)?
    ))
}

fn predicate(dialect: Dialect, key: &[String], row: &ManagedRow) -> Result<String, DialectError> {
    let mut ordered = Vec::new();
    for name in key
        .iter()
        .chain(row.values.keys().filter(|name| !key.contains(name)))
    {
        let value = row.values.get(name).ok_or_else(|| {
            DialectError::Unsupported(
                "managed_rows".to_string(),
                format!("missing predicate column '{name}'"),
            )
        })?;
        let column = quote_ident(dialect, name);
        let value = literal(dialect, value);
        ordered.push(match dialect {
            Dialect::Mysql | Dialect::Mariadb => format!("{column} <=> {value}"),
            Dialect::Postgres => format!("{column} IS NOT DISTINCT FROM {value}"),
            Dialect::Sqlite => format!("{column} IS {value}"),
        });
    }
    Ok(ordered.join(" AND "))
}

pub(crate) fn literal(_dialect: Dialect, value: &ManagedValue) -> String {
    match &value.0 {
        serde_json::Value::Null => "NULL".to_string(),
        serde_json::Value::Bool(value) => if *value { "TRUE" } else { "FALSE" }.to_string(),
        serde_json::Value::Number(value) => value.to_string(),
        serde_json::Value::String(value) => format!("'{}'", value.replace('\'', "''")),
        value @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)) => {
            let encoded = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
            format!("'{}'", encoded.replace('\'', "''"))
        }
    }
}

pub(crate) fn quote_qualified(dialect: Dialect, value: &str) -> String {
    value
        .split('.')
        .map(|part| quote_ident(dialect, part))
        .collect::<Vec<_>>()
        .join(".")
}

pub(crate) fn quote_ident(dialect: Dialect, value: &str) -> String {
    match dialect {
        Dialect::Mysql | Dialect::Mariadb => format!("`{}`", value.replace('`', "``")),
        Dialect::Postgres | Dialect::Sqlite => format!("\"{}\"", value.replace('"', "\"\"")),
    }
}
