use std::collections::{HashMap, HashSet};

use crate::operations::Operation;
use crate::states::EnumDef;

use super::analyze::all_clarifications_raw;
use super::model::{
    Answer, Clarification, ClarificationKind, Decision, DisambiguationResult, DisambiguatorError,
};
use super::types::normalize_type;

pub(crate) struct DisambiguationPlan {
    clarifications: Vec<Clarification>,
    by_id: HashMap<String, usize>,
}

impl DisambiguationPlan {
    pub(crate) fn new(ops: &[Operation]) -> Self {
        let clarifications = all_clarifications_raw(ops);
        let by_id = clarifications
            .iter()
            .enumerate()
            .map(|(idx, clarification)| (clarification.id.clone(), idx))
            .collect();
        Self {
            clarifications,
            by_id,
        }
    }

    pub(crate) fn process(
        &self,
        ops: &[Operation],
        decisions: &[Decision],
    ) -> Result<DisambiguationResult, DisambiguatorError> {
        self.validate_decisions(decisions)?;
        let pending = self.pending_clarifications(decisions);
        if pending.is_empty() {
            Ok(DisambiguationResult::Resolved(
                self.apply_decisions(ops, decisions),
            ))
        } else {
            Ok(DisambiguationResult::NeedsInput(pending))
        }
    }

    fn validate_decisions(&self, decisions: &[Decision]) -> Result<(), DisambiguatorError> {
        let mut seen = HashSet::new();
        for decision in decisions {
            if !seen.insert(decision.clarification_id.as_str()) {
                return Err(DisambiguatorError::DuplicateDecision(
                    decision.clarification_id.clone(),
                ));
            }

            let clar = self
                .clarification(&decision.clarification_id)
                .ok_or_else(|| {
                    DisambiguatorError::UnknownDecision(decision.clarification_id.clone())
                })?;
            validate_answer(clar, &decision.answer)?;
        }
        self.validate_rename_claims(decisions)
    }

    fn validate_rename_claims(&self, decisions: &[Decision]) -> Result<(), DisambiguatorError> {
        let mut claims: HashMap<(String, String), String> = HashMap::new();

        for decision in decisions {
            let Answer::RenameTo(name) = &decision.answer else {
                continue;
            };
            let clar = self
                .clarification(&decision.clarification_id)
                .expect("decisions are validated before claim validation");
            let Some((scope, target)) = rename_claim(clar, name) else {
                continue;
            };

            if let Some(previous) = claims.insert((scope.clone(), target.clone()), clar.id.clone())
                && previous != clar.id
            {
                return Err(DisambiguatorError::ConflictingRenameTarget { scope, target });
            }
        }

        Ok(())
    }

    fn pending_clarifications(&self, decisions: &[Decision]) -> Vec<Clarification> {
        let decided_ids: HashSet<&str> = decisions
            .iter()
            .map(|d| d.clarification_id.as_str())
            .collect();
        let mut claimed_columns: HashSet<(String, String)> = HashSet::new();
        let mut claimed_tables: HashSet<String> = HashSet::new();
        let mut claimed_enum_values: HashSet<(String, String)> = HashSet::new();

        for decision in decisions {
            if let Answer::RenameTo(name) = &decision.answer
                && let Some(clar) = self.clarification(&decision.clarification_id)
            {
                match &clar.kind {
                    ClarificationKind::RenameColumn { table, .. } => {
                        claimed_columns.insert((table.clone(), name.clone()));
                    }
                    ClarificationKind::RenameTable { .. } => {
                        claimed_tables.insert(name.clone());
                    }
                    ClarificationKind::RenameEnumValue { enum_name, .. } => {
                        claimed_enum_values.insert((enum_name.clone(), name.clone()));
                    }
                    _ => {}
                }
            }
        }

        self.clarifications
            .iter()
            .filter(|c| !decided_ids.contains(c.id.as_str()))
            .filter_map(|c| {
                filter_claimed_candidates(
                    c,
                    &claimed_columns,
                    &claimed_tables,
                    &claimed_enum_values,
                )
            })
            .collect()
    }

