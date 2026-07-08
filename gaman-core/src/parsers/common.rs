use sqlparser::ast::{
    ColumnOption, CreateIndex, CreateTable, DataType, Expr, IndexColumn, ObjectName,
    TableConstraint,
};

use super::error::ParseError;
use crate::dialects::Dialect;
use crate::states::{
    Column, Constraint, ForeignKey, Index, PrimaryKey, Table, names, schema_qualified_key,
};

pub(super) fn data_type_to_str(dt: &DataType) -> String {
    dt.to_string().to_lowercase()
}

pub(super) fn object_name_parts(name: &ObjectName) -> (String, Option<String>) {
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

pub(super) fn index_col_name(ic: &IndexColumn) -> String {
    match &ic.column.expr {
        Expr::Identifier(ident) => ident.value.clone(),
        other => other.to_string(),
    }
}

pub(super) fn extract_string_literal(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Value(v) => match &v.value {
            sqlparser::ast::Value::SingleQuotedString(s)
            | sqlparser::ast::Value::DollarQuotedString(sqlparser::ast::DollarQuotedString {
                value: s,
                ..
            }) => Some(s.clone()),
            _ => None,
        },
        _ => None,
    }
}

pub(super) fn parse_create_table(ct: &CreateTable) -> Result<(String, Table), ParseError> {
    let (name, schema) = object_name_parts(&ct.name);
    let key = schema_qualified_key(&name, schema.as_deref());

    let mut table = Table {
        name: name.clone(),
        schema,
        primary_key: None,
        columns: Vec::new(),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        constraints: Vec::new(),
        triggers: Vec::new(),
    };

    for col_def in &ct.columns {
        let col_name = col_def.name.value.clone();
        let mut col = Column {
            name: col_name.clone(),
            col_type: data_type_to_str(&col_def.data_type),
            nullable: true,
            default: None,
            primary_key: false,
            references: None,
            check: None,
            generated: None,
        };

        for opt in &col_def.options {
            match &opt.option {
                ColumnOption::Null => col.nullable = true,
                ColumnOption::NotNull => col.nullable = false,
                ColumnOption::Default(expr) => col.default = Some(expr.to_string()),
                ColumnOption::Generated {
                    generation_expr, ..
                } => col.generated = generation_expr.as_ref().map(ToString::to_string),
                ColumnOption::Identity(_) => {
                    col.default.get_or_insert_with(|| "identity".to_string());
                    col.nullable = false;
                }
                ColumnOption::PrimaryKey(_) => {
                    col.primary_key = true;
                    col.nullable = false;
                }
                ColumnOption::Unique(u) => {
                    let cname = u
                        .name
                        .as_ref()
                        .map(|n| n.value.clone())
                        .unwrap_or_else(|| names::unique(&name, &[col_name.as_str()]));
                    table.constraints.push(Constraint::Unique {
                        name: cname,
                        columns: vec![col_name.clone()],
                    });
                }
                ColumnOption::ForeignKey(fk) => {
                    let fk_name = fk
                        .name
                        .as_ref()
                        .map(|n| n.value.clone())
                        .unwrap_or_else(|| names::foreign_key(&name, &[col_name.as_str()]));
                    let (to_table_name, to_schema) = object_name_parts(&fk.foreign_table);
                    let to_table = schema_qualified_key(&to_table_name, to_schema.as_deref());
                    let to_column = fk
                        .referred_columns
                        .first()
                        .map(|i| i.value.clone())
                        .unwrap_or_default();
                    table.foreign_keys.push(ForeignKey::single(
                        fk_name,
                        col_name.clone(),
                        to_table,
                        to_column,
                    ));
                }
                ColumnOption::Check(chk) => {
                    let cname = chk
                        .name
                        .as_ref()
                        .map(|n| n.value.clone())
                        .unwrap_or_else(|| names::column_check(&name, &col_name));
                    table.constraints.push(Constraint::Check {
                        name: cname,
                        expression: chk.expr.to_string(),
                    });
                }
                _ => {}
            }
        }
        table.columns.push(col);
    }

    for tc in &ct.constraints {
        apply_table_constraint(tc, &name, &mut table);
    }

    Ok((key, table))
}

pub(super) fn parse_create_index(ci: &CreateIndex) -> (String, Index) {
    let (table_name, _) = object_name_parts(&ci.table_name);
    let idx_name = ci
        .name
        .as_ref()
        .map(|n| object_name_parts(n).0)
        .unwrap_or_else(|| {
            let cols: Vec<_> = ci.columns.iter().map(index_col_name).collect();
            names::index(&table_name, &cols)
        });
    (
        table_name,
        Index {
            name: idx_name,
            columns: ci.columns.iter().map(index_col_name).collect(),
            unique: ci.unique,
            predicate: ci.predicate.as_ref().map(|e| e.to_string()),
        },
    )
}

pub(super) fn unsupported_statement(
    dialect: Dialect,
    stmt: &sqlparser::ast::Statement,
    reason: &str,
) -> ParseError {
    ParseError::unsupported(
        dialect,
        stmt.to_string().chars().take(120).collect::<String>(),
        reason,
    )
}

fn apply_table_constraint(tc: &TableConstraint, table_name: &str, table: &mut Table) {
    match tc {
        TableConstraint::PrimaryKey(pk) => {
            let pk_cols: Vec<String> = pk.columns.iter().map(index_col_name).collect();
            let pk_name = pk
                .name
                .as_ref()
                .map(|name| name.value.clone())
                .unwrap_or_else(|| names::primary_key(table_name));
            table.primary_key = Some(PrimaryKey {
                name: pk_name,
                columns: pk_cols.clone(),
            });
            for col in table.columns.iter_mut() {
                if pk_cols.contains(&col.name) {
                    col.primary_key = true;
                    col.nullable = false;
                }
            }
        }
        TableConstraint::Unique(u) => {
            let cname = u.name.as_ref().map(|n| n.value.clone()).unwrap_or_else(|| {
                let cols: Vec<_> = u.columns.iter().map(index_col_name).collect();
                names::unique(table_name, &cols)
            });
            let columns: Vec<String> = u.columns.iter().map(index_col_name).collect();
            table.constraints.push(Constraint::Unique {
                name: cname,
                columns,
            });
        }
        TableConstraint::ForeignKey(fk) => {
            let fk_name = fk
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .unwrap_or_else(|| {
                    let columns: Vec<_> = fk.columns.iter().map(|i| i.value.as_str()).collect();
                    if columns.is_empty() {
                        names::foreign_key(table_name, &["col"])
                    } else {
                        names::foreign_key(table_name, &columns)
                    }
                });
            let (to_table_name, to_schema) = object_name_parts(&fk.foreign_table);
            let to_table = schema_qualified_key(&to_table_name, to_schema.as_deref());
            let from_columns = fk.columns.iter().map(|i| i.value.clone());
            let to_columns = fk.referred_columns.iter().map(|i| i.value.clone());
            table
                .foreign_keys
                .push(ForeignKey::new(fk_name, from_columns, to_table, to_columns));
        }
        TableConstraint::Check(chk) => {
            let cname = chk
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .unwrap_or_else(|| names::table_check(table_name));
            table.constraints.push(Constraint::Check {
                name: cname,
                expression: chk.expr.to_string(),
            });
        }
        _ => {}
    }
}
