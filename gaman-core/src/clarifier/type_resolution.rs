use std::collections::{HashMap, HashSet};

use crate::clarifier::ids::{clarification_id, is_unknown_type_id};
use crate::clarifier::{
    Answer, Clarification, ClarificationKind, ClarifyError, Decision, Severity,
};
use crate::dialects::Dialect;
use crate::states::{Schema, schema_qualified_key};

#[doc(hidden)]
pub enum TypeResolution {
    Resolved(Schema),
    NeedsInput(Vec<Clarification>),
}

#[doc(hidden)]
pub fn resolve_unknown_types(
    dialect: Dialect,
    mut desired: Schema,
    previous: &Schema,
    decisions: &[Decision],
) -> Result<TypeResolution, ClarifyError> {
    let clarifications = unknown_type_clarifications(dialect, &desired, previous);
    let by_id = clarifications
        .iter()
        .map(|clarification| (clarification.id.clone(), clarification.clone()))
        .collect::<HashMap<_, _>>();

    validate_type_decisions(dialect, &by_id, decisions)?;

    let decided_ids = decisions
        .iter()
        .filter(|decision| is_unknown_type_id(&decision.clarification_id))
        .map(|decision| decision.clarification_id.as_str())
        .collect::<HashSet<_>>();
    let pending = clarifications
        .into_iter()
        .filter(|clarification| !decided_ids.contains(clarification.id.as_str()))
        .collect::<Vec<_>>();
    if !pending.is_empty() {
        return Ok(TypeResolution::NeedsInput(pending));
    }

    for decision in decisions
        .iter()
        .filter(|decision| is_unknown_type_id(&decision.clarification_id))
    {
        let Some(clarification) = by_id.get(&decision.clarification_id) else {
            continue;
        };
        let ClarificationKind::UnknownType { table, column, .. } = &clarification.kind else {
            continue;
        };
        if let Answer::UseType(type_name) = &decision.answer
            && let Some(table) = desired.tables.get_mut(table)
            && let Some(column) = table
                .columns
                .iter_mut()
                .find(|candidate| candidate.name == *column)
        {
            column.col_type = dialect.canonical_type(type_name);
        }
    }

    Ok(TypeResolution::Resolved(desired))
}

#[doc(hidden)]
pub fn non_type_decisions(decisions: &[Decision]) -> Vec<Decision> {
    decisions
        .iter()
        .filter(|decision| !is_unknown_type_id(&decision.clarification_id))
        .cloned()
        .collect()
}

fn unknown_type_clarifications(
    dialect: Dialect,
    desired: &Schema,
    previous: &Schema,
) -> Vec<Clarification> {
    let trusted_types = trusted_project_types(previous);
    let mut clarifications = Vec::new();

    for (table_name, table) in &desired.tables {
        for column in &table.columns {
            if is_type_accepted(dialect, desired, &trusted_types, &column.col_type) {
                continue;
            }
            let kind = ClarificationKind::UnknownType {
                table: table_name.clone(),
                column: column.name.clone(),
                type_name: column.col_type.clone(),
                suggested: dialect.type_suggestions(&column.col_type),
            };
            clarifications.push(Clarification {
                id: clarification_id(&kind),
                severity: Severity::Warning,
                kind,
            });
        }
    }

    clarifications.sort_by(|left, right| left.id.cmp(&right.id));
    clarifications
}

fn validate_type_decisions(
    dialect: Dialect,
    by_id: &HashMap<String, Clarification>,
    decisions: &[Decision],
) -> Result<(), ClarifyError> {
    let mut seen = HashSet::new();
    for decision in decisions
        .iter()
        .filter(|decision| is_unknown_type_id(&decision.clarification_id))
    {
        if !seen.insert(decision.clarification_id.as_str()) {
            return Err(ClarifyError::DuplicateDecision(
                decision.clarification_id.clone(),
            ));
        }
        let clarification = by_id
            .get(&decision.clarification_id)
            .ok_or_else(|| ClarifyError::UnknownDecision(decision.clarification_id.clone()))?;
        validate_type_answer(clarification, &decision.answer, dialect)?;
    }
    Ok(())
}

fn validate_type_answer(
    clarification: &Clarification,
    answer: &Answer,
    dialect: Dialect,
) -> Result<(), ClarifyError> {
    match (&clarification.kind, answer) {
        (ClarificationKind::UnknownType { .. }, Answer::UseType(type_name)) => {
            if type_name.trim().is_empty() {
                return Err(ClarifyError::EmptyInput {
                    id: clarification.id.clone(),
                });
            }
            if !dialect.is_catalog_type(type_name) {
                return Err(ClarifyError::InvalidCandidate {
                    id: clarification.id.clone(),
                    chosen: type_name.clone(),
                });
            }
        }
        (ClarificationKind::UnknownType { .. }, Answer::KeepType) => {}
        _ => {
            return Err(ClarifyError::InvalidAnswer {
                id: clarification.id.clone(),
            });
        }
    }
    Ok(())
}

