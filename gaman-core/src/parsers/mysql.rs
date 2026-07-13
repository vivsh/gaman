//! MySQL-family AST lowering into Gaman schema state.

use sqlparser::ast::Statement;

use super::error::ParseError;
use super::sql::ParseContext;
use super::{common, mysql_family};
use crate::dialects::Dialect;

pub(super) fn lower_statement(
    statement: &Statement,
    ctx: &mut ParseContext,
    dialect: Dialect,
) -> Result<(), ParseError> {
    match statement {
        Statement::CreateTable(table) => {
            let (key, mut lowered) = common::parse_create_table(table, dialect)?;
            mysql_family::apply_column_options(table, &mut lowered, Dialect::Mysql);
            ctx.insert_table((key, lowered))
        }
        Statement::CreateIndex(index) => {
            ctx.push_index(common::parse_create_index(index));
            Ok(())
        }
        Statement::CreateView(view) => lower_view(view, ctx),
        _ => Err(common::unsupported_statement(
            dialect,
            statement,
            "this MySQL CREATE statement is preserved through opaque fallback",
        )),
    }
}

/// Lowers the common modeled portion of a MySQL-family view.
pub(super) fn lower_view(
    view: &sqlparser::ast::CreateView,
    ctx: &mut ParseContext,
) -> Result<(), ParseError> {
    let (name, schema) = common::object_name_parts(&view.name);
    let key = crate::states::schema_qualified_key(&name, schema.as_deref());
    ctx.schema.views.insert(
        key,
        crate::states::ViewDef {
            name,
            schema,
            definition: view.query.to_string(),
            opaque: Default::default(),
        },
    );
    Ok(())
}
