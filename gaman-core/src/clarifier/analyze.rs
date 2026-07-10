use std::collections::{HashMap, HashSet};

use crate::operations::Operation;
use crate::states::types::EntityKind;
use crate::states::{Column, EnumDef, Table};

use super::ids::clarification_id;
use super::model::{Clarification, ClarificationKind, Severity};
use super::types::{normalize_type, types_compatible};

#[derive(Debug, Clone, PartialEq, Eq)]
struct RankedCandidate {
    name: String,
    score: i32,
}

pub(crate) fn all_clarifications_raw(ops: &[Operation]) -> Vec<Clarification> {
    let mut result = Vec::new();
    let mut dropped_cols: HashMap<&str, Vec<(usize, &Column)>> = HashMap::new();
    let mut added_cols: HashMap<&str, Vec<(usize, &Column)>> = HashMap::new();
    let mut dropped_tables: Vec<(usize, &Table)> = Vec::new();
    let mut created_tables: Vec<(usize, &Table)> = Vec::new();
    let mut dropped_enums: HashMap<String, &EnumDef> = HashMap::new();
    let mut created_enums: HashMap<String, &EnumDef> = HashMap::new();

    for (idx, op) in ops.iter().enumerate() {
        match op {
            Operation::DropColumn {
                table_name, column, ..
            } => {
                dropped_cols
                    .entry(table_name.as_str())
                    .or_default()
                    .push((idx, column));
            }
            Operation::AddColumn { table_name, column } => {
                added_cols
                    .entry(table_name.as_str())
                    .or_default()
                    .push((idx, column));
            }
            Operation::DropTable { table } => dropped_tables.push((idx, table)),
            Operation::CreateTable { table } => created_tables.push((idx, table)),
            Operation::DropEnum { enum_def } => {
                dropped_enums.insert(enum_def.qualified_name(), enum_def);
            }
            Operation::CreateEnum { enum_def } => {
                created_enums.insert(enum_def.qualified_name(), enum_def);
            }
            _ => {}
        }
    }

    result.extend(column_rename_clarifications(&dropped_cols, &added_cols));
    result.extend(table_rename_clarifications(
        &dropped_tables,
        &created_tables,
    ));
    result.extend(enum_value_rename_clarifications(
        &dropped_enums,
        &created_enums,
    ));
    result.extend(risky_change_clarifications(ops));
    result.extend(opaque_and_unmanaged_clarifications(ops));

    result.sort_by(|a, b| a.id.cmp(&b.id));
    result.dedup_by(|left, right| left.id == right.id);
    result
}

