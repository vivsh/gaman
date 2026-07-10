use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use gaman_core::dialects::Dialect;
use gaman_core::drift::{DriftPropertyDoc, contract_for};
use gaman_core::states::types::EntityKind;
use serde::Deserialize;
use thiserror::Error;

const DIALECTS: &[&str] = &["postgres", "sqlite", "mysql"];
const DOC_PATH: &str = "docs/support-evidence.md";

#[derive(Debug, Error)]
enum EvidenceError {
    #[error("I/O error at '{path}': {message}")]
    Io { path: String, message: String },
    #[error("failed to parse '{path}': {message}")]
    Parse { path: String, message: String },
    #[error("{0}")]
    Message(String),
}

#[derive(Debug, Deserialize)]
struct ParserResults {
    cases: BTreeMap<String, ParserCaseResult>,
}

#[derive(Debug, Deserialize)]
struct ParserCaseResult {
    dialect: String,
    status: String,
    #[serde(default)]
    entities: Vec<ParserEntity>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ParserEntity {
    kind: String,
}

#[derive(Debug, Deserialize)]
struct OnlineResults {
    features: BTreeMap<String, BTreeMap<String, OnlineResultCell>>,
}

#[derive(Debug, Deserialize)]
struct OnlineResultCell {
    status: String,
    #[serde(default)]
    evidence: Vec<OnlineEvidence>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Deserialize)]
