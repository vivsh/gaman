use std::collections::{HashMap, HashSet};

use crate::operations::Operation;
use crate::states::EnumDef;

use super::analyze::all_clarifications_raw;
use super::model::{
    Answer, Clarification, ClarificationKind, ClarifyError, ClarifyResult, Decision,
};
use super::types::normalize_type;

pub(crate) struct ClarifyPlan {
    clarifications: Vec<Clarification>,
    by_id: HashMap<String, usize>,
}

impl ClarifyPlan {
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
    ) -> Result<ClarifyResult, ClarifyError> {
        self.validate_decisions(decisions)?;
        let pending = self.pending_clarifications(decisions);
        if pending.is_empty() {
            Ok(ClarifyResult::Resolved(
                self.apply_decisions(ops, decisions),
            ))
        } else {
            Ok(ClarifyResult::NeedsInput(pending))
        }
    }

    fn validate_decisions(&self, decisions: &[Decision]) -> Result<(), ClarifyError> {
        let mut seen = HashSet::new();
        for decision in decisions {
            if !seen.insert(decision.clarification_id.as_str()) {
                return Err(ClarifyError::DuplicateDecision(
                    decision.clarification_id.clone(),
                ));
            }

            let clar = self
                .clarification(&decision.clarification_id)
                .ok_or_else(|| ClarifyError::UnknownDecision(decision.clarification_id.clone()))?;
            validate_answer(clar, &decision.answer)?;
        }
        self.validate_rename_claims(decisions)
    }

    fn validate_rename_claims(&self, decisions: &[Decision]) -> Result<(), ClarifyError> {
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
                return Err(ClarifyError::ConflictingRenameTarget { scope, target });
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
        let dropped_tables = ops
            .iter()
            .filter_map(|operation| match operation {
                Operation::DropTable { table } => Some(table.qualified_name()),
                _ => None,
            })
            .collect::<HashSet<_>>();
        let deleted_rows = ops
            .iter()
            .filter_map(|operation| match operation {
                Operation::DeleteRow {
                    table_name,
                    key,
                    row,
                } => row
                    .identity(key)
                    .ok()
                    .map(|identity| ((table_name.as_str(), identity), row)),
                _ => None,
            })
            .collect::<HashMap<_, _>>();

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
                Operation::InsertRow {
                    table_name,
                    key,
                    row,
                } if rename_table_targets.contains(table_name.as_str()) => {
                    let previous_table = table_renames
                        .iter()
                        .find_map(|(old, new)| (*new == table_name.as_str()).then_some(*old));
                    let previous = previous_table
                        .and_then(|table| row.identity(key).ok().map(|identity| (table, identity)))
                        .and_then(|identity| deleted_rows.get(&identity));
                    if let Some(previous) = previous
                        && *previous != row
                    {
                        result.push(Operation::UpdateRow {
                            table_name: table_name.clone(),
                            key: key.clone(),
                            old: (*previous).clone(),
                            new: row.clone(),
                        });
                    }
                }
                Operation::UpdateRow {
                    table_name,
                    key,
                    old,
                    new,
                } => {
                    let mut expected = old.clone();
                    for ((renamed_table, old_name), new_name) in &col_renames {
                        if *renamed_table != table_name.as_str() {
                            continue;
                        }
                        if let Some(value) = expected.values.remove(*old_name) {
                            expected.values.insert((*new_name).to_string(), value);
                        }
                    }
                    if expected != *new {
                        result.push(Operation::UpdateRow {
                            table_name: table_name.clone(),
                            key: key
                                .iter()
                                .map(|name| {
                                    col_renames
                                        .get(&(table_name.as_str(), name.as_str()))
                                        .map_or_else(|| name.clone(), |name| (*name).to_string())
                                })
                                .collect(),
                            old: expected,
                            new: new.clone(),
                        });
                    }
                }
                Operation::DeleteRow { table_name, .. } if dropped_tables.contains(table_name) => {}
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

        trust_accepted_risks(&mut result, &decision_map);
        result
    }

    fn clarification(&self, id: &str) -> Option<&Clarification> {
        self.by_id.get(id).map(|idx| &self.clarifications[*idx])
    }
}