fn opaque_and_unmanaged_clarifications(ops: &[Operation]) -> Vec<Clarification> {
    let mut result = Vec::new();
    for op in ops {
        if let Operation::CreateTable { table } = op {
            for index in &table.indexes {
                if index.is_opaque() && !index.opaque.trusted {
                    push_opaque(
                        &mut result,
                        EntityKind::Index,
                        format!("{}.{}", table.qualified_name(), index.name),
                    );
                }
            }
            for constraint in &table.constraints {
                if constraint
                    .opaque_meta()
                    .is_some_and(|opaque| opaque.raw.is_some() && !opaque.trusted)
                {
                    push_opaque(
                        &mut result,
                        EntityKind::Constraint,
                        format!("{}.{}", table.qualified_name(), constraint.name()),
                    );
                }
            }
            for trigger in &table.triggers {
                if trigger.is_opaque() && !trigger.opaque.trusted {
                    let name = trigger.name.as_deref().unwrap_or("<unnamed>");
                    push_opaque(
                        &mut result,
                        EntityKind::Trigger,
                        format!("{}.{}", table.qualified_name(), name),
                    );
                }
            }
        }
        match op {
            Operation::CreateTable { table }
                if table.has_unmanaged_options() && !table.options.trusted =>
            {
                let kind = ClarificationKind::UnmanagedTableOptions {
                    table: table.qualified_name(),
                };
                result.push(Clarification {
                    id: clarification_id(&kind),
                    severity: Severity::Warning,
                    kind,
                });
            }
            Operation::AcknowledgeTableOptions {
                table_name, new, ..
            } if !new.trusted => {
                let kind = ClarificationKind::UnmanagedTableOptions {
                    table: table_name.clone(),
                };
                result.push(Clarification {
                    id: clarification_id(&kind),
                    severity: Severity::Warning,
                    kind,
                });
            }
            Operation::AddIndex {
                table_name, index, ..
            } if index.is_opaque() && !index.opaque.trusted => {
                push_opaque(
                    &mut result,
                    EntityKind::Index,
                    format!("{}.{}", table_name, index.name),
                );
            }
            Operation::DropIndex {
                table_name, index, ..
            } if index.is_opaque() => {
                push_opaque(
                    &mut result,
                    EntityKind::Index,
                    format!("{}.{}", table_name, index.name),
                );
            }
            Operation::AddConstraint {
                table_name,
                constraint,
            } if constraint
                .opaque_meta()
                .is_some_and(|opaque| opaque.raw.is_some() && !opaque.trusted) =>
            {
                push_opaque(
                    &mut result,
                    EntityKind::Constraint,
                    format!("{}.{}", table_name, constraint.name()),
                );
            }
            Operation::DropConstraint {
                table_name,
                constraint,
            } if constraint.is_opaque() => {
                push_opaque(
                    &mut result,
                    EntityKind::Constraint,
                    format!("{}.{}", table_name, constraint.name()),
                );
            }
            Operation::CreateTrigger {
                table_name,
                trigger,
            } if trigger.is_opaque() && !trigger.opaque.trusted => {
                let name = trigger.name.as_deref().unwrap_or("<unnamed>");
                push_opaque(
                    &mut result,
                    EntityKind::Trigger,
                    format!("{}.{}", table_name, name),
                );
            }
            Operation::DropTrigger {
                table_name,
                trigger,
            } if trigger.is_opaque() => {
                let name = trigger.name.as_deref().unwrap_or("<unnamed>");
                push_opaque(
                    &mut result,
                    EntityKind::Trigger,
                    format!("{}.{}", table_name, name),
                );
            }
            Operation::AlterTrigger {
                table_name,
                old,
                new,
            } if old.is_opaque() || new.is_opaque() => {
                let name = new
                    .name
                    .as_deref()
                    .or(old.name.as_deref())
                    .unwrap_or("<unnamed>");
                push_opaque(
                    &mut result,
                    EntityKind::Trigger,
                    format!("{}.{}", table_name, name),
                );
            }
            Operation::CreateFunction { function }
                if function.is_opaque() && !function.opaque.trusted =>
            {
                push_opaque(&mut result, EntityKind::Function, function.qualified_name());
            }
            Operation::DropFunction { function } if function.is_opaque() => {
                push_opaque(&mut result, EntityKind::Function, function.qualified_name());
            }
            Operation::AlterFunction { old, new } if old.is_opaque() || new.is_opaque() => {
                push_opaque(&mut result, EntityKind::Function, new.qualified_name());
            }
            Operation::CreateView { view } if view.is_opaque() && !view.opaque.trusted => {
                push_opaque(&mut result, EntityKind::View, view.qualified_name());
            }
            Operation::DropView { view } if view.is_opaque() => {
                push_opaque(&mut result, EntityKind::View, view.qualified_name());
            }
            Operation::ReplaceView { old, new } if old.is_opaque() || new.is_opaque() => {
                push_opaque(&mut result, EntityKind::View, new.qualified_name());
            }
            Operation::CreateExtension { extension }
                if extension.is_opaque() && !extension.opaque.trusted =>
            {
                push_opaque(
                    &mut result,
                    EntityKind::Extension,
                    extension.qualified_name(),
                );
            }
            Operation::DropExtension { extension } if extension.is_opaque() => {
                push_opaque(
                    &mut result,
                    EntityKind::Extension,
                    extension.qualified_name(),
                );
            }
            _ => {}
        }
    }
    result
}

fn push_opaque(result: &mut Vec<Clarification>, kind: EntityKind, name: String) {
    let kind = ClarificationKind::OpaqueEntity { kind, name };
    result.push(Clarification {
        id: clarification_id(&kind),
        severity: Severity::Warning,
        kind,
    });
}

