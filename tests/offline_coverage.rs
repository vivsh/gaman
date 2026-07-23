use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const README_FEATURE_ROWS: &[&str] = &[
    "offline_planning",
    "live_migration_application",
    "live_introspection",
    "live_verify_db",
    "table_create_drop_rename",
    "column_add_drop_rename",
    "column_type_null_default",
    "composite_primary_keys",
    "single_foreign_keys",
    "composite_foreign_keys",
    "unique_constraints",
    "indexes",
    "extensions",
    "enums",
    "functions",
    "trigger_query_objects",
];

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
    #[serde(default)]
    assertions: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct OfflineCaseSummary {
    description: String,
    group: String,
    features: Vec<String>,
    kind: String,
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
    generation: String,
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
    online: Vec<OnlineEvidenceRef>,
    #[serde(default)]
    offline: Vec<OfflineEvidenceRef>,
}

#[derive(Debug, Deserialize)]
struct OnlineEvidenceRef {
    case: String,
    checks: Vec<String>,
}

#[derive(Debug, Deserialize)]
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

/// Verifies accepted offline evidence comes from real fixtures and covers modeled behavior.
#[test]
fn offline_evidence_matrix_is_complete_and_current() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let catalog_path = root.join("tests/cases/offline-features.yaml");
    let results_path = evidence_path(
        root,
        "GAMAN_OFFLINE_RESULTS",
        "results/offline-results.yaml",
    );
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
    let results_path = evidence_path(root, "GAMAN_ONLINE_RESULTS", "results/online-results.yaml");
    let offline_results_path = evidence_path(
        root,
        "GAMAN_OFFLINE_RESULTS",
        "results/offline-results.yaml",
    );
    let catalog: FeatureCatalog = read_yaml(&catalog_path);
    let support: SupportMatrix = read_yaml(&support_path);
    let results: OnlineResults = read_yaml(&results_path);
    let offline_results: OfflineResults = read_yaml(&offline_results_path);
    let cases = read_online_cases(root);

    assert_eq!(
        results.generation, offline_results.generation,
        "accepted evidence files belong to different generations"
    );

    assert_feature_catalog_shape(&catalog);
    assert_online_case_metadata(&catalog, &cases);
    assert_online_results_shape(&catalog, &results);
    assert_online_results_evidence(&results, &cases);
    assert_support_matrix_shape(&support, &results, &offline_results);
    assert_readme_support_table(root, &support, &results, &offline_results);
    assert_support_evidence_doc_current(root);
}

fn read_yaml<T: for<'de> Deserialize<'de>>(path: &Path) -> T {
    let raw = fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
    serde_yaml::from_str(&raw)
        .unwrap_or_else(|error| panic!("failed to parse {}: {error}", path.display()))
}

/// Resolves staged evidence paths when validating a publication transaction.
fn evidence_path(root: &Path, variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join(fallback))
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

    for (feature, dialects) in &results.features {
        assert!(
            dialects
                .values()
                .any(|cell| cell.status == OfflineResultStatus::Success),
            "accepted offline result '{feature}' must have successful dialect evidence"
        );
        assert!(
            dialects
                .values()
                .all(|cell| cell.status != OfflineResultStatus::Failure),
            "accepted offline result '{feature}' contains failed evidence"
        );
    }
}

fn assert_offline_results_evidence(
    results: &OfflineResults,
    cases: &BTreeMap<String, OfflineCaseSummary>,
) {
    for (feature, dialects) in &results.features {
        for (dialect, cell) in dialects {
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
                    Some(dialect.clone()),
                    "offline result {feature} has stale dialect for case '{}'",
                    evidence.case
                );
                assert!(
                    !evidence.assertions.is_empty(),
                    "offline evidence must list assertions"
                );
            }
        }
    }
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
    let dialects = results
        .features
        .get(feature)
        .unwrap_or_else(|| panic!("offline results are missing feature '{feature}'"));
    assert!(
        dialects
            .values()
            .any(|cell| cell.status == OfflineResultStatus::Success && !cell.evidence.is_empty()),
        "offline feature '{feature}' is not accepted as successful evidence"
    );
}