fn trust_accepted_risks(ops: &mut [Operation], decisions: &HashMap<&str, &Answer>) {
    for op in ops {
        if let Operation::CreateTable { table } = op {
            let table_name = table.qualified_name();
            for index in &mut table.indexes {
                let id = opaque_id(
                    crate::states::types::EntityKind::Index,
                    &format!("{}.{}", table_name, index.name),
                );
                if index.is_opaque() && accepted(decisions, &id) {
                    index.mark_trusted();
                }
            }
            for constraint in &mut table.constraints {
                let id = opaque_id(
                    crate::states::types::EntityKind::Constraint,
                    &format!("{}.{}", table_name, constraint.name()),
                );
                if constraint.is_opaque() && accepted(decisions, &id) {
                    constraint.mark_trusted();
                }
            }
            for trigger in &mut table.triggers {
                let name = trigger.name.as_deref().unwrap_or("<unnamed>");
                let id = opaque_id(
                    crate::states::types::EntityKind::Trigger,
                    &format!("{}.{}", table_name, name),
                );
                if trigger.is_opaque() && accepted(decisions, &id) {
                    trigger.mark_trusted();
                }
            }
        }
        match op {
            Operation::CreateTable { table } if table.has_unmanaged_options() => {
                let id = super::ids::clarification_id(&ClarificationKind::UnmanagedTableOptions {
                    table: table.qualified_name(),
                });
                if accepted(decisions, &id) {
                    table.mark_options_trusted();
                }
            }
            Operation::AcknowledgeTableOptions {
                table_name, new, ..
            } => {
                let id = super::ids::clarification_id(&ClarificationKind::UnmanagedTableOptions {
                    table: table_name.clone(),
                });
                if accepted(decisions, &id) {
                    new.trusted = true;
                }
            }
            Operation::AddIndex {
                table_name, index, ..
            }
            | Operation::DropIndex {
                table_name, index, ..
            } if index.is_opaque() => {
                let id = opaque_id(
                    crate::states::types::EntityKind::Index,
                    &format!("{}.{}", table_name, index.name),
                );
                if accepted(decisions, &id) {
                    index.mark_trusted();
                }
            }
            Operation::AddConstraint {
                table_name,
                constraint,
            }
            | Operation::DropConstraint {
                table_name,
                constraint,
            } if constraint.is_opaque() => {
                let id = opaque_id(
                    crate::states::types::EntityKind::Constraint,
                    &format!("{}.{}", table_name, constraint.name()),
                );
                if accepted(decisions, &id) {
                    constraint.mark_trusted();
                }
            }
            Operation::CreateTrigger {
                table_name,
                trigger,
            }
            | Operation::DropTrigger {
                table_name,
                trigger,
            }
            | Operation::AlterTrigger {
                table_name,
                new: trigger,
                ..
            } if trigger.is_opaque() => {
                let name = trigger.name.as_deref().unwrap_or("<unnamed>");
                let id = opaque_id(
                    crate::states::types::EntityKind::Trigger,
                    &format!("{}.{}", table_name, name),
                );
                if accepted(decisions, &id) {
                    trigger.mark_trusted();
                }
            }
            Operation::CreateFunction { function } | Operation::DropFunction { function }
                if function.is_opaque() =>
            {
                let id = opaque_id(
                    crate::states::types::EntityKind::Function,
                    &function.qualified_name(),
                );
                if accepted(decisions, &id) {
                    function.mark_trusted();
                }
            }
            Operation::AlterFunction { new, .. } if new.is_opaque() => {
                let id = opaque_id(
                    crate::states::types::EntityKind::Function,
                    &new.qualified_name(),
                );
                if accepted(decisions, &id) {
                    new.mark_trusted();
                }
            }
            Operation::CreateView { view } | Operation::DropView { view } if view.is_opaque() => {
                let id = opaque_id(
                    crate::states::types::EntityKind::View,
                    &view.qualified_name(),
                );
                if accepted(decisions, &id) {
                    view.mark_trusted();
                }
            }
            Operation::ReplaceView { new, .. } if new.is_opaque() => {
                let id = opaque_id(
                    crate::states::types::EntityKind::View,
                    &new.qualified_name(),
                );
                if accepted(decisions, &id) {
                    new.mark_trusted();
                }
            }
            Operation::CreateExtension { extension } | Operation::DropExtension { extension }
                if extension.is_opaque() =>
            {
                let id = opaque_id(
                    crate::states::types::EntityKind::Extension,
                    &extension.qualified_name(),
                );
                if accepted(decisions, &id) {
                    extension.mark_trusted();
                }
            }
            _ => {}
        }
    }
}

fn accepted(decisions: &HashMap<&str, &Answer>, id: &str) -> bool {
    decisions
        .get(id)
        .is_some_and(|answer| matches!(**answer, Answer::AcceptRisk))
}

fn opaque_id(kind: crate::states::types::EntityKind, name: &str) -> String {
    super::ids::clarification_id(&ClarificationKind::OpaqueEntity {
        kind,
        name: name.to_string(),
    })
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

fn validate_answer(clar: &Clarification, answer: &Answer) -> Result<(), ClarifyError> {
    match (&clar.kind, answer) {
        (ClarificationKind::RenameColumn { candidates, .. }, Answer::RenameTo(name))
        | (ClarificationKind::RenameTable { candidates, .. }, Answer::RenameTo(name))
        | (ClarificationKind::RenameEnumValue { candidates, .. }, Answer::RenameTo(name)) => {
            if !candidates.contains(name) {
                return Err(ClarifyError::InvalidCandidate {
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
                return Err(ClarifyError::EmptyInput {
                    id: clar.id.clone(),
                });
            }
            if !suggested.contains(name) {
                return Err(ClarifyError::InvalidCandidate {
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
                return Err(ClarifyError::EmptyInput {
                    id: clar.id.clone(),
                });
            }
        }
        (ClarificationKind::NotNullAdd { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullAdd { .. }, Answer::NotNullManual)
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullManual)
        | (ClarificationKind::TypeCast { .. }, Answer::TypeCastImplicit) => {}
        (ClarificationKind::OpaqueEntity { .. }, Answer::AcceptRisk)
        | (ClarificationKind::UnmanagedTableOptions { .. }, Answer::AcceptRisk)
        | (ClarificationKind::ColumnMetadataChange { .. }, Answer::AcceptRisk)
        | (ClarificationKind::DeleteManagedRow { .. }, Answer::AcceptRisk) => {}
        _ => {
            return Err(ClarifyError::InvalidAnswer {
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
