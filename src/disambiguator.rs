use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::operations::Operation;
use crate::states::{Column, Table};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Fatal,
    Warning,
    Suggestion,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClarificationKind {
    RenameTable { old: String, candidates: Vec<String> },
    RenameColumn { table: String, old: String, candidates: Vec<String> },
    NotNullAdd { table: String, column: String, col_type: String },
    NotNullChange { table: String, column: String },
    TypeCast { table: String, column: String, from: String, to: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clarification {
    pub id: String,
    pub severity: Severity,
    pub kind: ClarificationKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum Answer {
    RenameTo(String),
    RenameNo,
    NotNullDefault(String),
    NotNullNullable,
    NotNullManual,
    TypeCast(String),
    TypeCastImplicit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub clarification_id: String,
    pub answer: Answer,
}

#[derive(Debug, Clone)]
pub enum DisambiguationResult {
    NeedsInput(Vec<Clarification>),
    Resolved(Vec<Operation>),
}

#[derive(Debug, Error, PartialEq)]
pub enum DisambiguatorError {
    #[error("decision references unknown clarification '{0}'")]
    UnknownDecision(String),
    #[error("decision for '{id}' has an answer incompatible with the clarification kind")]
    InvalidAnswer { id: String },
    #[error("decision for '{id}' chose '{chosen}' which is not a valid candidate")]
    InvalidCandidate { id: String, chosen: String },
}

#[derive(Debug, Error)]
pub enum PromptError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    #[allow(dead_code)]
    Other(String),
}

pub trait PromptEngine {
    fn prompt(&self, clarifications: &[Clarification]) -> Result<Vec<Decision>, PromptError>;
}

pub struct Disambiguator;

impl Disambiguator {
    pub fn process(
        &self,
        ops: &[Operation],
        decisions: &[Decision],
    ) -> Result<DisambiguationResult, DisambiguatorError> {
        let pending = gather_clarifications(ops, decisions);
        if pending.is_empty() {
            let resolved = apply_decisions(ops, decisions)?;
            Ok(DisambiguationResult::Resolved(resolved))
        } else {
            Ok(DisambiguationResult::NeedsInput(pending))
        }
    }
}

fn normalize_type(t: &str) -> String {
    let t = t.trim().to_lowercase();
    match t.find('(') {
        Some(idx) => t[..idx].trim_end().to_string(),
        None => t,
    }
}

fn type_family(t: &str) -> Option<u8> {
    match t {
        "text" | "varchar" | "char" | "character varying" | "character" | "bpchar" | "name" => Some(0),
        "int" | "integer" | "int4" | "int8" | "int2" | "bigint" | "smallint"
        | "serial" | "bigserial" | "smallserial" => Some(1),
        "float" | "float4" | "float8" | "real" | "double precision" => Some(2),
        "numeric" | "decimal" => Some(3),
        "bool" | "boolean" => Some(4),
        _ => None,
    }
}

fn types_compatible(a: &str, b: &str) -> bool {
    let na = normalize_type(a);
    let nb = normalize_type(b);
    if na == nb {
        return true;
    }
    match (type_family(&na), type_family(&nb)) {
        (Some(fa), Some(fb)) => fa == fb,
        _ => false,
    }
}

fn tables_structurally_similar(a: &Table, b: &Table) -> bool {
    let a_names: HashSet<&str> = a.columns.iter().map(|c| c.name.as_str()).collect();
    let b_names: HashSet<&str> = b.columns.iter().map(|c| c.name.as_str()).collect();
    let min_len = a_names.len().min(b_names.len());
    if min_len == 0 {
        return a_names.is_empty() && b_names.is_empty();
    }
    let overlap = a_names.intersection(&b_names).count();
    overlap * 2 > min_len
}

fn all_clarifications_raw(ops: &[Operation]) -> Vec<Clarification> {
    let mut result = Vec::new();
    let mut dropped_cols: HashMap<&str, Vec<&Column>> = HashMap::new();
    let mut added_cols: HashMap<&str, Vec<&Column>> = HashMap::new();
    let mut dropped_tables: Vec<&Table> = Vec::new();
    let mut created_tables: Vec<&Table> = Vec::new();

    for op in ops {
        match op {
            Operation::DropColumn { table_name, column, .. } => {
                dropped_cols.entry(table_name.as_str()).or_default().push(column);
            }
            Operation::AddColumn { table_name, column } => {
                added_cols.entry(table_name.as_str()).or_default().push(column);
            }
            Operation::DropTable { table } => dropped_tables.push(table),
            Operation::CreateTable { table } => created_tables.push(table),
            _ => {}
        }
    }

    let mut table_names: Vec<&str> = dropped_cols.keys().copied().collect();
    table_names.sort();

    for table_name in table_names {
        let drops = &dropped_cols[table_name];
        let adds = added_cols.get(table_name).map(|v| v.as_slice()).unwrap_or(&[]);
        let mut sorted_drops = drops.to_vec();
        sorted_drops.sort_by_key(|c| c.name.as_str());

        for dropped in sorted_drops {
            let mut candidates: Vec<String> = adds
                .iter()
                .filter(|a| a.name != dropped.name && types_compatible(&dropped.col_type, &a.col_type))
                .map(|a| a.name.clone())
                .collect();
            candidates.sort();

            if !candidates.is_empty() {
                result.push(Clarification {
                    id: format!("rename_col:{}:{}", table_name, dropped.name),
                    severity: Severity::Suggestion,
                    kind: ClarificationKind::RenameColumn {
                        table: table_name.to_string(),
                        old: dropped.name.clone(),
                        candidates,
                    },
                });
            }
        }
    }

    let mut sorted_dropped_tables = dropped_tables.clone();
    sorted_dropped_tables.sort_by_key(|t| t.name.as_str());

    for dropped in sorted_dropped_tables {
        let mut candidates: Vec<String> = created_tables
            .iter()
            .filter(|ct| ct.name != dropped.name && tables_structurally_similar(dropped, ct))
            .map(|ct| ct.name.clone())
            .collect();
        candidates.sort();

        if !candidates.is_empty() {
            result.push(Clarification {
                id: format!("rename_table:{}", dropped.name),
                severity: Severity::Suggestion,
                kind: ClarificationKind::RenameTable {
                    old: dropped.name.clone(),
                    candidates,
                },
            });
        }
    }

    for op in ops {
        match op {
            Operation::AddColumn { table_name, column }
                if !column.nullable && column.default.is_none() && !column.primary_key =>
            {
                result.push(Clarification {
                    id: format!("notnull_add:{}:{}", table_name, column.name),
                    severity: Severity::Fatal,
                    kind: ClarificationKind::NotNullAdd {
                        table: table_name.clone(),
                        column: column.name.clone(),
                        col_type: column.col_type.clone(),
                    },
                });
            }
            Operation::AlterColumn { table_name, old, new, .. } => {
                if old.nullable && !new.nullable {
                    result.push(Clarification {
                        id: format!("notnull_change:{}:{}", table_name, old.name),
                        severity: Severity::Fatal,
                        kind: ClarificationKind::NotNullChange {
                            table: table_name.clone(),
                            column: old.name.clone(),
                        },
                    });
                }
                if normalize_type(&old.col_type) != normalize_type(&new.col_type) {
                    result.push(Clarification {
                        id: format!("typecast:{}:{}", table_name, old.name),
                        severity: Severity::Warning,
                        kind: ClarificationKind::TypeCast {
                            table: table_name.clone(),
                            column: old.name.clone(),
                            from: old.col_type.clone(),
                            to: new.col_type.clone(),
                        },
                    });
                }
            }
            _ => {}
        }
    }

    result.sort_by(|a, b| a.id.cmp(&b.id));
    result
}

fn gather_clarifications(ops: &[Operation], decisions: &[Decision]) -> Vec<Clarification> {
    let raw = all_clarifications_raw(ops);
    let decided_ids: HashSet<&str> = decisions.iter().map(|d| d.clarification_id.as_str()).collect();
    let claimed: HashSet<&str> = decisions.iter().filter_map(|d| {
        if let Answer::RenameTo(name) = &d.answer { Some(name.as_str()) } else { None }
    }).collect();

    raw.into_iter()
        .filter(|c| !decided_ids.contains(c.id.as_str()))
        .filter_map(|mut c| {
            match &mut c.kind {
                ClarificationKind::RenameColumn { candidates, .. }
                | ClarificationKind::RenameTable { candidates, .. } => {
                    candidates.retain(|n| !claimed.contains(n.as_str()));
                    if candidates.is_empty() { None } else { Some(c) }
                }
                _ => Some(c),
            }
        })
        .collect()
}

fn validate_answer(clar: &Clarification, answer: &Answer) -> Result<(), DisambiguatorError> {
    match (&clar.kind, answer) {
        (ClarificationKind::RenameColumn { candidates, .. }, Answer::RenameTo(name)) => {
            if !candidates.contains(name) {
                return Err(DisambiguatorError::InvalidCandidate {
                    id: clar.id.clone(),
                    chosen: name.clone(),
                });
            }
        }
        (ClarificationKind::RenameColumn { .. }, Answer::RenameNo) => {}
        (ClarificationKind::RenameTable { candidates, .. }, Answer::RenameTo(name)) => {
            if !candidates.contains(name) {
                return Err(DisambiguatorError::InvalidCandidate {
                    id: clar.id.clone(),
                    chosen: name.clone(),
                });
            }
        }
        (ClarificationKind::RenameTable { .. }, Answer::RenameNo) => {}
        (ClarificationKind::NotNullAdd { .. }, Answer::NotNullDefault(_))
        | (ClarificationKind::NotNullAdd { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullAdd { .. }, Answer::NotNullManual) => {}
        (ClarificationKind::NotNullChange { .. }, Answer::NotNullDefault(_))
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullNullable)
        | (ClarificationKind::NotNullChange { .. }, Answer::NotNullManual) => {}
        (ClarificationKind::TypeCast { .. }, Answer::TypeCast(_))
        | (ClarificationKind::TypeCast { .. }, Answer::TypeCastImplicit) => {}
        _ => {
            return Err(DisambiguatorError::InvalidAnswer { id: clar.id.clone() });
        }
    }
    Ok(())
}

fn apply_decisions(
    ops: &[Operation],
    decisions: &[Decision],
) -> Result<Vec<Operation>, DisambiguatorError> {
    let raw = all_clarifications_raw(ops);
    let clar_map: HashMap<&str, &Clarification> = raw.iter().map(|c| (c.id.as_str(), c)).collect();

    for decision in decisions {
        let clar = clar_map
            .get(decision.clarification_id.as_str())
            .ok_or_else(|| DisambiguatorError::UnknownDecision(decision.clarification_id.clone()))?;
        validate_answer(clar, &decision.answer)?;
    }

    let decision_map: HashMap<&str, &Answer> = decisions
        .iter()
        .map(|d| (d.clarification_id.as_str(), &d.answer))
        .collect();

    let mut col_renames: HashMap<(&str, &str), &str> = HashMap::new();
    let mut rename_col_targets: HashSet<(&str, &str)> = HashSet::new();
    let mut table_renames: HashMap<&str, &str> = HashMap::new();
    let mut rename_table_targets: HashSet<&str> = HashSet::new();

    for decision in decisions {
        if let Answer::RenameTo(new_name) = &decision.answer {
            if let Some(clar) = clar_map.get(decision.clarification_id.as_str()) {
                match &clar.kind {
                    ClarificationKind::RenameColumn { table, old, .. } => {
                        col_renames.insert((table.as_str(), old.as_str()), new_name.as_str());
                        rename_col_targets.insert((table.as_str(), new_name.as_str()));
                    }
                    ClarificationKind::RenameTable { old, .. } => {
                        table_renames.insert(old.as_str(), new_name.as_str());
                        rename_table_targets.insert(new_name.as_str());
                    }
                    _ => {}
                }
            }
        }
    }

    let mut result = Vec::with_capacity(ops.len());

    for op in ops {
        match op {
            Operation::DropColumn { table_name, column, .. } => {
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
                let id = format!("notnull_add:{}:{}", table_name, column.name);
                let mut col = column.clone();
                if let Some(answer) = decision_map.get(id.as_str()) {
                    match answer {
                        Answer::NotNullDefault(val) => col.default = Some(val.clone()),
                        Answer::NotNullNullable => col.nullable = true,
                        _ => {}
                    }
                }
                result.push(Operation::AddColumn { table_name: table_name.clone(), column: col });
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
            Operation::AlterColumn { table_name, old, new, cast_expr } => {
                let mut new_col = new.clone();
                let mut cast = cast_expr.clone();

                if old.nullable && !new.nullable {
                    let id = format!("notnull_change:{}:{}", table_name, old.name);
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
                    let id = format!("typecast:{}:{}", table_name, old.name);
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
            _ => result.push(op.clone()),
        }
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::Column;

    fn col(name: &str, t: &str, nullable: bool) -> Column {
        Column {
            name: name.to_string(),
            col_type: t.to_string(),
            nullable,
            default: None,
            primary_key: false,
            references: None,
            check: None,
        }
    }

    fn decision(id: &str, answer: Answer) -> Decision {
        Decision { clarification_id: id.to_string(), answer }
    }

    fn get_clarifications(ops: &[Operation]) -> Vec<Clarification> {
        let d = Disambiguator;
        match d.process(ops, &[]).unwrap() {
            DisambiguationResult::NeedsInput(c) => c,
            DisambiguationResult::Resolved(_) => vec![],
        }
    }

    /// Verifies that dropping a column and adding one with a compatible type on the same table
    /// produces a single RenameColumn suggestion with one candidate.
    #[test]
    fn test_rename_single_candidate() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "users".into(),
                column: col("email", "varchar", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("email_address", "text", true),
            },
        ];
        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        assert_eq!(clar[0].id, "rename_col:users:email");
        assert_eq!(clar[0].severity, Severity::Suggestion);
        match &clar[0].kind {
            ClarificationKind::RenameColumn { table, old, candidates } => {
                assert_eq!(table, "users");
                assert_eq!(old, "email");
                assert_eq!(candidates, &["email_address"]);
            }
            _ => panic!("expected RenameColumn"),
        }
    }

    /// Verifies that when multiple added columns are compatible rename candidates for a dropped
    /// column, they all appear in one clarification and are sorted alphabetically.
    #[test]
    fn test_rename_multiple_candidates_sorted() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "users".into(),
                column: col("name", "varchar", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("full_name", "text", true),
            },
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("display_name", "varchar", true),
            },
        ];
        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        match &clar[0].kind {
            ClarificationKind::RenameColumn { candidates, .. } => {
                assert_eq!(candidates, &["display_name", "full_name"]);
            }
            _ => panic!("expected RenameColumn"),
        }
    }

    /// Verifies that a dropped text column and an added integer column on the same table
    /// do not produce a rename suggestion since the types are incompatible.
    #[test]
    fn test_rename_incompatible_types_no_candidate() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "users".into(),
                column: col("age", "text", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("score", "integer", true),
            },
        ];
        let clar = get_clarifications(&ops);
        assert!(clar.is_empty());
    }

    /// Verifies that adding a NOT NULL column with no default produces a Fatal NotNullAdd clarification.
    #[test]
    fn test_notnull_add() {
        let ops = vec![Operation::AddColumn {
            table_name: "orders".into(),
            column: col("reference_id", "integer", false),
        }];
        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        assert_eq!(clar[0].severity, Severity::Fatal);
        assert_eq!(clar[0].id, "notnull_add:orders:reference_id");
        match &clar[0].kind {
            ClarificationKind::NotNullAdd { table, column, col_type } => {
                assert_eq!(table, "orders");
                assert_eq!(column, "reference_id");
                assert_eq!(col_type, "integer");
            }
            _ => panic!("expected NotNullAdd"),
        }
    }

    /// Verifies that altering a nullable column to NOT NULL produces a Fatal NotNullChange clarification.
    #[test]
    fn test_notnull_change() {
        let ops = vec![Operation::AlterColumn {
            table_name: "users".into(),
            old: col("status", "text", true),
            new: col("status", "text", false),
            cast_expr: None,
        }];
        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        assert_eq!(clar[0].severity, Severity::Fatal);
        assert_eq!(clar[0].id, "notnull_change:users:status");
    }

    /// Verifies that altering a column from text to integer produces a Warning TypeCast clarification.
    #[test]
    fn test_typecast() {
        let ops = vec![Operation::AlterColumn {
            table_name: "products".into(),
            old: col("price", "text", true),
            new: col("price", "integer", true),
            cast_expr: None,
        }];
        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        assert_eq!(clar[0].severity, Severity::Warning);
        assert_eq!(clar[0].id, "typecast:products:price");
    }

    /// Verifies that reordering operations does not change the clarification IDs produced.
    #[test]
    fn test_stable_ids() {
        let ops_a = vec![
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("x", "int", false),
            },
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("y", "varchar", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("z", "text", true),
            },
        ];
        let ops_b = vec![
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("y", "varchar", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("z", "text", true),
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("x", "int", false),
            },
        ];
        let ids_a: Vec<String> =
            get_clarifications(&ops_a).into_iter().map(|c| c.id).collect();
        let ids_b: Vec<String> =
            get_clarifications(&ops_b).into_iter().map(|c| c.id).collect();
        assert_eq!(ids_a, ids_b);
    }

    /// Verifies that when one rename decision has already claimed a candidate name, that name
    /// is excluded from other pending rename clarifications in the same round.
    #[test]
    fn test_claimed_candidates_excluded() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("a", "text", true),
                cascade: false,
            },
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("b", "text", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("alpha", "text", true),
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("beta", "text", true),
            },
        ];
        let d = Disambiguator;
        let first = match d.process(&ops, &[]).unwrap() {
            DisambiguationResult::NeedsInput(c) => c,
            _ => panic!("expected NeedsInput"),
        };
        assert_eq!(first.len(), 2);

        let d1 = decision("rename_col:t:a", Answer::RenameTo("alpha".into()));
        let second = match d.process(&ops, &[d1]).unwrap() {
            DisambiguationResult::NeedsInput(c) => c,
            _ => panic!("expected NeedsInput"),
        };
        assert_eq!(second.len(), 1);
        match &second[0].kind {
            ClarificationKind::RenameColumn { candidates, .. } => {
                assert!(!candidates.contains(&"alpha".to_string()), "alpha must be excluded");
                assert!(candidates.contains(&"beta".to_string()));
            }
            _ => panic!("expected RenameColumn"),
        }
    }

    /// Verifies the full process() loop: starts with NeedsInput, reaches Resolved after answering,
    /// and the output ops contain a RenameColumn instead of the original Drop+Add pair.
    #[test]
    fn test_incremental_round_trip() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "users".into(),
                column: col("old_email", "varchar", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("new_email", "text", true),
            },
        ];
        let d = Disambiguator;
        assert!(matches!(
            d.process(&ops, &[]).unwrap(),
            DisambiguationResult::NeedsInput(_)
        ));

        let decisions =
            vec![decision("rename_col:users:old_email", Answer::RenameTo("new_email".into()))];
        match d.process(&ops, &decisions).unwrap() {
            DisambiguationResult::Resolved(final_ops) => {
                assert_eq!(final_ops.len(), 1);
                match &final_ops[0] {
                    Operation::RenameColumn { table_name, old_name, new_name } => {
                        assert_eq!(table_name, "users");
                        assert_eq!(old_name, "old_email");
                        assert_eq!(new_name, "new_email");
                    }
                    _ => panic!("expected RenameColumn"),
                }
            }
            _ => panic!("expected Resolved"),
        }
    }

    /// Verifies that answering NotNullDefault for a NotNullChange clarification injects
    /// a backfill UPDATE statement immediately before the AlterColumn op.
    #[test]
    fn test_backfill_injection() {
        let ops = vec![Operation::AlterColumn {
            table_name: "users".into(),
            old: col("status", "text", true),
            new: col("status", "text", false),
            cast_expr: None,
        }];
        let decisions =
            vec![decision("notnull_change:users:status", Answer::NotNullDefault("'active'".into()))];
        let d = Disambiguator;
        match d.process(&ops, &decisions).unwrap() {
            DisambiguationResult::Resolved(result) => {
                assert_eq!(result.len(), 2);
                match &result[0] {
                    Operation::Statement { up, .. } => {
                        assert!(up.contains("UPDATE"), "expected UPDATE backfill");
                        assert!(up.contains("'active'"));
                        assert!(up.contains("IS NULL"));
                    }
                    _ => panic!("expected Statement backfill"),
                }
                assert!(matches!(&result[1], Operation::AlterColumn { .. }));
            }
            _ => panic!("expected Resolved"),
        }
    }

    /// Verifies that process returns UnknownDecision when a decision references
    /// a clarification ID that does not correspond to any op.
    #[test]
    fn test_error_unknown_decision() {
        let d = Disambiguator;
        let err = d
            .process(&[], &[decision("rename_col:t:x", Answer::RenameNo)])
            .unwrap_err();
        assert_eq!(err, DisambiguatorError::UnknownDecision("rename_col:t:x".into()));
    }

    /// Verifies that process returns InvalidCandidate when a RenameTo answer
    /// names a column that is not in the candidate list.
    #[test]
    fn test_error_invalid_candidate() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("x", "text", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("y", "text", true),
            },
        ];
        let d = Disambiguator;
        let err = d
            .process(&ops, &[decision("rename_col:t:x", Answer::RenameTo("nonexistent".into()))])
            .unwrap_err();
        assert!(matches!(err, DisambiguatorError::InvalidCandidate { .. }));
    }

    /// Verifies that process returns InvalidAnswer when the answer variant is incompatible
    /// with the clarification kind (e.g. NotNullDefault for a rename clarification).
    #[test]
    fn test_error_invalid_answer() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "t".into(),
                column: col("x", "text", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "t".into(),
                column: col("y", "text", true),
            },
        ];
        let d = Disambiguator;
        let err = d
            .process(&ops, &[decision("rename_col:t:x", Answer::NotNullDefault("0".into()))])
            .unwrap_err();
        assert!(matches!(err, DisambiguatorError::InvalidAnswer { .. }));
    }
}