fn column_rename_clarifications(
    dropped_cols: &HashMap<&str, Vec<(usize, &Column)>>,
    added_cols: &HashMap<&str, Vec<(usize, &Column)>>,
) -> Vec<Clarification> {
    let mut result = Vec::new();
    let mut table_names: Vec<&str> = dropped_cols.keys().copied().collect();
    table_names.sort();

    for table_name in table_names {
        let drops = &dropped_cols[table_name];
        let adds = added_cols
            .get(table_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[]);
        let mut sorted_drops = drops.to_vec();
        sorted_drops.sort_by_key(|(_, c)| c.name.as_str());

        for (drop_idx, dropped) in sorted_drops {
            let candidates = ranked_column_candidates(dropped, drop_idx, adds);
            if candidates.is_empty() {
                continue;
            }
            let kind = ClarificationKind::RenameColumn {
                table: table_name.to_string(),
                old: dropped.name.clone(),
                candidates: candidates.into_iter().map(|c| c.name).collect(),
            };
            result.push(Clarification {
                id: clarification_id(&kind),
                severity: Severity::Suggestion,
                kind,
            });
        }
    }

    result
}

fn table_rename_clarifications(
    dropped_tables: &[(usize, &Table)],
    created_tables: &[(usize, &Table)],
) -> Vec<Clarification> {
    let mut result = Vec::new();
    let mut sorted_dropped_tables = dropped_tables.to_vec();
    sorted_dropped_tables.sort_by_key(|(_, t)| t.name.as_str());

    for (drop_idx, dropped) in sorted_dropped_tables {
        let candidates = ranked_table_candidates(dropped, drop_idx, created_tables);
        if candidates.is_empty() {
            continue;
        }
        let kind = ClarificationKind::RenameTable {
            old: dropped.name.clone(),
            candidates: candidates.into_iter().map(|c| c.name).collect(),
        };
        result.push(Clarification {
            id: clarification_id(&kind),
            severity: Severity::Suggestion,
            kind,
        });
    }

    result
}

fn enum_value_rename_clarifications(
    dropped_enums: &HashMap<String, &EnumDef>,
    created_enums: &HashMap<String, &EnumDef>,
) -> Vec<Clarification> {
    let mut result = Vec::new();
    let mut enum_keys: Vec<String> = dropped_enums
        .keys()
        .filter(|key| created_enums.contains_key(*key))
        .cloned()
        .collect();
    enum_keys.sort();

    for key in enum_keys {
        let old_enum = dropped_enums[&key];
        let new_enum = created_enums[&key];
        let old_values: HashSet<&str> = old_enum.values.iter().map(|v| v.as_str()).collect();
        let mut removed: Vec<&String> = old_enum
            .values
            .iter()
            .filter(|value| !new_enum.values.contains(value))
            .collect();
        removed.sort();
        let added: Vec<&String> = new_enum
            .values
            .iter()
            .filter(|value| !old_values.contains(value.as_str()))
            .collect();

        for old_value in removed {
            let mut candidates: Vec<String> = added
                .iter()
                .filter(|new_value| {
                    enum_rename_leaves_safe_additions(old_enum, new_enum, old_value, new_value)
                })
                .map(|value| (*value).clone())
                .collect();
            candidates.sort();
            if candidates.is_empty() {
                continue;
            }
            let kind = ClarificationKind::RenameEnumValue {
                enum_name: key.clone(),
                old: old_value.clone(),
                candidates,
            };
            result.push(Clarification {
                id: clarification_id(&kind),
                severity: Severity::Warning,
                kind,
            });
        }
    }

    result
}

