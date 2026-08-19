//! Shared live-schema selection and authored-export validation.

use std::collections::{BTreeMap, BTreeSet};

use crate::dialects::Dialect;
use crate::states::{EntityKind, Schema};

use super::CommandError;
pub use crate::entity_selector::EntityFilter;

impl EntityFilter {
    /// Parses one `[kind:]glob` selector, defaulting to table roots.
    pub fn parse(value: &str) -> Result<Self, CommandError> {
        crate::entity_selector::EntitySelector::parse_filter(value).map_err(|reason| {
            CommandError::Invalid(reason.replace("entity selector kind", "entity filter kind"))
        })
    }

    /// Reports whether this filter selects one canonical root identity.
    pub fn matches(&self, kind: EntityKind, identity: &str) -> bool {
        self.kind == kind && matches_identity(&self.pattern, identity)
    }
}

/// Selects root entities and converts the result into authored-schema-safe state.
pub(crate) fn select_authored_schema(
    schema: Schema,
    filters: &[EntityFilter],
    dialect: Dialect,
) -> Result<Schema, CommandError> {
    let selected = select_schema(schema, filters)?;
    ensure_authored_exportable(&selected)?;
    let yaml = serde_yaml::to_string(&selected).map_err(|error| {
        CommandError::Invalid(format!("cannot encode inspected schema: {error}"))
    })?;
    Schema::from_yaml_str(&yaml, dialect).map_err(|error| {
        CommandError::Invalid(format!(
            "inspected schema cannot be exported as authored YAML: {error}"
        ))
    })
}

/// Returns the selected root entities without changing their internal inspection metadata.
pub(crate) fn select_schema(
    schema: Schema,
    filters: &[EntityFilter],
) -> Result<Schema, CommandError> {
    select_schema_with_match_requirement(schema, filters, true)
}

/// Selects migration-owned roots for drift while retaining missing live entities as drift input.
pub(crate) fn select_schema_for_drift(
    schema: Schema,
    filters: &[EntityFilter],
) -> Result<Schema, CommandError> {
    select_schema_with_match_requirement(schema, filters, false)
}

/// Selects roots with caller-controlled handling for a missing matching identity.
fn select_schema_with_match_requirement(
    schema: Schema,
    filters: &[EntityFilter],
    require_match: bool,
) -> Result<Schema, CommandError> {
    if filters.is_empty() {
        return Ok(schema);
    }
    let mut selected = Schema::default();
    for filter in filters {
        match filter.kind {
            EntityKind::Table => {
                copy_matches(
                    &schema.tables,
                    &mut selected.tables,
                    filter,
                    |table| table.qualified_name(),
                    require_match,
                )?;
                selected.managed_rows.extend(
                    schema
                        .managed_rows
                        .iter()
                        .filter(|(table, _)| matches_identity(&filter.pattern, table))
                        .map(|(table, rows)| (table.clone(), rows.clone())),
                );
            }
            EntityKind::Function => copy_matches(
                &schema.functions,
                &mut selected.functions,
                filter,
                |function| function.qualified_name(),
                require_match,
            )?,
            EntityKind::View => copy_matches(
                &schema.views,
                &mut selected.views,
                filter,
                |view| view.qualified_name(),
                require_match,
            )?,
            EntityKind::Enum => copy_matches(
                &schema.enums,
                &mut selected.enums,
                filter,
                |enum_def| enum_def.qualified_name(),
                require_match,
            )?,
            EntityKind::Extension => copy_matches(
                &schema.extensions,
                &mut selected.extensions,
                filter,
                |extension| extension.qualified_name(),
                require_match,
            )?,
            _ => {
                return Err(CommandError::Invalid(format!(
                    "'{:?}' is not a root inspection filter kind",
                    filter.kind
                )));
            }
        }
    }
    Ok(selected)
}

/// Copies one filter's matching entries and rejects ambiguous unqualified identities.
fn copy_matches<T: Clone>(
    source: &BTreeMap<String, T>,
    destination: &mut BTreeMap<String, T>,
    filter: &EntityFilter,
    identity: impl Fn(&T) -> String,
    require_match: bool,
) -> Result<(), CommandError> {
    let matches = source
        .iter()
        .filter(|(_, value)| matches_identity(&filter.pattern, &identity(value)))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<Vec<_>>();
    if matches.is_empty() && require_match {
        return Err(CommandError::Invalid(format!(
            "inspection filter '{}:{}' matched no entities",
            kind_name(filter.kind),
            filter.pattern
        )));
    }
    reject_ambiguous_unqualified(&matches, filter, &identity)?;
    destination.extend(matches);
    Ok(())
}

/// Matches a canonical identity and accepts `public.` as a PostgreSQL default-schema alias.
fn matches_identity(pattern: &str, identity: &str) -> bool {
    glob_matches(pattern, identity)
        || (!identity.contains('.') && glob_matches(pattern, &format!("public.{identity}")))
}

