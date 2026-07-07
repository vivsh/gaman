use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

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
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OfflineResultStatus {
    Success,
    Failure,
    Skipped,
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
struct OfflineCaseSummary {
    description: String,
    group: String,
    features: Vec<String>,
    kind: String,
    #[serde(default)]
    dialect: Option<String>,
    #[serde(default)]
    parser_dialect: Option<String>,
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
struct OnlineResults {
    features: BTreeMap<String, BTreeMap<String, OnlineResultCell>>,
}

#[derive(Debug, Deserialize)]
struct OnlineResultCell {
    status: OnlineResultStatus,
    #[serde(default)]
    evidence: Vec<OnlineEvidence>,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
enum OnlineResultStatus {
    Success,
    Failure,
    Unimplemented,
}

#[derive(Debug, Deserialize)]
struct OnlineEvidence {
    case: String,
    description: String,
    #[serde(default)]
    checks: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OnlineCaseSummary {
    description: String,
    features: Vec<String>,
    dialects: BTreeMap<String, serde_yaml::Value>,
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
    evidence: SupportEvidenceRefs,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
struct SupportEvidenceRefs {
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

/// Verifies accepted offline evidence comes from real fixtures and covers modeled behavior.
#[test]
fn offline_evidence_matrix_is_complete_and_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog_path = root.join("tests/cases/offline-features.yaml");
    let results_path = root.join("results/offline-results.yaml");
    let catalog: OfflineFeatureCatalog = read_yaml(&catalog_path);
    let results: OfflineResults = read_yaml(&results_path);
    let cases = read_offline_cases(root);

    assert_offline_catalog_shape(&catalog);
    assert_offline_case_metadata(&catalog, &cases);
    assert_offline_results_shape(&catalog, &results);
    assert_offline_results_evidence(&results, &cases);
    assert_offline_modeled_feature_coverage(&results);
}

/// Verifies the product feature support matrix and README support table stay in sync.
#[test]
fn support_matrix_is_complete_and_readme_is_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog_path = root.join("tests/cases/features.yaml");
    let support_path = root.join("tests/cases/support-matrix.yaml");
    let results_path = root.join("results/online-results.yaml");
    let offline_results_path = root.join("results/offline-results.yaml");
    let catalog: FeatureCatalog = read_yaml(&catalog_path);
    let support: SupportMatrix = read_yaml(&support_path);
    let results: OnlineResults = read_yaml(&results_path);
    let offline_results: OfflineResults = read_yaml(&offline_results_path);
    let cases = read_online_cases(root);

    assert_feature_catalog_shape(&catalog);
    assert_online_case_metadata(&catalog, &cases);
    assert_online_results_shape(&catalog, &results);
    assert_online_results_evidence(&results, &cases);
    assert_support_matrix_shape(&support, &results, &offline_results);
    assert_readme_support_table(root, &support, &results, &offline_results);
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_yaml::from_str(&raw)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

fn read_offline_cases(root: &Path) -> BTreeMap<String, OfflineCaseSummary> {
    let root = root.join("tests/cases/offline");
    let mut cases = BTreeMap::new();
    for path in discover_yaml_files(&root) {
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let case: OfflineCaseSummary = serde_yaml::from_str(&raw)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("offline case file names must be UTF-8")
            .to_string();
        assert!(
            cases.insert(name.clone(), case).is_none(),
            "offline case file stem '{name}' is duplicated; keep offline case stems globally unique"
        );
    }
    cases
}

fn read_online_cases(root: &Path) -> BTreeMap<String, OnlineCaseSummary> {
    let root = root.join("tests/cases/online");
    let mut cases = BTreeMap::new();
    for path in discover_yaml_files(&root) {
        let raw = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let case: OnlineCaseSummary = serde_yaml::from_str(&raw)
            .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()));
        let name = path
            .file_stem()
            .and_then(|value| value.to_str())
            .expect("online case file names must be UTF-8")
            .to_string();
        cases.insert(name, case);
    }
    cases
}

fn discover_yaml_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    discover_yaml_files_into(root, &mut files);
    files.sort();
    files
}

fn discover_yaml_files_into(root: &Path, files: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(root)
        .unwrap_or_else(|error| panic!("failed to read case root {}: {error}", root.display()))
    {
        let entry = entry.unwrap_or_else(|error| panic!("failed to read case entry: {error}"));
        let path = entry.path();
        if path.is_dir() {
            discover_yaml_files_into(&path, files);
        } else if path.extension().and_then(|value| value.to_str()) == Some("yaml") {
            files.push(path);
        }
    }
}

fn assert_offline_catalog_shape(catalog: &OfflineFeatureCatalog) {
    let mut ids = BTreeSet::new();
    for feature in &catalog.features {
        assert!(
            ids.insert(feature.id.as_str()),
            "duplicate offline feature id '{}'",
            feature.id
        );
        assert!(
            !feature.label.trim().is_empty(),
            "offline feature '{}' must have a label",
            feature.id
        );
        assert!(
            !feature.category.trim().is_empty(),
            "offline feature '{}' must have a category",
            feature.id
        );
        if let Some(dialect) = &feature.dialect {
            assert!(
                SUPPORT_DIALECTS.contains(&dialect.as_str()),
                "offline feature '{}' uses unsupported dialect '{}'",
                feature.id,
                dialect
            );
        }
    }
}

fn assert_offline_case_metadata(
    catalog: &OfflineFeatureCatalog,
    cases: &BTreeMap<String, OfflineCaseSummary>,
) {
    let feature_ids = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut descriptions = BTreeSet::new();
    for (name, case) in cases {
        assert!(
            descriptions.insert(case.description.as_str()),
            "offline case '{name}' has duplicate description '{}'",
            case.description
        );
        assert!(
            !case.group.trim().is_empty(),
            "offline case '{name}' must list a group"
        );
        assert!(
            !case.features.is_empty(),
            "offline case '{name}' must list at least one feature"
        );
        for feature in &case.features {
            assert!(
                feature_ids.contains(feature.as_str()),
                "offline case '{name}' references unknown feature '{feature}'"
            );
        }
    }
}

fn assert_offline_results_shape(catalog: &OfflineFeatureCatalog, results: &OfflineResults) {
    let expected = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = results
        .features
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "offline result feature rows drifted");

    for (feature, cell) in &results.features {
        assert_eq!(
            cell.status,
            OfflineResultStatus::Success,
            "accepted offline result '{feature}' must be successful; regenerate with a full sqlite-enabled run"
        );
        assert!(
            !cell.evidence.is_empty(),
            "accepted offline result '{feature}' must include evidence"
        );
        if let Some(reason) = &cell.reason {
            assert!(
                reason.trim().is_empty(),
                "successful accepted offline result '{feature}' should not keep reason '{reason}'"
            );
        }
    }
}

fn assert_offline_results_evidence(
    results: &OfflineResults,
    cases: &BTreeMap<String, OfflineCaseSummary>,
) {
    for (feature, cell) in &results.features {
        for evidence in &cell.evidence {
            let case = cases.get(&evidence.case).unwrap_or_else(|| {
                panic!(
                    "offline result {feature} references missing case '{}'",
                    evidence.case
                )
            });
            assert!(
                case.features.iter().any(|value| value == feature),
                "offline result {feature} references case '{}' that does not list it",
                evidence.case
            );
            assert_eq!(
                evidence.description, case.description,
                "offline result {feature} has stale description for case '{}'",
                evidence.case
            );
            assert_eq!(
                evidence.group, case.group,
                "offline result {feature} has stale group for case '{}'",
                evidence.case
            );
            assert_eq!(
                evidence.kind, case.kind,
                "offline result {feature} has stale kind for case '{}'",
                evidence.case
            );
            assert_eq!(
                evidence.dialect,
                offline_case_dialect(case),
                "offline result {feature} has stale dialect for case '{}'",
                evidence.case
            );
        }
    }
}

fn offline_case_dialect(case: &OfflineCaseSummary) -> Option<String> {
    if case.kind == "sql_parse" {
        return case.parser_dialect.clone();
    }
    Some(
        case.dialect
            .clone()
            .unwrap_or_else(|| "postgres".to_string()),
    )
}

fn assert_offline_modeled_feature_coverage(results: &OfflineResults) {
    for operation in EXPECTED_OPERATIONS {
        assert_success_feature(results, &format!("operation.{operation}"));
    }
    for clarification in EXPECTED_CLARIFICATION_KINDS {
        assert_success_feature(results, &format!("clarification.{clarification}"));
    }
    for answer in EXPECTED_ANSWERS {
        assert_success_feature(results, &format!("answer.{answer}"));
    }
    for feature in EXPECTED_ROLLBACK_FEATURES {
        assert_success_feature(results, feature);
    }
    for feature in EXPECTED_UNSUPPORTED_FEATURES {
        assert_success_feature(results, feature);
    }

    assert_prefixed_evidence(results, "renderer.postgres.");
    assert_prefixed_evidence(results, "renderer.sqlite.");
    assert_prefixed_evidence(results, "parser.postgres.");
    assert_prefixed_evidence(results, "parser.sqlite.");
    assert_prefixed_evidence(results, "parser.mysql.");
}

fn assert_success_feature(results: &OfflineResults, feature: &str) {
    let cell = results
        .features
        .get(feature)
        .unwrap_or_else(|| panic!("offline results are missing feature '{feature}'"));
    assert_eq!(
        cell.status,
        OfflineResultStatus::Success,
        "offline feature '{feature}' is not accepted as successful evidence"
    );
    assert!(
        !cell.evidence.is_empty(),
        "offline feature '{feature}' must list evidence"
    );
}

fn assert_prefixed_evidence(results: &OfflineResults, prefix: &str) {
    let mut matches = 0;
    for (feature, cell) in &results.features {
        if feature.starts_with(prefix) {
            matches += 1;
            assert_eq!(
                cell.status,
                OfflineResultStatus::Success,
                "offline feature '{feature}' is not accepted as successful evidence"
            );
            assert!(
                !cell.evidence.is_empty(),
                "offline feature '{feature}' must list evidence"
            );
        }
    }
    assert!(
        matches > 0,
        "offline results must include {prefix} evidence"
    );
}

fn assert_feature_catalog_shape(catalog: &FeatureCatalog) {
    let actual = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let expected = EXPECTED_ONLINE_FEATURES
        .iter()
        .copied()
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "online feature catalog rows drifted");

    for feature in &catalog.features {
        assert!(
            !feature.label.trim().is_empty(),
            "online feature '{}' must have a label",
            feature.id
        );
    }
}