struct OnlineEvidence {
    case: String,
    description: String,
    #[serde(default)]
    checks: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OfflineResults {
    features: BTreeMap<String, OfflineResultCell>,
}

#[derive(Debug, Deserialize)]
struct OfflineResultCell {
    status: String,
    #[serde(default)]
    evidence: Vec<OfflineEvidence>,
}

#[derive(Debug, Deserialize)]
struct OfflineEvidence {
    case: String,
    description: String,
    group: String,
    kind: String,
    #[serde(default)]
    dialect: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FeatureCatalog {
    features: Vec<FeatureEntry>,
}

#[derive(Debug, Deserialize)]
struct FeatureEntry {
    id: String,
    label: String,
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
}

#[derive(Clone, Copy)]
enum Mode {
    Print,
    UpdateDoc,
    Check,
}

struct EvidenceInputs {
    parser: ParserResults,
    online: OnlineResults,
    offline: OfflineResults,
    online_labels: BTreeMap<String, String>,
    offline_labels: BTreeMap<String, String>,
    offline_categories: BTreeMap<String, String>,
    parser_paths: BTreeMap<String, PathBuf>,
    online_paths: BTreeMap<String, PathBuf>,
    offline_paths: BTreeMap<String, PathBuf>,
}

fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), EvidenceError> {
    let mode = parse_args()?;
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let inputs = read_inputs(&root)?;
    validate_inputs(&inputs)?;
    let doc = render_doc(&inputs);
    match mode {
        Mode::Print => println!("{doc}"),
        Mode::UpdateDoc => write_doc(&root, &doc)?,
        Mode::Check => check_doc(&root, &doc)?,
    }
    Ok(())
}

fn parse_args() -> Result<Mode, EvidenceError> {
    let mut mode = Mode::Print;
    for arg in std::env::args().skip(1) {
        mode = match arg.as_str() {
            "--update-doc" => Mode::UpdateDoc,
            "--check" => Mode::Check,
            _ => {
                return Err(EvidenceError::Message(format!(
                    "unsupported argument '{arg}'"
                )));
            }
        };
    }
    Ok(mode)
}

fn read_inputs(root: &Path) -> Result<EvidenceInputs, EvidenceError> {
    let parser = read_yaml(&root.join("results/parser-results.yaml"))?;
    let online = read_yaml(&root.join("results/online-results.yaml"))?;
    let offline = read_yaml(&root.join("results/offline-results.yaml"))?;
    let online_catalog: FeatureCatalog = read_yaml(&root.join("tests/cases/features.yaml"))?;
    let offline_catalog: OfflineFeatureCatalog =
        read_yaml(&root.join("tests/cases/offline-features.yaml"))?;
    Ok(EvidenceInputs {
        parser,
        online,
        offline,
        online_labels: online_labels(online_catalog),
        offline_labels: offline_labels(&offline_catalog),
        offline_categories: offline_categories(offline_catalog),
        parser_paths: case_paths(root, &root.join("tests/cases/parser"))?,
        online_paths: case_paths(root, &root.join("tests/cases/online"))?,
        offline_paths: case_paths(root, &root.join("tests/cases/offline"))?,
    })
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, EvidenceError> {
    let raw = fs::read_to_string(path).map_err(|error| EvidenceError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_yaml::from_str(&raw).map_err(|error| EvidenceError::Parse {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn online_labels(catalog: FeatureCatalog) -> BTreeMap<String, String> {
    catalog
        .features
        .into_iter()
        .map(|f| (f.id, f.label))
        .collect()
}

fn offline_labels(catalog: &OfflineFeatureCatalog) -> BTreeMap<String, String> {
    catalog
        .features
        .iter()
        .map(|f| (f.id.clone(), f.label.clone()))
        .collect()
}

fn offline_categories(catalog: OfflineFeatureCatalog) -> BTreeMap<String, String> {
    catalog
        .features
        .into_iter()
        .map(|f| (f.id, f.category))
        .collect()
}

fn case_paths(repo_root: &Path, root: &Path) -> Result<BTreeMap<String, PathBuf>, EvidenceError> {
    let mut paths = BTreeMap::new();
    collect_case_paths(repo_root, root, &mut paths)?;
    Ok(paths)
}

fn collect_case_paths(
    repo_root: &Path,
    root: &Path,
    paths: &mut BTreeMap<String, PathBuf>,
) -> Result<(), EvidenceError> {
    for entry in fs::read_dir(root).map_err(|error| EvidenceError::Io {
        path: root.display().to_string(),
        message: error.to_string(),
    })? {
        let path = entry
            .map_err(|error| EvidenceError::Message(error.to_string()))?
            .path();
        if path.is_dir() {
            collect_case_paths(repo_root, &path, paths)?;
        } else if path.extension().and_then(|v| v.to_str()) == Some("yaml") {
            insert_case_path(repo_root, paths, path)?;
        }
    }
    Ok(())
}

fn insert_case_path(
    repo_root: &Path,
    paths: &mut BTreeMap<String, PathBuf>,
    path: PathBuf,
) -> Result<(), EvidenceError> {
    let name = path
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(|| EvidenceError::Message(format!("non UTF-8 case path {}", path.display())))?;
    let relative = path
        .strip_prefix(repo_root)
        .map_err(|error| {
            EvidenceError::Message(format!(
                "case path {} is outside repo root {}: {error}",
                path.display(),
                repo_root.display()
            ))
        })?
        .to_path_buf();
    if let Some(existing) = paths.insert(name.to_string(), relative) {
        return Err(EvidenceError::Message(format!(
            "duplicate case stem '{name}' at {} and {}",
            existing.display(),
            path.display()
        )));
    }
    Ok(())
}

fn validate_inputs(inputs: &EvidenceInputs) -> Result<(), EvidenceError> {
    validate_parser_evidence(inputs)?;
    validate_online_evidence(inputs)?;
    validate_offline_evidence(inputs)?;
    Ok(())
}

fn validate_parser_evidence(inputs: &EvidenceInputs) -> Result<(), EvidenceError> {
    for case in inputs.parser.cases.keys() {
        require_case_path(case, &inputs.parser_paths, "parser")?;
    }
    Ok(())
}

fn validate_online_evidence(inputs: &EvidenceInputs) -> Result<(), EvidenceError> {
    for (feature, dialects) in &inputs.online.features {
        if !inputs.online_labels.contains_key(feature) {
            return Err(EvidenceError::Message(format!(
                "unknown online feature '{feature}'"
            )));
        }
        for evidence in dialects.values().flat_map(|cell| &cell.evidence) {
            require_case_path(&evidence.case, &inputs.online_paths, "online")?;
        }
    }
    Ok(())
}

fn validate_offline_evidence(inputs: &EvidenceInputs) -> Result<(), EvidenceError> {
    for (feature, cell) in &inputs.offline.features {
        if !inputs.offline_labels.contains_key(feature) {
            return Err(EvidenceError::Message(format!(
                "unknown offline feature '{feature}'"
            )));
        }
        for evidence in &cell.evidence {
            require_case_path(&evidence.case, &inputs.offline_paths, "offline")?;
        }
    }
    Ok(())
}

fn require_case_path(
    case: &str,
    paths: &BTreeMap<String, PathBuf>,
    kind: &str,
) -> Result<(), EvidenceError> {
    if paths.contains_key(case) {
        Ok(())
    } else {
        Err(EvidenceError::Message(format!(
            "{kind} evidence references missing case '{case}'"
        )))
    }
}

fn render_doc(inputs: &EvidenceInputs) -> String {
    let mut lines = Vec::new();
    render_header(&mut lines);
    render_parser_support(&mut lines, inputs);
    render_inspection_support(&mut lines, inputs);
    render_verify_support(&mut lines, inputs);
    render_offline_drift_support(&mut lines, inputs);
    render_boundaries(&mut lines);
    render_regeneration(&mut lines);
    lines.join("\n") + "\n"
}

fn render_header(lines: &mut Vec<String>) {
    lines.push("# Support Evidence".to_string());
    lines.push(String::new());
    lines.push("This generated document expands the condensed README support matrix with accepted fixture evidence for parser loading, live inspection, and `verify` drift detection.".to_string());
    lines.push(String::new());
    lines.push("Do not edit this file by hand. Refresh accepted harness results, then run `cargo run --bin gaman-evidence-doc -- --update-doc`.".to_string());
    lines.push(String::new());
    lines.push("Evidence sources:".to_string());
    lines.push(String::new());
    lines.push("- `results/parser-results.yaml`".to_string());
    lines.push("- `results/offline-results.yaml`".to_string());
    lines.push("- `results/online-results.yaml`".to_string());
    lines.push(String::new());
}

fn render_parser_support(lines: &mut Vec<String>, inputs: &EvidenceInputs) {
    lines.push("## Parser Support".to_string());
    lines.push(String::new());
    lines.push("Parser evidence records SQL accepted or deliberately rejected by `gaman::parsers::parse_sql(sql, dialect)`.".to_string());
    lines.push(String::new());
    for dialect in ["postgres", "sqlite"] {
        render_parser_dialect(lines, inputs, dialect);
    }
    lines.push("### MySQL / MariaDB".to_string());
    lines.push(String::new());
    lines.push("MySQL/MariaDB statement segmentation is covered in core tests. Schema lowering is explicitly unsupported in offline parser evidence until MySQL schema support is implemented.".to_string());
    lines.push(String::new());
}

fn render_parser_dialect(lines: &mut Vec<String>, inputs: &EvidenceInputs, dialect: &str) {
    lines.push(format!("### {}", dialect_label(dialect)));
    lines.push(String::new());
    lines.push("| Capability | Evidence |".to_string());
    lines.push("| --- | --- |".to_string());
    for (capability, evidence) in parser_capabilities(inputs, dialect) {
        lines.push(format!(
            "| {} | {} |",
            md(&capability),
            evidence.join("<br>")
        ));
    }
    lines.push(String::new());
}

fn parser_capabilities(inputs: &EvidenceInputs, dialect: &str) -> BTreeMap<String, Vec<String>> {
    let mut rows = BTreeMap::<String, Vec<String>>::new();
    for (case, result) in &inputs.parser.cases {
        if result.dialect != dialect {
            continue;
        }
        let capability = parser_capability(result);
        rows.entry(capability)
            .or_default()
            .push(case_link(case, &inputs.parser_paths));
    }
    rows
}

fn parser_capability(result: &ParserCaseResult) -> String {
    if !result.entities.is_empty() {
        return parser_entity_summary(&result.entities);
    }
    if result.status == "success" {
        "expected rejection / unsupported boundary".to_string()
    } else {
        result
            .reason
            .clone()
            .unwrap_or_else(|| "parser failure".to_string())
    }
}

fn parser_entity_summary(entities: &[ParserEntity]) -> String {
    let kinds = entities
        .iter()
        .map(|entity| entity.kind.as_str())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>()
        .join(", ");
    format!("supported lowering: {kinds}")
}

fn render_inspection_support(lines: &mut Vec<String>, inputs: &EvidenceInputs) {
    lines.push("## Inspection Support".to_string());
    lines.push(String::new());
    lines.push("Inspection evidence lists online cases whose checks include `inspect`, filtered to features that describe reflected schema state rather than unrelated lifecycle mechanics.".to_string());
    lines.push(String::new());
    render_online_feature_table(lines, inputs, "inspect", INSPECTION_FEATURES);
    render_online_feature_details(lines, inputs, "inspect", INSPECTION_FEATURES);
}

fn render_verify_support(lines: &mut Vec<String>, inputs: &EvidenceInputs) {
    lines.push("## verify Drift Support".to_string());
    lines.push(String::new());
    lines.push("`verify` evidence combines the core dialect drift contract with online cases whose checks include `verify`.".to_string());
    lines.push(String::new());
    render_drift_contract(lines);
    render_online_feature_table(lines, inputs, "verify", VERIFY_FEATURES);
    render_online_feature_details(lines, inputs, "verify", VERIFY_FEATURES);
}

fn render_drift_contract(lines: &mut Vec<String>) {
    lines.push("### Drift Property Contract".to_string());
    lines.push(String::new());
    for dialect in DIALECTS {
        lines.push(format!("#### {}", dialect_label(dialect)));
        lines.push(String::new());
        let docs = drift_docs_for(dialect);
        if docs.is_empty() {
            lines.push("No drift properties are registered.".to_string());
            lines.push(String::new());
            continue;
        }
        lines.push("| Entity | Property | Compared | Ignored |".to_string());
        lines.push("| --- | --- | --- | --- |".to_string());
        for doc in docs {
            lines.push(format!(
                "| {} | `{}` | {} | {} |",
                entity_kind_name(doc.entity_kind),
                doc.property,
                md(doc.compared),
                md(doc.ignored)
            ));
        }
        lines.push(String::new());
    }
}

fn drift_docs_for(dialect: &str) -> &'static [DriftPropertyDoc] {
    match dialect {
        "postgres" => contract_for(Dialect::Postgres),
        "sqlite" => contract_for(Dialect::Sqlite),
        "mysql" => contract_for(Dialect::Mysql),
        _ => &[],
    }
}

fn render_online_feature_table(
    lines: &mut Vec<String>,
    inputs: &EvidenceInputs,
    check: &str,
    allowed: &[&str],
) {
    lines.push("### Evidence Summary".to_string());
    lines.push(String::new());
    lines.push("| Feature | PostgreSQL | SQLite | MySQL |".to_string());
    lines.push("| --- | --- | --- | --- |".to_string());
    for feature in allowed {
        if !feature_has_check(inputs, feature, check) {
            continue;
        }
        let label = online_label(inputs, feature);
        lines.push(format!(
            "| {} (`{}`) | {} | {} | {} |",
            md(&label),
            feature,
            status_cell(inputs, feature, "postgres"),
            status_cell(inputs, feature, "sqlite"),
            status_cell(inputs, feature, "mysql")
        ));
    }
    lines.push(String::new());
}

fn render_online_feature_details(
    lines: &mut Vec<String>,
    inputs: &EvidenceInputs,
    check: &str,
    allowed: &[&str],
) {
    lines.push("### Fixture Evidence".to_string());
    lines.push(String::new());
    for feature in allowed {
        if !feature_has_check(inputs, feature, check) {
            continue;
        }
        render_online_feature(lines, inputs, feature, check);
    }
}

fn render_online_feature(
    lines: &mut Vec<String>,
    inputs: &EvidenceInputs,
    feature: &str,
    check: &str,
) {
    lines.push(format!(
        "#### {} (`{}`)",
        online_label(inputs, feature),
        feature
    ));
    lines.push(String::new());
    for dialect in DIALECTS {
        let Some(cell) = inputs
            .online
            .features
            .get(feature)
            .and_then(|d| d.get(*dialect))
        else {
            continue;
        };
        let evidence = check_evidence(cell, check);
        if evidence.is_empty() {
            continue;
        }
        lines.push(format!("##### {}", dialect_label(dialect)));
        lines.push(String::new());
        for item in evidence {
            lines.push(format!(
                "- {}: {} (`{}`)",
                case_link(&item.case, &inputs.online_paths),
                md(&item.description),
                item.checks.join(", ")
            ));
        }
        lines.push(String::new());
    }
}

fn feature_has_check(inputs: &EvidenceInputs, feature: &str, check: &str) -> bool {
    inputs.online.features.get(feature).is_some_and(|dialects| {
        dialects
            .values()
            .any(|cell| !check_evidence(cell, check).is_empty())
    })
}

fn check_evidence<'a>(cell: &'a OnlineResultCell, check: &str) -> Vec<&'a OnlineEvidence> {
    if cell.status != "success" {
        return Vec::new();
    }
    cell.evidence
        .iter()
        .filter(|evidence| evidence.checks.iter().any(|value| value == check))
        .collect()
}

fn render_offline_drift_support(lines: &mut Vec<String>, inputs: &EvidenceInputs) {
    lines.push("## Offline Drift Comparator Evidence".to_string());
    lines.push(String::new());
    lines.push(
        "Offline drift evidence exercises `gaman_core::drift` directly without live catalog setup."
            .to_string(),
    );
    lines.push(String::new());
    lines.push("| Feature | Status | Evidence |".to_string());
    lines.push("| --- | --- | --- |".to_string());
    for (feature, cell) in offline_verify_rows(inputs) {
        let evidence = cell
            .evidence
            .iter()
            .map(|item| offline_evidence_link(item, inputs))
            .collect::<Vec<_>>()
            .join("<br>");
        lines.push(format!(
            "| {} (`{}`) | {} | {} |",
            md(&offline_label(inputs, feature)),
            feature,
            md(&cell.status),
            evidence
        ));
    }
    lines.push(String::new());
}

fn offline_verify_rows(inputs: &EvidenceInputs) -> Vec<(&str, &OfflineResultCell)> {
    inputs
        .offline
        .features
        .iter()
        .filter(|(feature, _)| {
            inputs.offline_categories.get(*feature) == Some(&"verify".to_string())
        })
        .map(|(feature, cell)| (feature.as_str(), cell))
        .collect()
}

fn offline_evidence_link(item: &OfflineEvidence, inputs: &EvidenceInputs) -> String {
    let dialect = item.dialect.as_deref().unwrap_or("all");
    format!(
        "{}: {} (`{}/{}/{}`)",
        case_link(&item.case, &inputs.offline_paths),
        md(&item.description),
        item.group,
        dialect,
        item.kind
    )
}

fn render_boundaries(lines: &mut Vec<String>) {
    lines.push("## Bounded Or Unsupported Evidence".to_string());
    lines.push(String::new());
    lines.push("- PostgreSQL parser rejects non-`CREATE` schema-loading statements, event triggers, policies, grants, and unsupported materialized view tail syntax.".to_string());
    lines.push("- SQLite parser rejects non-`CREATE` schema-loading statements and reports functions, enums, and extensions as unsupported schema constructs.".to_string());
    lines.push("- MySQL/MariaDB has SQL segmentation and dialect selection support, but schema lowering, SQL rendering, live inspection, and `verify` are not implemented in accepted evidence.".to_string());
    lines.push("- `verify` is property-based: unregistered properties and opaque body/source-only differences are ignored unless promoted into the drift contract.".to_string());
    lines.push(String::new());
}

fn render_regeneration(lines: &mut Vec<String>) {
    lines.push("## Regenerating This Page".to_string());
    lines.push(String::new());
    lines.push("Refresh accepted evidence, then regenerate and check this document:".to_string());
    lines.push(String::new());
    lines.push("```bash".to_string());
    lines.push(
        "cargo test -p gaman --test parser -- --record results/parser-results.yaml".to_string(),
    );
    lines.push("cargo test -p gaman --features sqlite --test offline -- --record results/offline-results.yaml".to_string());
    lines.push("set -a; source .env; set +a; cargo test -p gaman --features sqlite --test online -- --record results/online-results.yaml".to_string());
    lines.push("cargo run --bin gaman-evidence-doc -- --update-doc".to_string());
    lines.push("cargo run --bin gaman-evidence-doc -- --check".to_string());
    lines.push("```".to_string());
}

fn status_cell(inputs: &EvidenceInputs, feature: &str, dialect: &str) -> String {
    inputs
        .online
        .features
        .get(feature)
        .and_then(|dialects| dialects.get(dialect))
        .map(status_text)
        .unwrap_or_else(|| "not recorded".to_string())
}

fn status_text(cell: &OnlineResultCell) -> String {
    if let Some(reason) = &cell.reason
        && !reason.trim().is_empty()
    {
        return format!("{} ({})", cell.status, md(reason));
    }
    cell.status.clone()
}

fn case_link(case: &str, paths: &BTreeMap<String, PathBuf>) -> String {
    match paths.get(case) {
        Some(path) => format!("[`{}`]({})", md(case), path.display()),
        None => format!("`{}`", md(case)),
    }
}

fn online_label(inputs: &EvidenceInputs, feature: &str) -> String {
    inputs
        .online_labels
        .get(feature)
        .cloned()
        .unwrap_or_else(|| feature.to_string())
}

fn offline_label(inputs: &EvidenceInputs, feature: &str) -> String {
    inputs
        .offline_labels
        .get(feature)
        .cloned()
        .unwrap_or_else(|| feature.to_string())
}

fn entity_kind_name(kind: EntityKind) -> &'static str {
    match kind {
        EntityKind::Table => "table",
        EntityKind::Column => "column",
        EntityKind::Constraint => "constraint",
        EntityKind::ForeignKey => "foreign key",
        EntityKind::Index => "index",
        EntityKind::Trigger => "trigger",
        EntityKind::Function => "function",
        EntityKind::View => "view",
        EntityKind::Enum => "enum",
        EntityKind::Extension => "extension",
    }
}

fn dialect_label(dialect: &str) -> &'static str {
    match dialect {
        "postgres" => "PostgreSQL",
        "sqlite" => "SQLite",
        "mysql" => "MySQL / MariaDB",
        _ => "unknown",
    }
}

fn md(value: &str) -> String {
    value.replace('|', "\\|").replace('\n', " ")
}

fn write_doc(root: &Path, doc: &str) -> Result<(), EvidenceError> {
    let path = root.join(DOC_PATH);
    fs::write(&path, doc).map_err(|error| EvidenceError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })
}