    fn apply_decisions(&self, ops: &[Operation], decisions: &[Decision]) -> Vec<Operation> {
        let decision_map: HashMap<&str, &Answer> = decisions
            .iter()
            .map(|d| (d.clarification_id.as_str(), &d.answer))
            .collect();

        let mut col_renames: HashMap<(&str, &str), &str> = HashMap::new();
        let mut rename_col_targets: HashSet<(&str, &str)> = HashSet::new();
        let mut table_renames: HashMap<&str, &str> = HashMap::new();
        let mut rename_table_targets: HashSet<&str> = HashSet::new();
        let mut enum_value_renames: HashMap<(&str, &str), &str> = HashMap::new();
        let mut enum_create_by_key: HashMap<String, &EnumDef> = HashMap::new();

        for op in ops {
            if let Operation::CreateEnum { enum_def } = op {
                enum_create_by_key.insert(enum_def.qualified_name(), enum_def);
            }
        }

        for decision in decisions {
            if let Answer::RenameTo(new_name) = &decision.answer
                && let Some(clar) = self.clarification(&decision.clarification_id)
            {
                match &clar.kind {
                    ClarificationKind::RenameColumn { table, old, .. } => {
                        col_renames.insert((table.as_str(), old.as_str()), new_name.as_str());
                        rename_col_targets.insert((table.as_str(), new_name.as_str()));
                    }
                    ClarificationKind::RenameTable { old, .. } => {
                        table_renames.insert(old.as_str(), new_name.as_str());
                        rename_table_targets.insert(new_name.as_str());
                    }
                    ClarificationKind::RenameEnumValue { enum_name, old, .. } => {
                        enum_value_renames
                            .insert((enum_name.as_str(), old.as_str()), new_name.as_str());
                    }
                    _ => {}
                }
            }
        }

        let mut result = Vec::with_capacity(ops.len());
        let mut replaced_enums: HashSet<String> = HashSet::new();

        for op in ops {
            match op {
                Operation::DropColumn {
                    table_name, column, ..
                } => {
                    if let Some(new_name) =
                        col_renames.get(&(table_name.as_str(), column.name.as_str()))
                    {
                        result.push(Operation::RenameColumn {
                            table_name: table_name.clone(),
                            old_name: column.name.clone(),
                            new_name: new_name.to_string(),
                        });
                    } else {
                        result.push(op.clone());
                    }
                }
                Operation::AddColumn { table_name, column } => {
                    if rename_col_targets.contains(&(table_name.as_str(), column.name.as_str())) {
                        continue;
                    }
                    let id = super::ids::clarification_id(&ClarificationKind::NotNullAdd {
                        table: table_name.clone(),
                        column: column.name.clone(),
                        col_type: column.col_type.clone(),
                    });
                    let mut col = column.clone();
                    if let Some(answer) = decision_map.get(id.as_str()) {
                        match answer {
                            Answer::NotNullDefault(val) => col.default = Some(val.clone()),
                            Answer::NotNullNullable => col.nullable = true,
                            _ => {}
                        }
                    }
                    result.push(Operation::AddColumn {
                        table_name: table_name.clone(),
                        column: col,
                    });
                }
                Operation::DropTable { table } => {
                    if let Some(new_name) = table_renames.get(table.name.as_str()) {
                        result.push(Operation::RenameTable {
                            old_name: table.name.clone(),
                            new_name: new_name.to_string(),
                        });
                    } else {
                        result.push(op.clone());
                    }
                }
                Operation::CreateTable { table } => {
                    if rename_table_targets.contains(table.name.as_str()) {
                        continue;
                    }
                    result.push(op.clone());
                }
                Operation::AlterColumn {
                    table_name,
                    old,
                    new,
                    cast_expr,
                } => {
                    let mut new_col = new.clone();
                    let mut cast = cast_expr.clone();

                    if old.nullable && !new.nullable {
                        let id = super::ids::clarification_id(&ClarificationKind::NotNullChange {
                            table: table_name.clone(),
                            column: old.name.clone(),
                        });
                        if let Some(answer) = decision_map.get(id.as_str()) {
                            match answer {
                                Answer::NotNullDefault(val) => {
                                    result.push(Operation::Statement {
                                        up: format!(
                                            "UPDATE \"{}\" SET \"{}\" = {} WHERE \"{}\" IS NULL",
                                            table_name, old.name, val, old.name
                                        ),
                                        down: None,
                                    });
                                }
                                Answer::NotNullNullable => {
                                    new_col.nullable = true;
                                }
                                _ => {}
                            }
                        }
                    }

                    if normalize_type(&old.col_type) != normalize_type(&new_col.col_type) {
                        let id = super::ids::clarification_id(&ClarificationKind::TypeCast {
                            table: table_name.clone(),
                            column: old.name.clone(),
                            from: old.col_type.clone(),
                            to: new_col.col_type.clone(),
                        });
                        if let Some(answer) = decision_map.get(id.as_str()) {
                            match answer {
                                Answer::TypeCast(expr) => cast = Some(expr.clone()),
                                Answer::TypeCastImplicit => cast = None,
                                _ => {}
                            }
                        }
                    }

                    result.push(Operation::AlterColumn {
                        table_name: table_name.clone(),
                        old: old.clone(),
                        new: new_col,
                        cast_expr: cast,
                    });
                }
                Operation::DropEnum { enum_def } => {
                    let key = enum_def.qualified_name();
                    let mut renamed = enum_def.clone();
                    let mut emitted_rename = false;

                    for value in &enum_def.values {
                        if let Some(new_value) =
                            enum_value_renames.get(&(key.as_str(), value.as_str()))
                        {
                            result.push(Operation::RenameEnumValue {
                                enum_name: enum_def.name.clone(),
                                schema: enum_def.schema.clone(),
                                old_value: value.clone(),
                                new_value: (*new_value).to_string(),
                            });
                            for renamed_value in &mut renamed.values {
                                if renamed_value == value {
                                    *renamed_value = (*new_value).to_string();
                                    break;
                                }
                            }
                            emitted_rename = true;
                        }
                    }

                    if emitted_rename {
                        if let Some(new_enum) = enum_create_by_key.get(&key) {
                            if renamed != **new_enum {
                                result.push(Operation::AlterEnum {
                                    old: renamed,
                                    new: (*new_enum).clone(),
                                });
                            }
                            replaced_enums.insert(key);
                        } else {
                            result.push(Operation::DropEnum { enum_def: renamed });
                        }
                    } else {
                        result.push(op.clone());
                    }
                }
                Operation::CreateEnum { enum_def }
                    if replaced_enums.contains(&enum_def.qualified_name()) => {}
                _ => result.push(op.clone()),
            }
        }

        result
    }