fn risky_change_clarifications(ops: &[Operation]) -> Vec<Clarification> {
    let mut result = Vec::new();
    for op in ops {
        match op {
            Operation::AddColumn { table_name, column }
                if !column.nullable && column.default.is_none() && !column.primary_key =>
            {
                let kind = ClarificationKind::NotNullAdd {
                    table: table_name.clone(),
                    column: column.name.clone(),
                    col_type: column.col_type.clone(),
                };
                result.push(Clarification {
                    id: clarification_id(&kind),
                    severity: Severity::Fatal,
                    kind,
                });
            }
            Operation::AlterColumn {
                table_name,
                old,
                new,
                ..
            } => {
                if old.nullable && !new.nullable {
                    let kind = ClarificationKind::NotNullChange {
                        table: table_name.clone(),
                        column: old.name.clone(),
                    };
                    result.push(Clarification {
                        id: clarification_id(&kind),
                        severity: Severity::Fatal,
                        kind,
                    });
                }
                if normalize_type(&old.col_type) != normalize_type(&new.col_type) {
                    let kind = ClarificationKind::TypeCast {
                        table: table_name.clone(),
                        column: old.name.clone(),
                        from: old.col_type.clone(),
                        to: new.col_type.clone(),
                    };
                    result.push(Clarification {
                        id: clarification_id(&kind),
                        severity: Severity::Warning,
                        kind,
                    });
                }
            }
            _ => {}
        }
    }
    result
}

fn ranked_column_candidates(
    dropped: &Column,
    drop_idx: usize,
    adds: &[(usize, &Column)],
) -> Vec<RankedCandidate> {
    let mut candidates: Vec<RankedCandidate> = adds
        .iter()
        .filter_map(|(add_idx, added)| column_rename_score(dropped, drop_idx, added, *add_idx))
        .collect();
    candidates.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    candidates
}

fn ranked_table_candidates(
    dropped: &Table,
    drop_idx: usize,
    created: &[(usize, &Table)],
) -> Vec<RankedCandidate> {
    let mut candidates: Vec<RankedCandidate> = created
        .iter()
        .filter_map(|(create_idx, table)| table_rename_score(dropped, drop_idx, table, *create_idx))
        .collect();
    candidates.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.name.cmp(&b.name)));
    candidates
}

fn column_rename_score(
    dropped: &Column,
    drop_idx: usize,
    added: &Column,
    add_idx: usize,
) -> Option<RankedCandidate> {
    if added.name == dropped.name || !types_compatible(&dropped.col_type, &added.col_type) {
        return None;
    }

    let mut score = 100;
    if dropped.col_type == added.col_type {
        score += 25;
    } else if normalize_type(&dropped.col_type) == normalize_type(&added.col_type) {
        score += 20;
    }
    score += name_similarity_score(&dropped.name, &added.name);
    if dropped.nullable == added.nullable {
        score += 5;
    }
    if dropped.default == added.default {
        score += 4;
    }
    if dropped.generated == added.generated {
        score += 4;
    }
    if add_idx > drop_idx {
        score += 2;
    }

    Some(RankedCandidate {
        name: added.name.clone(),
        score,
    })
}

fn table_rename_score(
    dropped: &Table,
    drop_idx: usize,
    created: &Table,
    create_idx: usize,
) -> Option<RankedCandidate> {
    if created.name == dropped.name {
        return None;
    }

    if dropped.columns.is_empty() && created.columns.is_empty() {
        return Some(RankedCandidate {
            name: created.name.clone(),
            score: 10 + name_similarity_score(&dropped.name, &created.name),
        });
    }

    let min_len = dropped.columns.len().min(created.columns.len());
    if min_len == 0 {
        return None;
    }

    let mut score = name_similarity_score(&dropped.name, &created.name);
    let mut matched = 0;
    for old in &dropped.columns {
        if let Some((_, best_score)) = created
            .columns
            .iter()
            .filter(|candidate| types_compatible(&old.col_type, &candidate.col_type))
            .map(|candidate| (candidate, column_pair_score(old, candidate)))
            .max_by_key(|(_, score)| *score)
            && best_score > 0
        {
            matched += 1;
            score += best_score;
        }
    }

    if matched * 2 <= min_len {
        return None;
    }

    if dropped.primary_key == created.primary_key {
        score += 8;
    }
    if dropped.indexes.len() == created.indexes.len() {
        score += 3;
    }
    if dropped.constraints.len() == created.constraints.len() {
        score += 3;
    }
    if dropped.foreign_keys.len() == created.foreign_keys.len() {
        score += 3;
    }
    if dropped.triggers.len() == created.triggers.len() {
        score += 2;
    }
    if create_idx > drop_idx {
        score += 2;
    }

    Some(RankedCandidate {
        name: created.name.clone(),
        score,
    })
}

