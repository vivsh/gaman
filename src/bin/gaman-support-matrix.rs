use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

const START: &str = "<!-- gaman:support-matrix:start -->";
const END: &str = "<!-- gaman:support-matrix:end -->";
const DIALECTS: &[&str] = &["postgres", "sqlite", "mysql"];

#[derive(Debug, Error)]
enum MatrixError {
    #[error("I/O error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Deserialize)]
struct SupportMatrix {
    rows: Vec<SupportRow>,
}

#[derive(Debug, Deserialize)]
struct SupportRow {
    id: String,
    label: String,
    dialects: BTreeMap<String, SupportCell>,
}

#[derive(Debug, Deserialize)]
struct SupportCell {
    status: SupportStatus,
    #[serde(default)]
    evidence: EvidenceRefs,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct EvidenceRefs {
    #[serde(default)]
    online: Vec<String>,
    #[serde(default)]
    offline: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SupportStatus {
    Supported,
    Partial,
    Planned,
    Unsupported,
}

#[derive(Debug, Deserialize)]
struct OnlineResults {
    features: BTreeMap<String, BTreeMap<String, OnlineResultCell>>,
}

#[derive(Debug, Deserialize)]
struct OnlineResultCell {
    status: OnlineResultStatus,
}

#[derive(Debug, Deserialize)]
struct OfflineFeatureCatalog {
    features: Vec<OfflineFeatureEntry>,
}

#[derive(Debug, Deserialize)]
struct OfflineFeatureEntry {
    id: String,
    label: String,
    category: String,
    #[serde(default)]
    dialect: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OfflineResults {
    features: BTreeMap<String, OfflineResultCell>,
}

#[derive(Debug, Deserialize)]
struct OfflineResultCell {
    status: OfflineResultStatus,
    #[serde(default)]
    evidence: Vec<OfflineEvidence>,
}

#[derive(Debug, Deserialize)]
struct OfflineEvidence {
    case: String,
    group: String,
    kind: String,
    #[serde(default)]
    dialect: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OnlineResultStatus {
    Success,
    Failure,
    Unimplemented,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineResultStatus {
    Success,
    Failure,
    Skipped,
}

enum Mode {
    Support { update_readme: bool },
    Offline,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), MatrixError> {
    let mode = parse_args()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match mode {
        Mode::Support { update_readme } => {
            let matrix = read_support_inputs(&root)?;
            let table = render_support_table(&matrix)?;
            if update_readme {
                update_readme_table(&root.join("README.md"), &table)?;
            } else {
                println!("{table}");
            }
        }
        Mode::Offline => {
            let catalog: OfflineFeatureCatalog =
                read_yaml(&root.join("tests/cases/offline-features.yaml"))?;
            let results: OfflineResults = read_yaml(&root.join("results/offline-results.yaml"))?;
            println!("{}", render_offline_table(&catalog, &results)?);
        }
    }
    Ok(())
}

fn parse_args() -> Result<Mode, MatrixError> {
    let mut update_readme = false;
    let mut offline = false;
    for arg in std::env::args().skip(1) {
        match arg.as_str() {
            "--update-readme" => update_readme = true,
            "--offline" => offline = true,
            _ => {
                return Err(MatrixError::Message(format!(
                    "unsupported argument '{arg}'"
                )));
            }
        }
    }
    if offline && update_readme {
        return Err(MatrixError::Message(
            "--offline cannot be combined with --update-readme".into(),
        ));
    }
    Ok(if offline {
        Mode::Offline
    } else {
        Mode::Support { update_readme }
    })
}

fn read_support_inputs(root: &Path) -> Result<ResolvedMatrix, MatrixError> {
    let manifest: SupportMatrix = read_yaml(&root.join("tests/cases/support-matrix.yaml"))?;
    let online: OnlineResults = read_yaml(&root.join("results/online-results.yaml"))?;
    let offline: OfflineResults = read_yaml(&root.join("results/offline-results.yaml"))?;
    validate_support_matrix(&manifest, &online, &offline)?;
    Ok(ResolvedMatrix { manifest })
}

struct ResolvedMatrix {
    manifest: SupportMatrix,
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, MatrixError> {
    let raw = fs::read_to_string(path).map_err(|error| MatrixError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_yaml::from_str(&raw).map_err(|error| MatrixError::Parse {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn validate_support_matrix(
    manifest: &SupportMatrix,
    online: &OnlineResults,
    offline: &OfflineResults,
) -> Result<(), MatrixError> {
    for row in &manifest.rows {
        for dialect in DIALECTS {
            let cell = row.dialects.get(*dialect).ok_or_else(|| {
                MatrixError::Message(format!(
                    "support row '{}' is missing dialect '{}'",
                    row.id, dialect
                ))
            })?;
            validate_support_cell(row, dialect, cell, online, offline)?;
        }
    }
    Ok(())
}

fn validate_support_cell(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
    online: &OnlineResults,
    offline: &OfflineResults,
) -> Result<(), MatrixError> {
    match cell.status {
        SupportStatus::Supported => {
            require_evidence(row, dialect, cell)?;
            require_successful_evidence(row, dialect, cell, online, offline)
        }
        SupportStatus::Partial => {
            require_note(row, dialect, cell)?;
            require_evidence(row, dialect, cell)?;
            require_successful_evidence(row, dialect, cell, online, offline)
        }
        SupportStatus::Unsupported => {
            require_note(row, dialect, cell)?;
            require_successful_evidence(row, dialect, cell, online, offline)
        }
        SupportStatus::Planned => {
            require_note(row, dialect, cell)?;
            Ok(())
        }
    }
}

fn require_note(row: &SupportRow, dialect: &str, cell: &SupportCell) -> Result<(), MatrixError> {
    if cell
        .note
        .as_ref()
        .is_some_and(|note| !note.trim().is_empty())
    {
        return Ok(());
    }
    Err(MatrixError::Message(format!(
        "support row '{}'.{} needs a note for {:?}",
        row.id, dialect, cell.status
    )))
}

fn require_evidence(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
) -> Result<(), MatrixError> {
    if !cell.evidence.online.is_empty() || !cell.evidence.offline.is_empty() {
        return Ok(());
    }
    Err(MatrixError::Message(format!(
        "support row '{}'.{} needs accepted evidence",
        row.id, dialect
    )))
}

fn require_successful_evidence(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
    online: &OnlineResults,
    offline: &OfflineResults,
) -> Result<(), MatrixError> {
    for feature in &cell.evidence.online {
        let result = online_result(online, feature, dialect)?;
        if result.status != OnlineResultStatus::Success {
            return Err(MatrixError::Message(format!(
                "support row '{}'.{} needs online feature '{}' to be success, got {:?}",
                row.id, dialect, feature, result.status
            )));
        }
    }
    for feature in &cell.evidence.offline {
        let result = offline.features.get(feature).ok_or_else(|| {
            MatrixError::Message(format!(
                "support row '{}'.{} references unknown offline feature '{}'",
                row.id, dialect, feature
            ))
        })?;
        if result.status != OfflineResultStatus::Success {
            return Err(MatrixError::Message(format!(
                "support row '{}'.{} needs offline feature '{}' to be success, got {:?}",
                row.id, dialect, feature, result.status
            )));
        }
    }
    Ok(())
}

fn online_result<'a>(
    results: &'a OnlineResults,
    feature: &str,
    dialect: &str,
) -> Result<&'a OnlineResultCell, MatrixError> {
    let dialects = results.features.get(feature).ok_or_else(|| {
        MatrixError::Message(format!(
            "support matrix references unknown online feature '{feature}'"
        ))
    })?;
    dialects
        .get(dialect)
        .ok_or_else(|| MatrixError::Message(format!("online feature '{feature}' lacks {dialect}")))
}

fn render_support_table(matrix: &ResolvedMatrix) -> Result<String, MatrixError> {
    let mut lines = vec![
        "| Feature | PostgreSQL | SQLite | MySQL / MariaDB |".to_string(),
        "| --- | --- | --- | --- |".to_string(),
    ];
    for row in &matrix.manifest.rows {
        lines.push(format!(
            "| {} | {} | {} | {} |",
            row.label,
            support_symbol(row, "postgres")?,
            support_symbol(row, "sqlite")?,
            support_symbol(row, "mysql")?
        ));
    }
    append_notes(&mut lines, &matrix.manifest);
    Ok(lines.join("\n"))
}

fn support_symbol(row: &SupportRow, dialect: &str) -> Result<&'static str, MatrixError> {
    let cell = row
        .dialects
        .get(dialect)
        .ok_or_else(|| MatrixError::Message(format!("support row '{}' lacks {dialect}", row.id)))?;
    Ok(match cell.status {
        SupportStatus::Supported => "✅",
        SupportStatus::Partial => "◐",
        SupportStatus::Planned => "🚧",
        SupportStatus::Unsupported => "❌",
    })
}

fn append_notes(lines: &mut Vec<String>, manifest: &SupportMatrix) {
    let notes = support_notes(manifest);
    if notes.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Notes:".to_string());
    for note in notes {
        lines.push(format!("- {note}"));
    }
}

fn support_notes(manifest: &SupportMatrix) -> Vec<String> {
    let mut notes = Vec::new();
    for row in &manifest.rows {
        let mut grouped = BTreeMap::<&str, Vec<&str>>::new();
        for dialect in DIALECTS {
            let Some(cell) = row.dialects.get(*dialect) else {
                continue;
            };
            if matches!(
                cell.status,
                SupportStatus::Partial | SupportStatus::Unsupported
            ) {
                if let Some(note) = &cell.note {
                    grouped.entry(note.as_str()).or_default().push(*dialect);
                }
            }
        }
        for (note, dialects) in grouped {
            notes.push(format!("{} ({}): {note}", row.label, dialects.join("/")));
        }
    }
    notes
}

fn render_offline_table(
    catalog: &OfflineFeatureCatalog,
    results: &OfflineResults,
) -> Result<String, MatrixError> {
    let mut lines = vec![
        "| Category | Feature | Dialect | Status | Evidence |".to_string(),
        "| --- | --- | --- | --- | --- |".to_string(),
    ];
    for feature in &catalog.features {
        let cell = results.features.get(&feature.id).ok_or_else(|| {
            MatrixError::Message(format!(
                "offline results are missing feature '{}'",
                feature.id
            ))
        })?;
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            feature.category,
            feature.label,
            feature.dialect.as_deref().unwrap_or("all"),
            offline_symbol(&cell.status),
            offline_evidence(cell)
        ));
    }
    Ok(lines.join("\n"))
}

fn offline_symbol(status: &OfflineResultStatus) -> &'static str {
    match status {
        OfflineResultStatus::Success => "✅",
        OfflineResultStatus::Failure => "❌",
        OfflineResultStatus::Skipped => "⏭",
    }
}

fn offline_evidence(cell: &OfflineResultCell) -> String {
    if cell.evidence.is_empty() {
        return String::new();
    }
    cell.evidence
        .iter()
        .take(3)
        .map(|evidence| {
            let dialect = evidence
                .dialect
                .as_deref()
                .map(|value| format!("/{value}"))
                .unwrap_or_default();
            format!(
                "{} [{}{}:{}]",
                evidence.case, evidence.group, dialect, evidence.kind
            )
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn update_readme_table(readme_path: &Path, table: &str) -> Result<(), MatrixError> {
    let readme = fs::read_to_string(readme_path).map_err(|error| MatrixError::Io {
        path: readme_path.display().to_string(),
        message: error.to_string(),
    })?;
    let start = readme.find(START).ok_or_else(|| {
        MatrixError::Message("README support-matrix start marker is missing".into())
    })?;
    let after_start = start + START.len();
    let end = readme[after_start..]
        .find(END)
        .map(|relative| after_start + relative + END.len())
        .ok_or_else(|| {
            MatrixError::Message("README support-matrix end marker is missing".into())
        })?;
    let replacement = format!("{START}\n{table}\n{END}");
    let updated = format!("{}{}{}", &readme[..start], replacement, &readme[end..]);
    fs::write(readme_path, updated).map_err(|error| MatrixError::Io {
        path: readme_path.display().to_string(),
        message: error.to_string(),
    })
}
