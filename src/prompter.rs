use std::io::{self, BufRead, Write};

use crate::disambiguator::{
    Answer, Clarification, ClarificationKind, Decision, PromptEngine, PromptError, Severity,
};

/// A single selectable option presented to the user.
/// `Fixed` options resolve to an `Answer` immediately.
/// `RequiresInput` options need the user to type a value; `make_answer` converts it to the
/// correct `Answer` variant — determined at message-build time, not at render time.
#[derive(Clone)]
pub enum OptionAction {
    Fixed(Answer),
    RequiresInput { prompt: String, make_answer: fn(String) -> Answer },
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
/// Engines use `description` as the question header and `options` as the numbered choices.
/// A `None` action in `options` means the caller must map the chosen index back to an Answer
/// by including the resolved input string.
#[derive(Debug, Clone)]
pub struct ClarificationMessage {
    pub description: String,
    pub options: Vec<ClarificationOption>,
}

/// Builds the display message for a clarification. Engine-independent — call this from any
/// PromptEngine implementation to ensure wording stays consistent across CLI, HTTP, etc.
pub fn clarification_message(clar: &Clarification) -> ClarificationMessage {
    let tag = severity_tag(&clar.severity);
    match &clar.kind {
        ClarificationKind::RenameColumn { table, old, candidates } => {
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
            ClarificationMessage { description, options }
        }
        ClarificationKind::RenameTable { old, candidates } => {
            let description =
                format!("{} Table '{}' was removed. Was it renamed?", tag, old);
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
            ClarificationMessage { description, options }
        }
        ClarificationKind::NotNullAdd { table, column, col_type } => {
            let description = format!(
                "{} Column '{}' ({}) on '{}' is NOT NULL with no default.",
                tag, column, col_type, table
            );
            ClarificationMessage {
                description,
                options: vec![
                    ClarificationOption {
                        label: "Provide a one-off default SQL value (e.g. 0, '', now())".to_string(),
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
        ClarificationKind::TypeCast { table, column, from, to } => {
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
    }
}

pub struct CliPromptEngine;

impl PromptEngine for CliPromptEngine {
    fn prompt(
        &self,
        clarifications: &[Clarification],
    ) -> Result<Vec<Decision>, PromptError> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        let mut decisions = Vec::new();

        for clar in clarifications {
            let answer = prompt_one(&mut out, &mut stdin.lock(), clar)?;
            decisions.push(Decision { clarification_id: clar.id.clone(), answer });
        }

        Ok(decisions)
    }
}

fn severity_tag(s: &Severity) -> &'static str {
    match s {
        Severity::Fatal => "[FATAL]",
        Severity::Warning => "[WARNING]",
        Severity::Suggestion => "[suggest]",
    }
}

fn prompt_one(
    out: &mut impl Write,
    input: &mut impl BufRead,
    clar: &Clarification,
) -> Result<Answer, PromptError> {
    let msg = clarification_message(clar);
    writeln!(out, "{}", msg.description)?;
    for (i, opt) in msg.options.iter().enumerate() {
        writeln!(out, "  {} - {}", i + 1, opt.label)?;
    }
    out.flush()?;
    let choice = read_choice(input, msg.options.len())?;
    let opt = &msg.options[choice - 1];
    match &opt.action {
        OptionAction::Fixed(answer) => Ok(answer.clone()),
        OptionAction::RequiresInput { prompt, make_answer } => {
            write!(out, "  {} ", prompt)?;
            out.flush()?;
            let val = read_line(input)?.trim().to_string();
            Ok(make_answer(val))
        }
    }
}

fn read_line(input: &mut impl BufRead) -> Result<String, PromptError> {
    let mut line = String::new();
    input.read_line(&mut line)?;
    Ok(line)
}

fn read_choice(input: &mut impl BufRead, max: usize) -> Result<usize, PromptError> {
    loop {
        let line = read_line(input)?;
        if let Ok(n) = line.trim().parse::<usize>() {
            if n <= max {
                return Ok(n);
            }
        }
    }
}
