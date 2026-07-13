//! Regression tests for fail-closed evidence artifact publication.

#[path = "support/evidence_io.rs"]
#[allow(dead_code)]
mod evidence_io;

use serde::Serialize;
use std::fs;
use std::path::PathBuf;

/// Verifies a complete document atomically replaces its accepted predecessor.
#[test]
fn atomic_yaml_publication_replaces_complete_file() {
    let directory = unique_directory("replace");
    fs::create_dir_all(&directory).expect("create evidence test directory");
    let destination = directory.join("accepted.yaml");
    fs::write(&destination, "generation: old\n").expect("seed accepted evidence");

    evidence_io::write_yaml_atomic(&destination, &Sample { generation: "new" })
        .expect("publish evidence");

    let accepted = fs::read_to_string(&destination).expect("read accepted evidence");
    assert_eq!(accepted, "generation: new\n");
    assert_no_temporary_files(&directory);
    fs::remove_dir_all(directory).expect("remove evidence test directory");
}

/// Verifies serialization failure preserves accepted evidence and leaves no temporary file.
#[test]
fn failed_yaml_publication_preserves_accepted_file() {
    let directory = unique_directory("failure");
    fs::create_dir_all(&directory).expect("create evidence test directory");
    let destination = directory.join("accepted.yaml");
    fs::write(&destination, "generation: accepted\n").expect("seed accepted evidence");

    let error = evidence_io::write_yaml_atomic(&destination, &FailsSerialization);

    assert!(error.is_err());
    let accepted = fs::read_to_string(&destination).expect("read accepted evidence");
    assert_eq!(accepted, "generation: accepted\n");
    assert_no_temporary_files(&directory);
    fs::remove_dir_all(directory).expect("remove evidence test directory");
}

#[derive(Serialize)]
struct Sample<'a> {
    generation: &'a str,
}

struct FailsSerialization;

impl Serialize for FailsSerialization {
    fn serialize<S>(&self, _serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        Err(serde::ser::Error::custom("intentional fixture failure"))
    }
}

/// Creates an isolated path without depending on external temporary-file behavior.
fn unique_directory(label: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "gaman-evidence-{label}-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ))
}

/// Ensures unsuccessful publication cannot leave partial evidence artifacts.
fn assert_no_temporary_files(directory: &std::path::Path) {
    let entries = fs::read_dir(directory)
        .expect("read evidence directory")
        .map(|entry| entry.expect("read evidence entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries, vec!["accepted.yaml"]);
}