fn column_pair_score(old: &Column, new: &Column) -> i32 {
    let mut score = 0;
    if old.name == new.name {
        score += 40;
    } else {
        score += name_similarity_score(&old.name, &new.name);
    }
    if old.col_type == new.col_type {
        score += 25;
    } else if normalize_type(&old.col_type) == normalize_type(&new.col_type) {
        score += 20;
    } else if types_compatible(&old.col_type, &new.col_type) {
        score += 12;
    }
    if old.nullable == new.nullable {
        score += 4;
    }
    if old.default == new.default {
        score += 3;
    }
    if old.primary_key == new.primary_key {
        score += 3;
    }
    score
}

fn name_similarity_score(left: &str, right: &str) -> i32 {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    if left == right {
        return 40;
    }

    let mut score = 0;
    if left.contains(&right) || right.contains(&left) {
        score += 18;
    }
    score += common_prefix_len(&left, &right).min(12) as i32;
    score += common_suffix_len(&left, &right).min(8) as i32;

    let left_tokens = split_name_tokens(&left);
    let right_tokens = split_name_tokens(&right);
    let common = left_tokens
        .iter()
        .filter(|token| right_tokens.contains(token))
        .count();
    score + (common as i32 * 8)
}

fn common_prefix_len(left: &str, right: &str) -> usize {
    left.chars()
        .zip(right.chars())
        .take_while(|(left, right)| left == right)
        .count()
}

fn common_suffix_len(left: &str, right: &str) -> usize {
    left.chars()
        .rev()
        .zip(right.chars().rev())
        .take_while(|(left, right)| left == right)
        .count()
}

fn split_name_tokens(value: &str) -> Vec<&str> {
    value
        .split(|ch: char| !ch.is_ascii_alphanumeric())
        .filter(|token| !token.is_empty())
        .collect()
}

fn enum_after_value_rename(old: &EnumDef, old_value: &str, new_value: &str) -> EnumDef {
    let mut renamed = old.clone();
    for value in &mut renamed.values {
        if value == old_value {
            *value = new_value.to_string();
            break;
        }
    }
    renamed
}

fn enum_rename_leaves_safe_additions(
    old: &EnumDef,
    new: &EnumDef,
    old_value: &str,
    new_value: &str,
) -> bool {
    let renamed = enum_after_value_rename(old, old_value, new_value);
    values_are_subsequence(&renamed.values, &new.values)
}

fn values_are_subsequence(old: &[String], new: &[String]) -> bool {
    let mut new_iter = new.iter();
    old.iter()
        .all(|old_value| new_iter.by_ref().any(|new_value| new_value == old_value))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(name: &str, t: &str, nullable: bool) -> Column {
        Column {
            name: name.to_string(),
            col_type: t.to_string(),
            nullable,
            default: None,
            primary_key: false,
            references: None,
            check: None,
            generated: None,
        }
    }

    fn table(name: &str, columns: Vec<Column>) -> Table {
        Table {
            name: name.to_string(),
            schema: None,
            primary_key: None,
            columns,
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
            options: Default::default(),
        }
    }

    #[test]
    fn column_candidates_rank_stronger_name_match_first() {
        let dropped = col("email", "varchar", true);
        let weaker = col("display_name", "text", true);
        let stronger = col("email_address", "text", true);
        let candidates = ranked_column_candidates(&dropped, 0, &[(2, &weaker), (1, &stronger)]);

        assert_eq!(candidates[0].name, "email_address");
    }

    #[test]
    fn table_candidates_rank_structural_match_first() {
        let dropped = table(
            "users",
            vec![col("id", "integer", false), col("email", "text", true)],
        );
        let weaker = table("accounts", vec![col("id", "integer", false)]);
        let stronger = table(
            "members",
            vec![col("id", "integer", false), col("email", "text", true)],
        );
        let candidates = ranked_table_candidates(&dropped, 0, &[(2, &weaker), (1, &stronger)]);

        assert_eq!(candidates[0].name, "members");
    }
}