fn check_doc(root: &Path, expected: &str) -> Result<(), EvidenceError> {
    let path = root.join(DOC_PATH);
    let actual = fs::read_to_string(&path).map_err(|error| EvidenceError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    if actual == expected {
        Ok(())
    } else {
        Err(EvidenceError::Message(format!(
            "{DOC_PATH} is stale; run `cargo run --bin gaman-evidence-doc -- --update-doc`"
        )))
    }
}

const INSPECTION_FEATURES: &[&str] = &[
    "live_inspect_verify",
    "tables_columns",
    "table_create_drop_rename",
    "column_add_drop_rename",
    "type_null_default_changes",
    "generated_columns",
    "single_primary_keys",
    "composite_primary_keys",
    "single_foreign_keys",
    "composite_foreign_keys",
    "indexes",
    "unique_constraints",
    "check_constraints",
    "indexes_constraints",
    "partial_indexes",
    "opaque_index_metadata",
    "schemas_namespaces",
    "extensions",
    "enums",
    "functions_opaque",
    "trigger_query_objects",
    "views_opaque",
    "unsupported_feature_errors",
];

const VERIFY_FEATURES: &[&str] = &[
    "live_inspect_verify",
    "ownership_scoped_verify",
    "tables_columns",
    "type_null_default_changes",
    "generated_columns",
    "single_primary_keys",
    "composite_primary_keys",
    "single_foreign_keys",
    "composite_foreign_keys",
    "indexes",
    "unique_constraints",
    "check_constraints",
    "indexes_constraints",
    "opaque_index_metadata",
    "schemas_namespaces",
    "extensions",
    "enums",
    "functions_opaque",
    "trigger_query_objects",
    "views_opaque",
    "unsupported_feature_errors",
];
