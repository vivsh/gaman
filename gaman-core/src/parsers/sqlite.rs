use sqlparser::ast::Statement;

use super::common::{
    object_name_parts, parse_create_index, parse_create_table, unsupported_statement,
};
use super::error::ParseError;
use super::postgres::{trigger_events, trigger_scope, trigger_timing};
use super::sql::ParseContext;
use crate::dialects::Dialect;
use crate::states::{TriggerDef, ViewDef, schema_qualified_key};

pub(super) fn lower_statement(stmt: &Statement, ctx: &mut ParseContext) -> Result<(), ParseError> {
    match stmt {
        Statement::CreateTable(ct) => ctx.insert_table(parse_create_table(ct)?),
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
                },
            );
            Ok(())
        }
        Statement::CreateTrigger(trigger) => lower_create_trigger(trigger, ctx),
        other => Err(unsupported_statement(
            Dialect::Sqlite,
            other,
            "statement is not represented in Schema for SQLite",
        )),
    }
}

fn lower_create_trigger(
    trigger: &sqlparser::ast::CreateTrigger,
    ctx: &mut ParseContext,
) -> Result<(), ParseError> {
    let (table_name, schema) = object_name_parts(&trigger.table_name);
    let table_key = schema_qualified_key(&table_name, schema.as_deref());
    let table =
        ctx.schema
            .tables
            .get_mut(&table_key)
            .ok_or_else(|| ParseError::UnknownTriggerTable {
                table: table_key.clone(),
            })?;
    let query = trigger.statements.as_ref().map(ToString::to_string);
    if query.is_none() {
        return Err(ParseError::unsupported(
            Dialect::Sqlite,
            trigger.to_string(),
            "SQLite triggers require a statement body",
        ));
    }
    table.triggers.push(TriggerDef {
        name: Some(object_name_parts(&trigger.name).0),
        timing: trigger_timing(trigger.period),
        events: trigger_events(&trigger.events),
        scope: trigger_scope(trigger.trigger_object),
        function_name: None,
        when: trigger.condition.as_ref().map(ToString::to_string),
        query,
        language: Some("sql".to_string()),
    });
    Ok(())
}