fn trusted_project_types(schema: &Schema) -> HashSet<String> {
    schema
        .tables
        .values()
        .flat_map(|table| table.columns.iter())
        .map(|column| type_key(&column.col_type))
        .collect()
}

fn is_type_accepted(
    dialect: Dialect,
    desired: &Schema,
    trusted_types: &HashSet<String>,
    col_type: &str,
) -> bool {
    dialect.is_catalog_type(col_type)
        || is_modeled_enum_type(desired, col_type)
        || is_trusted_type(trusted_types, col_type)
}

fn is_modeled_enum_type(schema: &Schema, col_type: &str) -> bool {
    let key = type_key(col_type);
    schema.enums.values().any(|enum_def| {
        key == type_key(&enum_def.name)
            || key == type_key(&enum_def.qualified_name())
            || key
                == type_key(&schema_qualified_key(
                    &enum_def.name,
                    enum_def.schema.as_deref(),
                ))
    })
}

fn is_trusted_type(trusted_types: &HashSet<String>, col_type: &str) -> bool {
    let key = type_key(col_type);
    trusted_types.contains(&key)
        || key
            .strip_suffix("[]")
            .is_some_and(|base| trusted_types.contains(base))
}

fn type_key(col_type: &str) -> String {
    col_type
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{Column, EnumDef, Table};

    fn table(name: &str, col_type: &str) -> Table {
        Table {
            name: name.to_string(),
            schema: None,
            primary_key: None,
            columns: vec![Column {
                name: "value".to_string(),
                col_type: col_type.to_string(),
                nullable: true,
                ..Default::default()
            }],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    }

    fn schema_with_type(col_type: &str) -> Schema {
        let mut schema = Schema::default();
        schema
            .tables
            .insert("items".to_string(), table("items", col_type));
        schema
    }

    #[test]
    fn new_unknown_type_needs_input() {
        let desired = schema_with_type("my_domain");
        let result =
            resolve_unknown_types(Dialect::Postgres, desired, &Schema::default(), &[]).unwrap();
        let TypeResolution::NeedsInput(clarifications) = result else {
            panic!("expected unknown type clarification");
        };
        assert_eq!(clarifications.len(), 1);
        assert_eq!(clarifications[0].id, "unknown_type:items:value");
    }

    #[test]
    fn replayed_unknown_type_is_trusted() {
        let previous = schema_with_type("my_domain");
        let desired = schema_with_type("my_domain");
        let result = resolve_unknown_types(Dialect::Postgres, desired, &previous, &[]).unwrap();
        assert!(matches!(result, TypeResolution::Resolved(_)));
    }

    #[test]
    fn modeled_enum_type_is_accepted() {
        let mut desired = schema_with_type("status");
        desired.enums.insert(
            "status".to_string(),
            EnumDef {
                name: "status".to_string(),
                schema: None,
                values: vec!["active".to_string()],
            },
        );
        let result =
            resolve_unknown_types(Dialect::Postgres, desired, &Schema::default(), &[]).unwrap();
        assert!(matches!(result, TypeResolution::Resolved(_)));
    }

    #[test]
    fn suggested_type_rewrites_schema() {
        let desired = schema_with_type("intger");
        let decisions = vec![Decision {
            clarification_id: "unknown_type:items:value".to_string(),
            answer: Answer::UseType("integer".to_string()),
        }];
        let result =
            resolve_unknown_types(Dialect::Postgres, desired, &Schema::default(), &decisions)
                .unwrap();
        let TypeResolution::Resolved(schema) = result else {
            panic!("expected resolved schema");
        };
        assert_eq!(schema.tables["items"].columns[0].col_type, "integer");
    }

    #[test]
    fn use_type_accepts_catalog_type_even_without_suggestion() {
        let desired = schema_with_type("my_domain");
        let decisions = vec![Decision {
            clarification_id: "unknown_type:items:value".to_string(),
            answer: Answer::UseType("text".to_string()),
        }];
        let result =
            resolve_unknown_types(Dialect::Postgres, desired, &Schema::default(), &decisions)
                .unwrap();
        let TypeResolution::Resolved(schema) = result else {
            panic!("expected resolved schema");
        };
        assert_eq!(schema.tables["items"].columns[0].col_type, "text");
    }
}
