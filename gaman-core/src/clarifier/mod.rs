mod analyze;
pub(crate) mod ids;
pub mod messages;
mod model;
mod resolve;
#[doc(hidden)]
pub mod type_resolution;
mod types;

pub use messages::{
    ClarificationMessage, ClarificationOption, OptionAction, clarification_message,
};
pub use model::{
    Answer, Clarification, ClarificationKind, ClarifyError, ClarifyResult, Decision, PromptEngine,
    PromptError, Severity,
};
#[doc(hidden)]
pub use type_resolution::{TypeResolution, non_type_decisions, resolve_unknown_types};

use crate::operations::Operation;
use resolve::ClarifyPlan;

pub struct Clarifier;

impl Clarifier {
    pub fn process(
        &self,
        ops: &[Operation],
        decisions: &[Decision],
    ) -> Result<ClarifyResult, ClarifyError> {
        ClarifyPlan::new(ops).process(ops, decisions)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::states::{Column, EnumDef, Table};

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
            generated_storage: None,
            dialect_options: Default::default(),
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

    fn enum_def(values: &[&str]) -> EnumDef {
        EnumDef {
            name: "status".to_string(),
            schema: None,
            values: values.iter().map(|value| value.to_string()).collect(),
            opaque: Default::default(),
        }
    }

    fn decision(id: &str, answer: Answer) -> Decision {
        Decision {
            clarification_id: id.to_string(),
            answer,
        }
    }

    fn get_clarifications(ops: &[Operation]) -> Vec<Clarification> {
        let d = Clarifier;
        match d.process(ops, &[]).unwrap() {
            ClarifyResult::NeedsInput(c) => c,
            ClarifyResult::Resolved(_) => vec![],
        }
    }

    #[test]
    fn rename_column_candidates_are_deterministic_and_ranked() {
        let ops = vec![
            Operation::AddColumn {
                table_name: "users".into(),
                column: col("display_name", "text", true),
            },
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
            ClarificationKind::RenameColumn {
                table,
                old,
                candidates,
            } => {
                assert_eq!(table, "users");
                assert_eq!(old, "email");
                assert_eq!(candidates, &["email_address", "display_name"]);
            }
            _ => panic!("expected RenameColumn"),
        }
    }

    #[test]
    fn rename_column_incompatible_type_has_no_candidate() {
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
        assert!(get_clarifications(&ops).is_empty());
    }

    #[test]
    fn rename_table_candidates_are_ranked_by_structure() {
        let ops = vec![
            Operation::DropTable {
                table: table(
                    "users",
                    vec![col("id", "integer", false), col("email", "text", true)],
                ),
            },
            Operation::CreateTable {
                table: table("accounts", vec![col("id", "integer", false)]),
            },
            Operation::CreateTable {
                table: table(
                    "members",
                    vec![col("id", "integer", false), col("email", "text", true)],
                ),
            },
        ];

        let clar = get_clarifications(&ops);
        assert_eq!(clar.len(), 1);
        match &clar[0].kind {
            ClarificationKind::RenameTable { old, candidates } => {
                assert_eq!(old, "users");
                assert_eq!(candidates[0], "members");
            }
            _ => panic!("expected RenameTable"),
        }
    }

    #[test]
    fn enum_value_rename_decision_rewrites_drop_create() {
        let ops = vec![
            Operation::DropEnum {
                enum_def: enum_def(&["draft", "live"]),
            },
            Operation::CreateEnum {
                enum_def: enum_def(&["draft", "published", "archived"]),
            },
        ];
        let d = Clarifier;
        let resolved = match d
            .process(
                &ops,
                &[decision(
                    "rename_enum_value:status:live",
                    Answer::RenameTo("published".to_string()),
                )],
            )
            .unwrap()
        {
            ClarifyResult::Resolved(ops) => ops,
            ClarifyResult::NeedsInput(c) => panic!("unexpected clarification: {c:?}"),
        };
        assert_eq!(resolved.len(), 2);
        assert!(matches!(
            &resolved[0],
            Operation::RenameEnumValue {
                enum_name,
                old_value,
                new_value,
                ..
            } if enum_name == "status" && old_value == "live" && new_value == "published"
        ));
        assert!(matches!(&resolved[1], Operation::AlterEnum { .. }));
    }

    #[test]
    fn not_null_and_typecast_clarifications_are_detected() {
        let ops = vec![
            Operation::AddColumn {
                table_name: "orders".into(),
                column: col("reference_id", "integer", false),
            },
            Operation::AlterColumn {
                table_name: "products".into(),
                old: col("price", "text", true),
                new: col("price", "integer", true),
                cast_expr: None,
            },
        ];

        let ids: Vec<String> = get_clarifications(&ops)
            .into_iter()
            .map(|clarification| clarification.id)
            .collect();
        assert_eq!(
            ids,
            ["notnull_add:orders:reference_id", "typecast:products:price"]
        );
    }

    #[test]
    fn decisions_rewrite_rename_and_backfill_operations() {
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
            Operation::AlterColumn {
                table_name: "users".into(),
                old: col("status", "text", true),
                new: col("status", "text", false),
                cast_expr: None,
            },
        ];
        let decisions = vec![
            decision(
                "rename_col:users:old_email",
                Answer::RenameTo("new_email".into()),
            ),
            decision(
                "notnull_change:users:status",
                Answer::NotNullDefault("'active'".into()),
            ),
        ];

        let resolved = match Clarifier.process(&ops, &decisions).unwrap() {
            ClarifyResult::Resolved(ops) => ops,
            ClarifyResult::NeedsInput(c) => panic!("unexpected clarification: {c:?}"),
        };

        assert!(matches!(resolved[0], Operation::RenameColumn { .. }));
        assert!(matches!(resolved[1], Operation::Statement { .. }));
        assert!(matches!(resolved[2], Operation::AlterColumn { .. }));
    }