fn assert_prefixed_evidence(results: &OfflineResults, prefix: &str) {
    let mut matches = 0;
    for (feature, dialects) in &results.features {
        if feature.starts_with(prefix) {
            matches += 1;
            assert!(
                dialects
                    .values()
                    .any(|cell| cell.status == OfflineResultStatus::Success
                        && !cell.evidence.is_empty()),
                "offline feature '{feature}' is not accepted as successful evidence"
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
    for descriptor in &cell.evidence.online {
        let evidence = online
            .features
            .values()
            .filter_map(|dialects| dialects.get(dialect))
            .filter(|result| result.status == OnlineResultStatus::Success)
            .flat_map(|result| &result.evidence)
            .find(|evidence| evidence.case == descriptor.case)
            .unwrap_or_else(|| {
                panic!(
                    "support matrix row '{}'.{} lacks online case '{}'",
                    row.id, dialect, descriptor.case
                )
            });
        for check in &descriptor.checks {
            assert!(
                evidence.checks.contains(check),
                "online case '{}' did not execute check '{check}'",
                descriptor.case
            );
        }
    }
    for descriptor in &cell.evidence.offline {
        let evidence = offline
            .features
            .values()
            .filter_map(|dialects| dialects.get(dialect))
            .filter(|result| result.status == OfflineResultStatus::Success)
            .flat_map(|result| &result.evidence)
            .find(|evidence| evidence.case == descriptor.case)
            .unwrap_or_else(|| {
                panic!(
                    "support matrix row '{}'.{} lacks offline case '{}'",
                    row.id, dialect, descriptor.case
                )
            });
        for assertion in &descriptor.assertions {
            assert!(
                evidence.assertions.contains(assertion),
                "offline case '{}' did not execute assertion '{assertion}'",
                descriptor.case
            );
        }
    }
}

fn assert_readme_support_table(
    root: &Path,
    support: &SupportMatrix,
    online: &OnlineResults,
    offline: &OfflineResults,
) {
    let readme_path = evidence_path(root, "GAMAN_README_PATH", "README.md");
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

fn assert_support_evidence_doc_current(root: &Path) {
    let binary = env!("CARGO_BIN_EXE_gaman-evidence-doc");
    let output = std::process::Command::new(binary)
        .arg("--check")
        .current_dir(root)
        .output()
        .unwrap_or_else(|error| panic!("failed to run gaman-evidence-doc --check: {error}"));
    assert!(
        output.status.success(),
        "support evidence doc is stale; stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
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
    online: &OnlineResults,
    _offline: &OfflineResults,
) -> String {
    let mut lines = vec![
        format!("<!-- evidence-generation: {} -->", online.generation),
        "| Feature | PostgreSQL | SQLite | MySQL | MariaDB |".to_string(),
        "| --- | --- | --- | --- | --- |".to_string(),
    ];
    for row_id in README_FEATURE_ROWS {
        let row = support
            .rows
            .iter()
            .find(|row| row.id == *row_id)
            .expect("README feature row is missing from the support matrix");
        lines.push(format!(
            "| {} | {} | {} | {} | {} |",
            row.label,
            linked_support_symbol(row, "postgres"),
            linked_support_symbol(row, "sqlite"),
            linked_support_symbol(row, "mysql"),
            linked_support_symbol(row, "mariadb")
        ));
    }
    lines.join("\n")
}

fn linked_support_symbol(row: &SupportRow, dialect: &str) -> String {
    format!(
        "[{}](docs/support-evidence.md#lifecycle-compatibility)",
        support_symbol(row, dialect)
    )
}

fn support_symbol(row: &SupportRow, dialect: &str) -> &'static str {
    match row.dialects[dialect].status {
        SupportStatus::Supported => "✅",
        SupportStatus::Partial => "◐",
        SupportStatus::Planned => "🚧",
        SupportStatus::Unsupported => "❌",
    }
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

const SUPPORT_DIALECTS: &[&str] = &["postgres", "sqlite", "mysql", "mariadb"];

const SUPPORT_TABLE_START: &str = "<!-- gaman:support-matrix:start -->";
const SUPPORT_TABLE_END: &str = "<!-- gaman:support-matrix:end -->";

const EXPECTED_ONLINE_FEATURES: &[&str] = &[
    "live_migration_application",
    "idempotent_migrations",
    "target_migrations",
    "rollback_migrations",
    "transaction_rollback",
    "non_transactional_failure_reporting",
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
    "foreign_key_actions",
    "composite_foreign_keys",
    "indexes",
    "unique_constraints",
    "check_constraints",
    "indexes_constraints",
    "partial_indexes",
    "opaque_index_metadata",
    "opaque_presence_verify",
    "quoted_identifiers",
    "unmanaged_table_options",
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
