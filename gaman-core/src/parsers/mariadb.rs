//! MariaDB AST lowering with MariaDB-owned option interpretation.

use sqlparser::ast::Statement;

use super::{common, error::ParseError, mysql_family, sql::ParseContext};
use crate::dialects::Dialect;

/// Lowers one MariaDB statement into modeled or opaque-capable schema state.
pub(super) fn lower_statement(
    statement: &Statement,
    ctx: &mut ParseContext,
) -> Result<(), ParseError> {
    match statement {
        Statement::CreateTable(create) => {
            let (key, mut table) = common::parse_create_table(create, Dialect::Mariadb)?;
            mysql_family::apply_column_options(create, &mut table, Dialect::Mariadb);
            ctx.insert_table((key, table))
        }
        Statement::CreateIndex(index) => {
            ctx.push_index(common::parse_create_index(index));
            Ok(())
        }
        Statement::CreateView(view) => super::mysql::lower_view(view, ctx),
        _ => Err(common::unsupported_statement(
            Dialect::Mariadb,
            statement,
            "this MariaDB CREATE statement is preserved through opaque fallback",
        )),
    }
}
