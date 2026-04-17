use sqlparser::ast::{ColumnOption, CreateTable, TableConstraint};

use super::error::SqlParseError;
use super::util::{data_type_to_str, index_col_name, object_name_parts};
use crate::states::{schema_qualified_key, Column, Constraint, ForeignKey, Table};

pub(super) fn parse_create_table(ct: &CreateTable) -> Result<(String, Table), SqlParseError> {
    let (name, schema) = object_name_parts(&ct.name);
    let key = schema_qualified_key(&name, schema.as_deref());

    let mut table = Table {
        name: name.clone(),
        schema,
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
            nullable: true,  // PostgreSQL default: nullable unless NOT NULL is specified
            default: None,
            primary_key: false,
            references: None,
            check: None,
        };

        for opt in &col_def.options {
            match &opt.option {
                ColumnOption::Null => col.nullable = true,
                ColumnOption::NotNull => col.nullable = false,
                ColumnOption::Default(expr) => col.default = Some(expr.to_string()),
                ColumnOption::PrimaryKey(_) => {
                    col.primary_key = true;
                    col.nullable = false;  // PRIMARY KEY implies NOT NULL
                }
                ColumnOption::Unique(u) => {
                    let cname = u
                        .name
                        .as_ref()
                        .map(|n| n.value.clone())
                        .unwrap_or_else(|| format!("{}_{}_key", name, col_name));
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
                        .unwrap_or_else(|| format!("{}_{}_fkey", name, col_name));
                    let (to_table, _) = object_name_parts(&fk.foreign_table);
                    let to_column = fk
                        .referred_columns
                        .first()
                        .map(|i| i.value.clone())
                        .unwrap_or_default();
                    table.foreign_keys.push(ForeignKey {
                        name: fk_name,
                        from_column: col_name.clone(),
                        to_table,
                        to_column,
                    });
                }
                ColumnOption::Check(chk) => {
                    let cname = chk
                        .name
                        .as_ref()
                        .map(|n| n.value.clone())
                        .unwrap_or_else(|| format!("{}_{}_check", name, col_name));
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

fn apply_table_constraint(tc: &TableConstraint, table_name: &str, table: &mut Table) {
    match tc {
        TableConstraint::PrimaryKey(pk) => {
            let pk_cols: Vec<String> = pk.columns.iter().map(|ic| index_col_name(ic)).collect();
            for col in table.columns.iter_mut() {
                if pk_cols.contains(&col.name) {
                    col.primary_key = true;
                    col.nullable = false;  // PRIMARY KEY implies NOT NULL
                }
            }
        }
        TableConstraint::Unique(u) => {
            let cname = u
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .unwrap_or_else(|| {
                    let cols: Vec<_> = u.columns.iter().map(|ic| index_col_name(ic)).collect();
                    format!("{}_{}_key", table_name, cols.join("_"))
                });
            let columns: Vec<String> = u.columns.iter().map(|ic| index_col_name(ic)).collect();
            table.constraints.push(Constraint::Unique { name: cname, columns });
        }
        TableConstraint::ForeignKey(fk) => {
            let fk_name = fk
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .unwrap_or_else(|| {
                    let from = fk.columns.first().map(|i| i.value.as_str()).unwrap_or("col");
                    format!("{}_{}_fkey", table_name, from)
                });
            let (to_table, _) = object_name_parts(&fk.foreign_table);
            let from_column = fk.columns.first().map(|i| i.value.clone()).unwrap_or_default();
            let to_column = fk
                .referred_columns
                .first()
                .map(|i| i.value.clone())
                .unwrap_or_default();
            table.foreign_keys.push(ForeignKey {
                name: fk_name,
                from_column,
                to_table,
                to_column,
            });
        }
        TableConstraint::Check(chk) => {
            let cname = chk
                .name
                .as_ref()
                .map(|n| n.value.clone())
                .unwrap_or_else(|| format!("{}_check", table_name));
            table.constraints.push(Constraint::Check {
                name: cname,
                expression: chk.expr.to_string(),
            });
        }
        _ => {}
    }
}
