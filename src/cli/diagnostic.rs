//! Concise terminal presentation for configuration and shared runner failures.

use std::error::Error;
use std::fmt;

use crate::conf::ConfigError;
use gaman_core::clarifier::{Clarification, clarification_message};
use gaman_core::command_args::ArgumentDiagnostic;
use gaman_core::states::SchemaLoadError;

/// A concise CLI diagnostic with optional details and actionable hints.
#[derive(Debug, Clone)]
pub struct CliDiagnostic {
    summary: String,
    details: Vec<String>,
    hints: Vec<String>,
    verbose_causes: Vec<String>,
}

impl CliDiagnostic {
    /// Creates one user-facing terminal diagnostic.
    pub(crate) fn new(summary: impl Into<String>) -> Self {
        Self {
            summary: summary.into(),
            details: Vec::new(),
            hints: Vec::new(),
            verbose_causes: Vec::new(),
        }
    }

    /// Adds one compact detail line.
    pub(crate) fn detail(mut self, detail: impl Into<String>) -> Self {
        self.details.push(detail.into());
        self
    }

    /// Adds one actionable hint line.
    pub(crate) fn hint(mut self, hint: impl Into<String>) -> Self {
        self.hints.push(hint.into());
        self
    }

    /// Adds sanitized internal causes that are printed only when verbose diagnostics are enabled.
    pub(crate) fn verbose_causes(mut self, causes: Vec<String>) -> Self {
        self.verbose_causes = causes;
        self
    }
}

/// Error returned by native argument resolution, presentation, or runner execution.
#[derive(Debug)]
pub enum CommandError {
    /// One fully rendered terminal diagnostic.
    Diagnostic(CliDiagnostic),
}

impl CommandError {
    /// Creates one concise diagnostic without exposing internal error chains.
    pub(crate) fn diagnostic(summary: impl Into<String>) -> Self {
        Self::Diagnostic(CliDiagnostic::new(summary))
    }

    /// Adds one detail line to a newly constructed CLI diagnostic.
    pub(crate) fn detail(self, detail: impl Into<String>) -> Self {
        match self {
            Self::Diagnostic(diagnostic) => Self::Diagnostic(diagnostic.detail(detail)),
        }
    }

    /// Adds one hint line to a newly constructed CLI diagnostic.
    pub(crate) fn hint(self, hint: impl Into<String>) -> Self {
        match self {
            Self::Diagnostic(diagnostic) => Self::Diagnostic(diagnostic.hint(hint)),
        }
    }

    /// Converts shared argh parser or semantic validation output without rewriting it.
    pub(crate) fn from_argument_diagnostic(error: ArgumentDiagnostic) -> Self {
        Self::diagnostic(error.output)
    }

    /// Converts one resolved configuration failure into an actionable diagnostic.
    pub(crate) fn from_config_error(error: ConfigError) -> Self {
        Self::Diagnostic(
            CliDiagnostic::new(error.to_string()).hint(
                "check command options, environment variables, and selected database dialect",
            ),
        )
    }

    /// Converts authored-schema loading failures without exposing implementation details.
    pub(crate) fn from_schema_load(error: SchemaLoadError) -> Self {
        Self::Diagnostic(
            CliDiagnostic::new(error.to_string())
                .hint("correct the schema input and retry the command"),
        )
    }

    /// Converts one core runner failure while preserving its shared diagnostic wording.
    pub(crate) fn from_runner(error: gaman_core::CommandError) -> Self {
        match error {
            gaman_core::CommandError::NeedsInput(clarifications) => {
                Self::clarifications_disabled("make", &clarifications)
            }
            error => {
                let causes = error.verbose_causes();
                let diagnostic = error.diagnostic();
                let mut rendered = CliDiagnostic::new(diagnostic.summary);
                for detail in diagnostic.details {
                    rendered = rendered.detail(detail);
                }
                if let Some(hint) = diagnostic.hint {
                    rendered = rendered.hint(hint);
                }
                Self::Diagnostic(rendered.verbose_causes(causes))
            }
        }
    }

    /// Renders required clarifications when the host is configured not to prompt.
    pub(crate) fn clarifications_disabled(mode: &str, clarifications: &[Clarification]) -> Self {
        let mut detail = format!(
            "{mode} requires {} clarification(s), but prompts are disabled",
            clarifications.len()
        );
        for clarification in clarifications {
            let message = clarification_message(clarification);
            detail.push_str(&format!(
                "\n  - {}: {}",
                clarification.id, message.description
            ));
        }
        Self::Diagnostic(
            CliDiagnostic::new("clarification needed")
                .detail(detail)
                .hint("run the command interactively or provide clarification decisions"),
        )
    }

    /// Prints normal actionable diagnostics and optional sanitized causes for debugging.
    pub fn print(&self, verbose: bool) {
        let Self::Diagnostic(diagnostic) = self;
        eprintln!("error: {}", diagnostic.summary);
        for detail in &diagnostic.details {
            eprintln!("  {detail}");
        }
        for hint in &diagnostic.hints {
            eprintln!("  hint: {hint}");
        }
        if verbose && !diagnostic.verbose_causes.is_empty() {
            eprintln!("  causes:");
            for cause in &diagnostic.verbose_causes {
                eprintln!("    - {cause}");
            }
        }
    }
}

impl fmt::Display for CommandError {
    /// Formats the same concise diagnostic used by terminal output.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self::Diagnostic(diagnostic) = self;
        formatter.write_str(&diagnostic.summary)
    }
}

impl Error for CommandError {}
