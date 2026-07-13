use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

const START: &str = "<!-- gaman:support-matrix:start -->";
const END: &str = "<!-- gaman:support-matrix:end -->";
const DIALECTS: &[&str] = &["postgres", "sqlite", "mysql", "mariadb"];

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
    online: Vec<OnlineEvidenceRef>,
    #[serde(default)]
    offline: Vec<OfflineEvidenceRef>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OnlineEvidenceRef {
    case: String,
    checks: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OfflineEvidenceRef {
    case: String,
    assertions: Vec<String>,
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
    generation: String,
    features: BTreeMap<String, BTreeMap<String, OnlineResultCell>>,
}

#[derive(Debug, Deserialize)]
struct OnlineResultCell {
    status: OnlineResultStatus,
    #[serde(default)]
    evidence: Vec<OnlineEvidence>,
}

#[derive(Debug, Deserialize)]
struct OnlineEvidence {
    case: String,
    checks: Vec<String>,
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
    generation: String,
    features: BTreeMap<String, BTreeMap<String, OfflineResultCell>>,
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
    #[serde(default)]
    assertions: Vec<String>,
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
                update_readme_table(&input_path(&root, "GAMAN_README_PATH", "README.md"), &table)?;
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
    let online: OnlineResults = read_yaml(&input_path(
        root,
        "GAMAN_ONLINE_RESULTS",
        "results/online-results.yaml",
    ))?;
    let offline: OfflineResults = read_yaml(&input_path(
        root,
        "GAMAN_OFFLINE_RESULTS",
        "results/offline-results.yaml",
    ))?;
    if online.generation != offline.generation {
        return Err(MatrixError::Message(format!(
            "mixed evidence generations: online '{}' and offline '{}'",
            online.generation, offline.generation
        )));
    }
    validate_support_matrix(&manifest, &online, &offline)?;
    Ok(ResolvedMatrix {
        manifest,
        generation: online.generation,
    })
}

/// Resolves an optional staged input while retaining repository defaults.
fn input_path(root: &Path, variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(fallback))
}

struct ResolvedMatrix {
    manifest: SupportMatrix,
    generation: String,
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
    require_unique_descriptors(row, dialect, cell)?;
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

/// Rejects duplicate case descriptors that could inflate one support claim.
fn require_unique_descriptors(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
) -> Result<(), MatrixError> {
    let online = cell.evidence.online.iter().map(|item| item.case.as_str());
    let offline = cell.evidence.offline.iter().map(|item| item.case.as_str());
    for descriptors in [online.collect::<Vec<_>>(), offline.collect::<Vec<_>>()] {
        let mut cases = BTreeSet::new();
        for case in descriptors {
            if !cases.insert(case) {
                return Err(MatrixError::Message(format!(
                    "support row '{}'.{} repeats evidence case '{}'",
                    row.id, dialect, case
                )));
            }
        }
    }
    Ok(())
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
    for descriptor in &cell.evidence.online {
        require_online_case(row, dialect, descriptor, online)?;
    }
    for descriptor in &cell.evidence.offline {
        require_offline_case(row, dialect, descriptor, offline)?;
    }
    Ok(())
}

/// Requires exact successful online evidence with all declared checks.
fn require_online_case(
    row: &SupportRow,
    dialect: &str,
    descriptor: &OnlineEvidenceRef,
    results: &OnlineResults,
) -> Result<(), MatrixError> {
    let evidence = results
        .features
        .values()
        .filter_map(|dialects| dialects.get(dialect))
        .filter(|cell| cell.status == OnlineResultStatus::Success)
        .flat_map(|cell| &cell.evidence)
        .find(|item| item.case == descriptor.case)
        .ok_or_else(|| {
            MatrixError::Message(format!(
                "support row '{}'.{} lacks successful online case '{}'",
                row.id, dialect, descriptor.case
            ))
        })?;
    require_items(
        row,
        dialect,
        &descriptor.case,
        "check",
        &descriptor.checks,
        &evidence.checks,
    )
}

/// Requires exact successful dialect-bound offline evidence.
fn require_offline_case(
    row: &SupportRow,
    dialect: &str,
    descriptor: &OfflineEvidenceRef,
    results: &OfflineResults,
) -> Result<(), MatrixError> {
    let evidence = results
        .features
        .values()
        .filter_map(|dialects| dialects.get(dialect))
        .filter(|cell| cell.status == OfflineResultStatus::Success)
        .flat_map(|cell| &cell.evidence)
        .find(|item| item.case == descriptor.case && item.dialect.as_deref() == Some(dialect))
        .ok_or_else(|| {
            MatrixError::Message(format!(
                "support row '{}'.{} lacks successful offline case '{}'",
                row.id, dialect, descriptor.case
            ))
        })?;
    require_items(
        row,
        dialect,
        &descriptor.case,
        "assertion",
        &descriptor.assertions,
        &evidence.assertions,
    )
}

/// Verifies that a case executed every support claim declared by the matrix.
fn require_items(
    row: &SupportRow,
    dialect: &str,
    case: &str,
    label: &str,
    required: &[String],
    observed: &[String],
) -> Result<(), MatrixError> {
    if required.is_empty() {
        return Err(MatrixError::Message(format!(
            "support row '{}'.{} case '{}' declares no {label}s",
            row.id, dialect, case
        )));
    }
    if let Some(missing) = required.iter().find(|value| !observed.contains(value)) {
        return Err(MatrixError::Message(format!(
            "support row '{}'.{} case '{}' did not execute {label} '{}'",
            row.id, dialect, case, missing
        )));
    }
    Ok(())
}

fn render_support_table(matrix: &ResolvedMatrix) -> Result<String, MatrixError> {
    let mut lines = vec![
        format!("<!-- evidence-generation: {} -->", matrix.generation),
        "| Feature | PostgreSQL | SQLite | MySQL | MariaDB |".to_string(),
        "| --- | --- | --- | --- | --- |".to_string(),
    ];
    for row in &matrix.manifest.rows {
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            row.label,
            support_symbol(row, "postgres")?,
            support_symbol(row, "sqlite")?,
            support_symbol(row, "mysql")?,
            support_symbol(row, "mariadb")?
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
            ) && let Some(note) = &cell.note
            {
                grouped.entry(note.as_str()).or_default().push(*dialect);
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
        let dialects = results.features.get(&feature.id).ok_or_else(|| {
            MatrixError::Message(format!(
                "offline results are missing feature '{}'",
                feature.id
            ))
        })?;
        let selected = feature
            .dialect
            .as_deref()
            .map(|dialect| vec![dialect])
            .unwrap_or_else(|| DIALECTS.to_vec());
        for dialect in selected {
            let cell = dialects.get(dialect).ok_or_else(|| {
                MatrixError::Message(format!("offline feature '{}' lacks {dialect}", feature.id))
            })?;
            lines.push(format!(
                "| {} | {} | {} | {} | {} |",
                feature.category,
                feature.label,
                dialect,
                offline_symbol(&cell.status),
                offline_evidence(cell)
            ));
        }
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
