use std::io::{self, BufRead, Write};

use gaman_core::clarifier::{
    Answer, Clarification, Decision, OptionAction, PromptEngine, PromptError, clarification_message,
};

pub struct CliPromptEngine;

impl PromptEngine for CliPromptEngine {
    fn prompt(&self, clarifications: &[Clarification]) -> Result<Vec<Decision>, PromptError> {
        let stdin = io::stdin();
        let stdout = io::stdout();
        let mut out = io::BufWriter::new(stdout.lock());
        let mut decisions = Vec::new();

        for clar in clarifications {
            let answer = prompt_one(&mut out, &mut stdin.lock(), clar)?;
            decisions.push(Decision {
                clarification_id: clar.id.clone(),
                answer,
            });
        }

        Ok(decisions)
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
        OptionAction::RequiresInput {
            prompt,
            make_answer,
        } => {
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
        if let Ok(n) = line.trim().parse::<usize>()
            && (1..=max).contains(&n)
        {
            return Ok(n);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;
    use gaman_core::clarifier::{ClarificationKind, Severity};

    fn rename_column_clarification() -> Clarification {
        Clarification {
            id: "rename_col:users:email".to_string(),
            severity: Severity::Suggestion,
            kind: ClarificationKind::RenameColumn {
                table: "users".to_string(),
                old: "email".to_string(),
                candidates: vec!["email_address".to_string()],
            },
        }
    }

    #[test]
    fn prompt_one_uses_message_spec_options() {
        let mut out = Vec::new();
        let mut input = Cursor::new("1\n");
        let answer = prompt_one(&mut out, &mut input, &rename_column_clarification()).unwrap();

        assert_eq!(answer, Answer::RenameTo("email_address".to_string()));
        let rendered = String::from_utf8(out).unwrap();
        assert!(rendered.contains("Column 'email' was removed from 'users'"));
        assert!(rendered.contains("1 - email_address"));
    }

    #[test]
    fn prompt_one_rejects_zero_choice_without_panicking() {
        let mut out = Vec::new();
        let mut input = Cursor::new("0\n2\n");
        let answer = prompt_one(&mut out, &mut input, &rename_column_clarification()).unwrap();

        assert_eq!(answer, Answer::RenameNo);
    }
}