/// Rejects a selector that could name more than one schema-qualified entity.
fn reject_ambiguous_unqualified<T>(
    matches: &[(String, T)],
    filter: &EntityFilter,
    identity: &impl Fn(&T) -> String,
) -> Result<(), CommandError> {
    if filter.pattern.contains('.') {
        return Ok(());
    }
    let names = matches
        .iter()
        .map(|(_, value)| {
            identity(value)
                .rsplit('.')
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect::<BTreeSet<_>>();
    if names.len() != matches.len() {
        return Err(CommandError::Invalid(format!(
            "unqualified inspection filter '{}:{}' is ambiguous; use a qualified identity",
            kind_name(filter.kind),
            filter.pattern
        )));
    }
    Ok(())
}

/// Ensures no internal lifecycle metadata would be lost in an authored export.
fn ensure_authored_exportable(schema: &Schema) -> Result<(), CommandError> {
    for (name, table) in &schema.tables {
        if table.has_unmanaged_options() {
            return export_error("table", name, "unmanaged table options");
        }
        if table.indexes.iter().any(|index| index.is_opaque())
            || table
                .constraints
                .iter()
                .any(|constraint| constraint.is_opaque())
            || table.triggers.iter().any(|trigger| trigger.is_opaque())
        {
            return export_error("table", name, "opaque owned entity");
        }
    }
    if schema.functions.values().any(|value| value.is_opaque())
        || schema.views.values().any(|value| value.is_opaque())
        || schema.enums.values().any(|value| value.is_opaque())
        || schema.extensions.values().any(|value| value.is_opaque())
    {
        return Err(CommandError::Invalid(
            "inspected schema contains opaque root entities and cannot be exported as authored YAML"
                .to_string(),
        ));
    }
    Ok(())
}

/// Returns one consistent export-boundary error.
fn export_error(kind: &str, name: &str, detail: &str) -> Result<(), CommandError> {
    Err(CommandError::Invalid(format!(
        "cannot export {kind} '{name}' as authored YAML because it contains {detail}"
    )))
}

/// Parses only the root kinds that are meaningful for catalog selection.
/// Returns the CLI spelling for one root entity kind.
fn kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Table => "table",
        EntityKind::Function => "function",
        EntityKind::View => "view",
        EntityKind::Enum => "enum",
        EntityKind::Extension => "extension",
        _ => "entity",
    }
}

/// Matches `*` and `?` glob syntax without treating names as filesystem paths.
fn glob_matches(pattern: &str, value: &str) -> bool {
    let pattern = pattern.chars().collect::<Vec<_>>();
    let value = value.chars().collect::<Vec<_>>();
    let mut previous = vec![false; value.len() + 1];
    previous[0] = true;
    for token in pattern {
        let mut current = vec![false; value.len() + 1];
        match token {
            '*' => {
                current[0] = previous[0];
                for index in 1..=value.len() {
                    current[index] = previous[index] || current[index - 1];
                }
            }
            '?' => {
                current[1..].copy_from_slice(&previous[..value.len()]);
            }
            literal => {
                for index in 1..=value.len() {
                    current[index] = previous[index - 1] && value[index - 1] == literal;
                }
            }
        }
        previous = current;
    }
    previous[value.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{Column, Index, Table};

    /// Verifies untyped filters select table roots by default.
    #[test]
    fn default_filter_selects_tables() {
        let filter = EntityFilter::parse("user*").expect("parse filter");
        assert_eq!(filter.kind, EntityKind::Table);
        assert!(glob_matches(&filter.pattern, "users"));
        assert!(matches_identity("public.users", "users"));
    }

    /// Verifies unsupported filter kinds fail before catalog selection.
    #[test]
    fn unsupported_filter_kind_is_rejected() {
        let error = EntityFilter::parse("index:users_name").expect_err("reject index filter");
        assert!(error.to_string().contains("unknown entity filter kind"));
    }

    /// Verifies exported inspection excludes no selected modeled table data.
    #[test]
    fn selected_modeled_table_is_authored_exportable() {
        let mut schema = Schema::default();
        schema.tables.insert(
            "users".to_string(),
            Table {
                name: "users".to_string(),
                schema: None,
                primary_key: None,
                columns: vec![Column {
                    name: "id".to_string(),
                    col_type: "integer".to_string(),
                    ..Default::default()
                }],
                foreign_keys: Vec::new(),
                indexes: Vec::new(),
                constraints: Vec::new(),
                triggers: Vec::new(),
                options: Default::default(),
            },
        );
        let result = select_authored_schema(
            schema,
            &[EntityFilter::parse("users").expect("parse filter")],
            Dialect::Postgres,
        )
        .expect("authored export");
        assert!(result.tables.contains_key("users"));
    }

    /// Verifies opaque inspection data cannot silently become authored YAML.
    #[test]
    fn opaque_index_blocks_authored_export() {
        let mut schema = Schema::default();
        schema.tables.insert(
            "users".to_string(),
            Table {
                name: "users".to_string(),
                schema: None,
                primary_key: None,
                columns: Vec::new(),
                foreign_keys: Vec::new(),
                indexes: vec![Index::from_trusted_raw(
                    "users_expression_idx",
                    "CREATE INDEX users_expression_idx ON users ((lower(email)))",
                )],
                constraints: Vec::new(),
                triggers: Vec::new(),
                options: Default::default(),
            },
        );
        let error = select_authored_schema(
            schema,
            &[EntityFilter::parse("users").expect("parse filter")],
            Dialect::Postgres,
        )
        .expect_err("opaque data must fail authored export");
        assert!(error.to_string().contains("opaque owned entity"));
    }
}