    fn clarification(&self, id: &str) -> Option<&Clarification> {
        self.by_id.get(id).map(|idx| &self.clarifications[*idx])
    }
}

fn filter_claimed_candidates(
    clarification: &Clarification,
    claimed_columns: &HashSet<(String, String)>,
    claimed_tables: &HashSet<String>,
    claimed_enum_values: &HashSet<(String, String)>,
) -> Option<Clarification> {
    let mut clarification = clarification.clone();
    match &mut clarification.kind {
        ClarificationKind::RenameColumn {
            table, candidates, ..
        } => {
            candidates.retain(|n| !claimed_columns.contains(&(table.clone(), n.clone())));
            (!candidates.is_empty()).then_some(clarification)
        }
        ClarificationKind::RenameTable { candidates, .. } => {
            candidates.retain(|n| !claimed_tables.contains(n));
            (!candidates.is_empty()).then_some(clarification)
        }
        ClarificationKind::RenameEnumValue {
            enum_name,
            candidates,
            ..
        } => {
            candidates.retain(|n| !claimed_enum_values.contains(&(enum_name.clone(), n.clone())));
            (!candidates.is_empty()).then_some(clarification)
        }
        _ => Some(clarification),
    }
}

fn validate_answer(clar: &Clarification, answer: &Answer) -> Result<(), DisambiguatorError> {
    match (&clar.kind, answer) {
        (ClarificationKind::RenameColumn { candidates, .. }, Answer::RenameTo(name))
        | (ClarificationKind::RenameTable { candidates, .. }, Answer::RenameTo(name))
        | (ClarificationKind::RenameEnumValue { candidates, .. }, Answer::RenameTo(name)) => {
            if !candidates.contains(name) {
                return Err(DisambiguatorError::InvalidCandidate {
                    id: clar.id.clone(),
                    chosen: name.clone(),
                });
            }
        }
        (ClarificationKind::RenameColumn { .. }, Answer::RenameNo)
        | (ClarificationKind::RenameTable { .. }, Answer::RenameNo)
        | (ClarificationKind::RenameEnumValue { .. }, Answer::RenameNo) => {}
        (ClarificationKind::UnknownType { suggested, .. }, Answer::UseType(name)) => {
            if name.trim().is_empty() {
                return Err(DisambiguatorError::EmptyInput {
                    id: clar.id.clone(),
                });
            }
            if !suggested.contains(name) {
                return Err(DisambiguatorError::InvalidCandidate {
                    id: clar.id.clone(),
                    chosen: name.clone(),
                });
            }
        }
        (ClarificationKind::UnknownType { .. }, Answer::KeepType) => {}
        (ClarificationKind::NotNullAdd { .. }, Answer::NotNullDefault(value))
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullDefault(value))
        | (ClarificationKind::TypeCast { .. }, Answer::TypeCast(value)) => {
            if value.trim().is_empty() {
                return Err(DisambiguatorError::EmptyInput {
                    id: clar.id.clone(),
                });
            }
        }
        (ClarificationKind::NotNullAdd { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullAdd { .. }, Answer::NotNullManual)
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullManual)
        | (ClarificationKind::TypeCast { .. }, Answer::TypeCastImplicit) => {}
        _ => {
            return Err(DisambiguatorError::InvalidAnswer {
                id: clar.id.clone(),
            });
        }
    }
    Ok(())
}

fn rename_claim(clar: &Clarification, target: &str) -> Option<(String, String)> {
    match &clar.kind {
        ClarificationKind::RenameColumn { table, .. } => {
            Some((format!("table column '{table}'"), target.to_string()))
        }
        ClarificationKind::RenameTable { .. } => {
            Some(("table rename".to_string(), target.to_string()))
        }
        ClarificationKind::RenameEnumValue { enum_name, .. } => {
            Some((format!("enum value '{enum_name}'"), target.to_string()))
        }
        _ => None,
    }
}
