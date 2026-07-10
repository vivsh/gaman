use super::model::{Answer, Clarification, ClarificationKind, Severity};

/// A single selectable option presented to the user.
/// `Fixed` options resolve to an `Answer` immediately.
/// `RequiresInput` options need the user to type a value; `make_answer` converts it to the
/// correct `Answer` variant at message-build time.
#[derive(Clone)]
pub enum OptionAction {
    Fixed(Answer),
    RequiresInput {
        prompt: String,
        make_answer: fn(String) -> Answer,
    },
}

impl std::fmt::Debug for OptionAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fixed(a) => write!(f, "Fixed({:?})", a),
            Self::RequiresInput { prompt, .. } => {
                write!(f, "RequiresInput {{ prompt: {:?} }}", prompt)
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct ClarificationOption {
    pub label: String,
    pub action: OptionAction,
}

/// The full, transport-agnostic representation of a clarification prompt.
/// Engines use `description` as the question header and `options` as numbered choices.
#[derive(Debug, Clone)]
pub struct ClarificationMessage {
    pub description: String,
    pub options: Vec<ClarificationOption>,
}

/// Builds the display message for a clarification. Keep human-facing copy here so wording
/// can be edited without touching clarification analysis or operation rewriting.
pub fn clarification_message(clar: &Clarification) -> ClarificationMessage {
    let tag = severity_tag(&clar.severity);
    match &clar.kind {
        ClarificationKind::RenameColumn {
            table,
            old,
            candidates,
        } => {
            let description = format!(
                "{} Column '{}' was removed from '{}'. Was it renamed?",
                tag, old, table
            );
            let mut options: Vec<ClarificationOption> = candidates
                .iter()
                .map(|c| ClarificationOption {
                    label: c.clone(),
                    action: OptionAction::Fixed(Answer::RenameTo(c.clone())),
                })
                .collect();
            options.push(ClarificationOption {
                label: "No, it was dropped".to_string(),
                action: OptionAction::Fixed(Answer::RenameNo),
            });
            ClarificationMessage {
                description,
                options,
            }
        }
        ClarificationKind::RenameTable { old, candidates } => {
            let description = format!("{} Table '{}' was removed. Was it renamed?", tag, old);
            let mut options: Vec<ClarificationOption> = candidates
                .iter()
                .map(|c| ClarificationOption {
                    label: c.clone(),
                    action: OptionAction::Fixed(Answer::RenameTo(c.clone())),
                })
                .collect();
            options.push(ClarificationOption {
                label: "No, it was dropped".to_string(),
                action: OptionAction::Fixed(Answer::RenameNo),
            });
            ClarificationMessage {
                description,
                options,
            }
        }
        ClarificationKind::RenameEnumValue {
            enum_name,
            old,
            candidates,
        } => {
            let description = format!(
                "{} Enum value '{}' was removed from '{}'. Was it renamed?",
                tag, old, enum_name
            );
            let mut options: Vec<ClarificationOption> = candidates
                .iter()
                .map(|c| ClarificationOption {
                    label: c.clone(),
                    action: OptionAction::Fixed(Answer::RenameTo(c.clone())),
                })
                .collect();
            options.push(ClarificationOption {
                label: "No, it was removed".to_string(),
                action: OptionAction::Fixed(Answer::RenameNo),
            });
            ClarificationMessage {
                description,
                options,
            }
        }
        ClarificationKind::NotNullAdd {
            table,
            column,
            col_type,
        } => {
            let description = format!(
                "{} Column '{}' ({}) on '{}' is NOT NULL with no default.",
                tag, column, col_type, table
            );
            ClarificationMessage {
                description,
                options: vec![
                    ClarificationOption {
                        label: "Provide a one-off default SQL value (e.g. 0, '', now())"
                            .to_string(),
                        action: OptionAction::RequiresInput {
                            prompt: "Default value:".to_string(),
                            make_answer: Answer::NotNullDefault,
                        },
                    },
                    ClarificationOption {
                        label: "Make it nullable instead".to_string(),
                        action: OptionAction::Fixed(Answer::NotNullNullable),
                    },
                    ClarificationOption {
                        label: "Handle manually (will fail on non-empty tables)".to_string(),
                        action: OptionAction::Fixed(Answer::NotNullManual),
                    },
                ],
            }
        }
        ClarificationKind::NotNullChange { table, column } => {
            let description = format!(
                "{} Column '{}' on '{}' changed from nullable to NOT NULL.",
                tag, column, table
            );
            ClarificationMessage {
                description,
                options: vec![
                    ClarificationOption {
                        label: "Provide a backfill default for existing NULL rows".to_string(),
                        action: OptionAction::RequiresInput {
                            prompt: "Backfill value:".to_string(),
                            make_answer: Answer::NotNullDefault,
                        },
                    },
                    ClarificationOption {
                        label: "Keep it nullable instead".to_string(),
                        action: OptionAction::Fixed(Answer::NotNullNullable),
                    },
                    ClarificationOption {
                        label: "Handle manually".to_string(),
                        action: OptionAction::Fixed(Answer::NotNullManual),
                    },
                ],
            }
        }
        ClarificationKind::TypeCast {
            table,
            column,
            from,
            to,
        } => {
            let description = format!(
                "{} Column '{}' on '{}' changed type: {} -> {}.",
                tag, column, table, from, to
            );
            ClarificationMessage {
                description,
                options: vec![
                    ClarificationOption {
                        label: "Provide a CAST expression (e.g. col::integer)".to_string(),
                        action: OptionAction::RequiresInput {
                            prompt: "CAST expression:".to_string(),
                            make_answer: Answer::TypeCast,
                        },
                    },
                    ClarificationOption {
                        label: "Use implicit cast (may fail at apply time)".to_string(),
                        action: OptionAction::Fixed(Answer::TypeCastImplicit),
                    },
                ],
            }
        }
        ClarificationKind::UnknownType {
            table,
            column,
            type_name,
            suggested,
        } => {
            let description = format!(
                "{} Column '{}' on '{}' uses unknown type '{}'.",
                tag, column, table, type_name
            );
            let mut options = suggested
                .iter()
                .map(|suggestion| ClarificationOption {
                    label: format!("Use {suggestion}"),
                    action: OptionAction::Fixed(Answer::UseType(suggestion.clone())),
                })
                .collect::<Vec<_>>();
            options.push(ClarificationOption {
                label: format!("Keep {type_name} as a custom/domain/extension type"),
                action: OptionAction::Fixed(Answer::KeepType),
            });
            ClarificationMessage {
                description,
                options,
            }
        }
        ClarificationKind::OpaqueEntity { kind, name } => {
            let description = format!(
                "{} {:?} '{}' is managed as opaque SQL; changes use coarse create/drop/replace semantics.",
                tag, kind, name
            );
            ClarificationMessage {
                description,
                options: vec![ClarificationOption {
                    label: "Accept opaque SQL management for this object".to_string(),
                    action: OptionAction::Fixed(Answer::AcceptRisk),
                }],
            }
        }
        ClarificationKind::UnmanagedTableOptions { table } => {
            let description = format!(
                "{} Table '{}' has unmanaged table options that Gaman will preserve but not model granularly.",
                tag, table
            );
            ClarificationMessage {
                description,
                options: vec![ClarificationOption {
                    label: "Accept unmanaged table options".to_string(),
                    action: OptionAction::Fixed(Answer::AcceptRisk),
                }],
            }
        }
    }
}

fn severity_tag(s: &Severity) -> &'static str {
    match s {
        Severity::Fatal => "[FATAL]",
        Severity::Warning => "[WARNING]",
        Severity::Suggestion => "[suggest]",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_nonempty_message(message: &ClarificationMessage) {
        assert!(!message.description.is_empty());
        assert!(!message.options.is_empty());
        for option in &message.options {
            assert!(!option.label.is_empty());
        }
    }

    /// Verifies rename-column messages expose stable answer semantics without locking prompt copy.
    #[test]
    fn rename_column_message_maps_options_to_answers() {
        let message = clarification_message(&Clarification {
            id: "rename_col:users:email".to_string(),
            severity: Severity::Suggestion,
            kind: ClarificationKind::RenameColumn {
                table: "users".to_string(),
                old: "email".to_string(),
                candidates: vec!["email_address".to_string()],
            },
        });

        assert_nonempty_message(&message);
        assert!(matches!(
            &message.options[0].action,
            OptionAction::Fixed(Answer::RenameTo(name)) if name == "email_address"
        ));
        assert!(matches!(
            &message.options[1].action,
            OptionAction::Fixed(Answer::RenameNo)
        ));
    }

    /// Verifies not-null-add messages expose all required action shapes without locking prompt copy.
    #[test]
    fn not_null_add_message_maps_options_to_answers() {
        let message = clarification_message(&Clarification {
            id: "notnull_add:orders:reference_id".to_string(),
            severity: Severity::Fatal,
            kind: ClarificationKind::NotNullAdd {
                table: "orders".to_string(),
                column: "reference_id".to_string(),
                col_type: "integer".to_string(),
            },
        });

        assert_nonempty_message(&message);
        assert_eq!(message.options.len(), 3);
        assert!(matches!(
            message.options[0].action,
            OptionAction::RequiresInput { .. }
        ));
        assert!(matches!(
            message.options[1].action,
            OptionAction::Fixed(Answer::NotNullNullable)
        ));
        assert!(matches!(
            message.options[2].action,
            OptionAction::Fixed(Answer::NotNullManual)
        ));
    }

    /// Verifies every clarification kind can be rendered as a non-empty message.
    #[test]
    fn every_clarification_kind_has_message_spec() {
        let clarifications = vec![
            Clarification {
                id: "rename_table:users".to_string(),
                severity: Severity::Suggestion,
                kind: ClarificationKind::RenameTable {
                    old: "users".to_string(),
                    candidates: vec!["accounts".to_string()],
                },
            },
            Clarification {
                id: "rename_enum_value:status:live".to_string(),
                severity: Severity::Warning,
                kind: ClarificationKind::RenameEnumValue {
                    enum_name: "status".to_string(),
                    old: "live".to_string(),
                    candidates: vec!["published".to_string()],
                },
            },
            Clarification {
                id: "notnull_change:users:status".to_string(),
                severity: Severity::Fatal,
                kind: ClarificationKind::NotNullChange {
                    table: "users".to_string(),
                    column: "status".to_string(),
                },
            },
            Clarification {
                id: "typecast:products:price".to_string(),
                severity: Severity::Warning,
                kind: ClarificationKind::TypeCast {
                    table: "products".to_string(),
                    column: "price".to_string(),
                    from: "text".to_string(),
                    to: "integer".to_string(),
                },
            },
            Clarification {
                id: "unknown_type:users:age".to_string(),
                severity: Severity::Warning,
                kind: ClarificationKind::UnknownType {
                    table: "users".to_string(),
                    column: "age".to_string(),
                    type_name: "intger".to_string(),
                    suggested: vec!["integer".to_string()],
                },
            },
        ];

        for clarification in clarifications {
            let message = clarification_message(&clarification);
            assert_nonempty_message(&message);
        }
    }

    /// Verifies unknown-type messages expose suggested and custom-type answers without locking copy.
    #[test]
    fn unknown_type_message_maps_options_to_answers() {
        let message = clarification_message(&Clarification {
            id: "unknown_type:users:age".to_string(),
            severity: Severity::Warning,
            kind: ClarificationKind::UnknownType {
                table: "users".to_string(),
                column: "age".to_string(),
                type_name: "intger".to_string(),
                suggested: vec!["integer".to_string()],
            },
        });

        assert_nonempty_message(&message);
        assert!(matches!(
            &message.options[0].action,
            OptionAction::Fixed(Answer::UseType(type_name)) if type_name == "integer"
        ));
        assert!(matches!(
            &message.options[1].action,
            OptionAction::Fixed(Answer::KeepType)
        ));
    }
}