fn assert_online_case_metadata(
    catalog: &FeatureCatalog,
    cases: &BTreeMap<String, OnlineCaseSummary>,
) {
    let feature_ids = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut descriptions = BTreeSet::new();
    for (name, case) in cases {
        assert!(
            descriptions.insert(case.description.as_str()),
            "online case '{name}' has duplicate description '{}'",
            case.description
        );
        assert!(
            !case.features.is_empty(),
            "online case '{name}' must list at least one feature"
        );
        for feature in &case.features {
            assert!(
                feature_ids.contains(feature.as_str()),
                "online case '{name}' references unknown feature '{feature}'"
            );
        }
        for dialect in case.dialects.keys() {
            assert!(
                SUPPORT_DIALECTS.contains(&dialect.as_str()),
                "online case '{name}' uses unsupported dialect key '{dialect}'"
            );
        }
    }
}

fn assert_online_results_shape(catalog: &FeatureCatalog, results: &OnlineResults) {
    let expected = catalog
        .features
        .iter()
        .map(|feature| feature.id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = results
        .features
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    assert_eq!(actual, expected, "online result feature rows drifted");

    for (feature, dialects) in &results.features {
        let actual_dialects = dialects.keys().map(String::as_str).collect::<BTreeSet<_>>();
        let expected_dialects = SUPPORT_DIALECTS.iter().copied().collect::<BTreeSet<_>>();
        assert_eq!(
            actual_dialects, expected_dialects,
            "online result feature '{feature}' must classify every dialect"
        );
    }
}

fn assert_online_results_evidence(
    results: &OnlineResults,
    cases: &BTreeMap<String, OnlineCaseSummary>,
) {
    for (feature, dialects) in &results.features {
        for (dialect, cell) in dialects {
            match cell.status {
                OnlineResultStatus::Success | OnlineResultStatus::Failure => assert!(
                    !cell.evidence.is_empty(),
                    "online result {feature}.{dialect} must include evidence for {:?}",
                    cell.status
                ),
                OnlineResultStatus::Unimplemented => assert!(
                    cell.reason
                        .as_ref()
                        .or(cell.note.as_ref())
                        .is_some_and(|reason| !reason.trim().is_empty()),
                    "online result {feature}.{dialect} needs a reason when unimplemented"
                ),
            }
            for evidence in &cell.evidence {
                assert_online_evidence(feature, dialect, evidence, cases);
            }
        }
    }
}

fn assert_online_evidence(
    feature: &str,
    dialect: &str,
    evidence: &OnlineEvidence,
    cases: &BTreeMap<String, OnlineCaseSummary>,
) {
    let case = cases.get(&evidence.case).unwrap_or_else(|| {
        panic!(
            "online result {feature}.{dialect} references missing case '{}'",
            evidence.case
        )
    });
    assert!(
        case.features.iter().any(|value| value == feature),
        "online result {feature}.{dialect} references case '{}' that does not list the feature",
        evidence.case
    );
    assert!(
        case.dialects.contains_key(dialect),
        "online result {feature}.{dialect} references case '{}' without dialect section",
        evidence.case
    );
    assert_eq!(
        evidence.description, case.description,
        "online result {feature}.{dialect} has stale description for case '{}'",
        evidence.case
    );
    assert!(
        !evidence.checks.is_empty(),
        "online result {feature}.{dialect} evidence '{}' must list checks",
        evidence.case
    );
}

fn assert_support_matrix_shape(
    support: &SupportMatrix,
    online: &OnlineResults,
    offline: &OfflineResults,
) {
    let mut ids = BTreeSet::new();
    for row in &support.rows {
        assert!(
            ids.insert(row.id.as_str()),
            "duplicate support matrix row id '{}'",
            row.id
        );
        assert!(
            !row.label.trim().is_empty(),
            "support matrix row '{}' must have a label",
            row.id
        );
        for dialect in SUPPORT_DIALECTS {
            let cell = row.dialects.get(*dialect).unwrap_or_else(|| {
                panic!(
                    "support matrix row '{}' is missing dialect '{}'",
                    row.id, dialect
                )
            });
            assert_support_cell(row, dialect, cell, online, offline);
        }
    }
}

fn assert_support_cell(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
    online: &OnlineResults,
    offline: &OfflineResults,
) {
    match cell.status {
        SupportStatus::Supported => {
            assert_support_evidence_present(row, dialect, cell);
            assert_support_evidence_success(row, dialect, cell, online, offline);
        }
        SupportStatus::Partial => {
            assert_support_note_present(row, dialect, cell);
            assert_support_evidence_present(row, dialect, cell);
            assert_support_evidence_success(row, dialect, cell, online, offline);
        }
        SupportStatus::Unsupported => {
            assert_support_note_present(row, dialect, cell);
            assert_support_evidence_success(row, dialect, cell, online, offline);
        }
        SupportStatus::Planned => assert_support_note_present(row, dialect, cell),
    }
}

fn assert_support_note_present(row: &SupportRow, dialect: &str, cell: &SupportCell) {
    assert!(
        cell.note
            .as_ref()
            .is_some_and(|note| !note.trim().is_empty()),
        "support matrix row '{}'.{} needs a note for {:?}",
        row.id,
        dialect,
        cell.status
    );
}

fn assert_support_evidence_present(row: &SupportRow, dialect: &str, cell: &SupportCell) {
    assert!(
        !cell.evidence.online.is_empty() || !cell.evidence.offline.is_empty(),
        "support matrix row '{}'.{} needs accepted evidence",
        row.id,
        dialect
    );
}

fn assert_support_evidence_success(
    row: &SupportRow,
    dialect: &str,
    cell: &SupportCell,
    online: &OnlineResults,
    offline: &OfflineResults,
) {
    for feature in &cell.evidence.online {
        let result = online
            .features
            .get(feature)
            .and_then(|dialects| dialects.get(dialect))
            .unwrap_or_else(|| {
                panic!(
                    "support matrix row '{}'.{} references missing online feature '{}'",
                    row.id, dialect, feature
                )
            });
        assert_eq!(
            result.status,
            OnlineResultStatus::Success,
            "support matrix row '{}'.{} needs online feature '{}' to be success",
            row.id,
            dialect,
            feature
        );
    }
    for feature in &cell.evidence.offline {
        let result = offline.features.get(feature).unwrap_or_else(|| {
            panic!(
                "support matrix row '{}'.{} references missing offline feature '{}'",
                row.id, dialect, feature
            )
        });
        assert_eq!(
            result.status,
            OfflineResultStatus::Success,
            "support matrix row '{}'.{} needs offline feature '{}' to be success",
            row.id,
            dialect,
            feature
        );
    }
}

fn assert_readme_support_table(
    root: &Path,
    support: &SupportMatrix,
    online: &OnlineResults,
    offline: &OfflineResults,
) {
    let readme_path = root.join("README.md");
    let readme = fs::read_to_string(&readme_path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", readme_path.display()));
    let actual = generated_block(&readme);
    let expected = format!(
        "{SUPPORT_TABLE_START}\n{}\n{SUPPORT_TABLE_END}",
        render_support_table(support, online, offline)
    );
    assert_eq!(
        actual, expected,
        "README support table is stale; run `cargo run --bin gaman-support-matrix -- --update-readme`"
    );
}

fn generated_block(readme: &str) -> &str {
    let start = readme
        .find(SUPPORT_TABLE_START)
        .expect("README support table start marker is missing");
    let after_start = start + SUPPORT_TABLE_START.len();
    let relative_end = readme[after_start..]
        .find(SUPPORT_TABLE_END)
        .expect("README support table end marker is missing");
    let end = after_start + relative_end + SUPPORT_TABLE_END.len();
    &readme[start..end]
}

fn render_support_table(
    support: &SupportMatrix,
    _online: &OnlineResults,
    _offline: &OfflineResults,
) -> String {
    let mut lines = vec![
        "| Feature | PostgreSQL | SQLite | MySQL / MariaDB |".to_string(),
        "| --- | --- | --- | --- |".to_string(),
    ];
    for row in &support.rows {
        let postgres = support_symbol(row, "postgres");
        let sqlite = support_symbol(row, "sqlite");
        let mysql = support_symbol(row, "mysql");
        lines.push(format!(
            "| {} | {postgres} | {sqlite} | {mysql} |",
            row.label
        ));
    }
    append_support_notes(&mut lines, support);
    lines.join("\n")
}

fn support_symbol(row: &SupportRow, dialect: &str) -> &'static str {
    match row.dialects[dialect].status {
        SupportStatus::Supported => "✅",
        SupportStatus::Partial => "◐",
        SupportStatus::Planned => "🚧",
        SupportStatus::Unsupported => "❌",
    }
}

fn append_support_notes(lines: &mut Vec<String>, support: &SupportMatrix) {
    let notes = support_notes(support);
    if notes.is_empty() {
        return;
    }
    lines.push(String::new());
    lines.push("Notes:".to_string());
    for note in notes {
        lines.push(format!("- {note}"));
    }
}

fn support_notes(support: &SupportMatrix) -> Vec<String> {
    let mut notes = Vec::new();
    for row in &support.rows {
        let mut grouped = BTreeMap::<&str, Vec<&str>>::new();
        for dialect in SUPPORT_DIALECTS {
            let cell = &row.dialects[*dialect];
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

const EXPECTED_OPERATIONS: &[&str] = &[
    "add_column",
    "add_constraint",
    "add_foreign_key",
    "add_index",
    "alter_column",
    "alter_enum",
    "alter_function",
    "alter_trigger",
    "create_enum",
    "create_extension",
    "create_function",
    "create_table",
    "create_trigger",
    "create_view",
    "drop_column",
    "drop_constraint",
    "drop_enum",
    "drop_extension",
    "drop_foreign_key",
    "drop_function",
    "drop_index",
    "drop_table",
    "drop_trigger",
    "drop_view",
    "rename_column",
    "rename_enum_value",
    "rename_table",
    "replace_view",
    "statement",
];

const EXPECTED_CLARIFICATION_KINDS: &[&str] = &[
    "not_null_add",
    "not_null_change",
    "rename_column",
    "rename_enum_value",
    "rename_table",
    "type_cast",
    "unknown_type",
];

const EXPECTED_ANSWERS: &[&str] = &[
    "keep_type",
    "not_null_default",
    "not_null_manual",
    "not_null_nullable",
    "rename_no",
    "rename_to",
    "type_cast",
    "type_cast_implicit",
    "use_type",
];

const EXPECTED_ROLLBACK_FEATURES: &[&str] = &[
    "rollback.non_reversible",
    "rollback.reversible",
    "rollback.unknown_selected_id",
];

const EXPECTED_UNSUPPORTED_FEATURES: &[&str] = &[
    "unsupported.primary_key_mutation",
    "unsupported.sqlite_enum",
    "unsupported.sqlite_extension",
    "unsupported.sqlite_function",
    "unsupported.sqlite_function_trigger",
    "unsupported.sqlite_schema_qualified_table",
];

const SUPPORT_DIALECTS: &[&str] = &["postgres", "sqlite", "mysql"];

const SUPPORT_TABLE_START: &str = "<!-- gaman:support-matrix:start -->";
const SUPPORT_TABLE_END: &str = "<!-- gaman:support-matrix:end -->";

const EXPECTED_ONLINE_FEATURES: &[&str] = &[
    "live_migration_application",
    "idempotent_migrations",
    "target_migrations",
    "rollback_migrations",
    "transaction_rollback",
    "lock_cleanup",
    "migration_tracking",
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
    "schemas_namespaces",
    "extensions",
    "enums",
    "functions_opaque",
    "trigger_query_objects",
    "views_opaque",
    "raw_sql_statements",
    "sqlite_rebuild_planner",
    "unsupported_feature_errors",
    "ownership_scoped_verify",
    "data_preservation",
];
