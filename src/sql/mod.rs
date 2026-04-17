mod error;
mod function;
mod table;
mod util;

#[cfg(test)]
mod tests;

pub use error::SqlParseError;

use function::parse_create_function;
use sqlparser::ast::{Statement, UserDefinedTypeRepresentation};
use sqlparser::dialect::PostgreSqlDialect;
use sqlparser::parser::Parser;
use table::parse_create_table;
use util::{index_col_name, object_name_parts};

use crate::states::{schema_qualified_key, EnumDef, ExtensionDef, Index, Schema, ViewDef};

struct ParseContext {
    schema: Schema,
    pending_indexes: Vec<(String, Index)>,
}

impl ParseContext {
    fn new() -> Self {
        Self {
            schema: Schema::default(),
            pending_indexes: Vec::new(),
        }
    }

    fn finish(mut self) -> Result<Schema, SqlParseError> {
        for (table_name, index) in self.pending_indexes {
            match self.schema.tables.get_mut(&table_name) {
                Some(table) => table.indexes.push(index),
                None => return Err(SqlParseError::UnknownTable { table: table_name }),
            }
        }
        Ok(self.schema)
    }
}

pub fn parse_sql(sql: &str) -> Result<Schema, SqlParseError> {
    let statements = Parser::parse_sql(&PostgreSqlDialect {}, sql)
        .map_err(|e| SqlParseError::Parse(e.to_string()))?;
    let mut ctx = ParseContext::new();
    for stmt in &statements {
        parse_statement(stmt, &mut ctx)?;
    }
    ctx.finish()
}

pub fn parse_sql_file(path: &std::path::Path) -> Result<Schema, SqlParseError> {
    let sql = std::fs::read_to_string(path).map_err(|e| SqlParseError::Io {
        path: path.display().to_string(),
        source: e,
    })?;
    parse_sql(&sql)
}

fn parse_statement(stmt: &Statement, ctx: &mut ParseContext) -> Result<(), SqlParseError> {
    match stmt {
        Statement::CreateTable(ct) => {
            let (key, tbl) = parse_create_table(ct)?;
            if ctx.schema.tables.contains_key(&key) {
                return Err(SqlParseError::DuplicateTable(key));
            }
            ctx.schema.tables.insert(key, tbl);
        }

        Statement::CreateIndex(ci) => {
            let (table_name, _) = object_name_parts(&ci.table_name);
            let idx_name = ci
                .name
                .as_ref()
                .map(|n| object_name_parts(n).0)
                .unwrap_or_else(|| {
                    let cols: Vec<_> = ci.columns.iter().map(|ic| index_col_name(ic)).collect();
                    format!("{}_{}_idx", table_name, cols.join("_"))
                });
            ctx.pending_indexes.push((
                table_name,
                Index {
                    name: idx_name,
                    columns: ci.columns.iter().map(|ic| index_col_name(ic)).collect(),
                    unique: ci.unique,
                    predicate: ci.predicate.as_ref().map(|e| e.to_string()),
                },
            ));
        }

        Statement::CreateView(cv) => {
            let (name, schema) = object_name_parts(&cv.name);
            let key = schema_qualified_key(&name, schema.as_deref());
            ctx.schema.views.insert(key, ViewDef { name, schema, definition: cv.query.to_string() });
        }

        Statement::CreateExtension(ce) => {
            let name = ce.name.value.clone();
            let schema = ce.schema.as_ref().map(|i| i.value.clone());
            let key = schema_qualified_key(&name, schema.as_deref());
            ctx.schema.extensions.insert(key, ExtensionDef {
                name,
                schema,
                version: ce.version.as_ref().map(|i| i.value.clone()),
            });
        }

        Statement::CreateType { name, representation } => {
            let (type_name, schema) = object_name_parts(name);
            let key = schema_qualified_key(&type_name, schema.as_deref());
            match representation {
                Some(UserDefinedTypeRepresentation::Enum { labels }) => {
                    ctx.schema.enums.insert(key, EnumDef {
                        name: type_name,
                        schema,
                        values: labels.iter().map(|l| l.value.clone()).collect(),
                    });
                }
                other => {
                    return Err(SqlParseError::Unsupported {
                        stmt: format!(
                            "CREATE TYPE {}",
                            other.as_ref().map(|r| r.to_string()).unwrap_or_default()
                        ),
                    });
                }
            }
        }

        Statement::CreateFunction(cf) => {
            let (key, func) = parse_create_function(cf, stmt)?;
            ctx.schema.functions.insert(key, func);
        }

        other => {
            return Err(SqlParseError::Unsupported {
                stmt: other.to_string().chars().take(120).collect(),
            });
        }
    }
    Ok(())
}