    #[test]
    fn partial_decisions_return_only_still_pending_clarifications() {
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

        let pending = match Clarifier
            .process(
                &ops,
                &[decision("rename_col:t:a", Answer::RenameTo("alpha".into()))],
            )
            .unwrap()
        {
            ClarifyResult::NeedsInput(c) => c,
            _ => panic!("expected NeedsInput"),
        };

        assert_eq!(pending.len(), 1);
        match &pending[0].kind {
            ClarificationKind::RenameColumn { candidates, .. } => {
                assert!(!candidates.contains(&"alpha".to_string()));
                assert!(candidates.contains(&"beta".to_string()));
            }
            _ => panic!("expected RenameColumn"),
        }
    }

    #[test]
    fn duplicate_decision_ids_fail() {
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
        let err = Clarifier
            .process(
                &ops,
                &[
                    decision("rename_col:t:x", Answer::RenameNo),
                    decision("rename_col:t:x", Answer::RenameNo),
                ],
            )
            .unwrap_err();
        assert_eq!(
            err,
            ClarifyError::DuplicateDecision("rename_col:t:x".to_string())
        );
    }

    #[test]
    fn conflicting_rename_targets_fail_before_rewrite() {
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
        ];
        let err = Clarifier
            .process(
                &ops,
                &[
                    decision("rename_col:t:a", Answer::RenameTo("alpha".into())),
                    decision("rename_col:t:b", Answer::RenameTo("alpha".into())),
                ],
            )
            .unwrap_err();
        assert!(matches!(err, ClarifyError::ConflictingRenameTarget { .. }));
    }

    #[test]
    fn empty_default_or_cast_input_fails() {
        let not_null_ops = vec![Operation::AddColumn {
            table_name: "orders".into(),
            column: col("reference_id", "integer", false),
        }];
        let err = Clarifier
            .process(
                &not_null_ops,
                &[decision(
                    "notnull_add:orders:reference_id",
                    Answer::NotNullDefault("  ".into()),
                )],
            )
            .unwrap_err();
        assert!(matches!(err, ClarifyError::EmptyInput { .. }));

        let typecast_ops = vec![Operation::AlterColumn {
            table_name: "products".into(),
            old: col("price", "text", true),
            new: col("price", "integer", true),
            cast_expr: None,
        }];
        let err = Clarifier
            .process(
                &typecast_ops,
                &[decision(
                    "typecast:products:price",
                    Answer::TypeCast("".into()),
                )],
            )
            .unwrap_err();
        assert!(matches!(err, ClarifyError::EmptyInput { .. }));
    }

    #[test]
    fn ids_escape_names_containing_separators() {
        let ops = vec![
            Operation::DropColumn {
                table_name: "tenant:users".into(),
                column: col("email:old", "text", true),
                cascade: false,
            },
            Operation::AddColumn {
                table_name: "tenant:users".into(),
                column: col("email:new", "text", true),
            },
        ];
        let clar = get_clarifications(&ops);
        assert_eq!(clar[0].id, "rename_col:tenant%3Ausers:email%3Aold");
    }

    #[test]
    fn invalid_decision_errors_are_preserved() {
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

        let unknown = Clarifier
            .process(&[], &[decision("rename_col:t:x", Answer::RenameNo)])
            .unwrap_err();
        assert_eq!(
            unknown,
            ClarifyError::UnknownDecision("rename_col:t:x".into())
        );

        let invalid_candidate = Clarifier
            .process(
                &ops,
                &[decision(
                    "rename_col:t:x",
                    Answer::RenameTo("nonexistent".into()),
                )],
            )
            .unwrap_err();
        assert!(matches!(
            invalid_candidate,
            ClarifyError::InvalidCandidate { .. }
        ));

        let invalid_answer = Clarifier
            .process(
                &ops,
                &[decision(
                    "rename_col:t:x",
                    Answer::NotNullDefault("0".into()),
                )],
            )
            .unwrap_err();
        assert!(matches!(invalid_answer, ClarifyError::InvalidAnswer { .. }));
    }
}
