use sqlparser::ast::{DataType, Expr, IndexColumn, ObjectName};

pub(crate) fn data_type_to_str(dt: &DataType) -> String {
    dt.to_string().to_lowercase()
}

pub(crate) fn object_name_parts(name: &ObjectName) -> (String, Option<String>) {
    let parts: Vec<&str> = name
        .0
        .iter()
        .map(|p| p.as_ident().map(|i| i.value.as_str()).unwrap_or(""))
        .collect();
    match parts.as_slice() {
        [schema, tbl] if !schema.is_empty() && *schema != "public" => {
            (tbl.to_string(), Some(schema.to_string()))
        }
        [_, tbl] => (tbl.to_string(), None),
        [tbl] => (tbl.to_string(), None),
        _ => (name.to_string(), None),
    }
}

pub(crate) fn index_col_name(ic: &IndexColumn) -> String {
    match &ic.column.expr {
        Expr::Identifier(ident) => ident.value.clone(),
        other => other.to_string(),
    }
}

pub(crate) fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            sqlparser::ast::Value::SingleQuotedString(s)
            | sqlparser::ast::Value::DollarQuotedString(
                sqlparser::ast::DollarQuotedString { value: s, .. },
            ) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}
