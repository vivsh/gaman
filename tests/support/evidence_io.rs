//! Atomic publication helpers shared by evidence-producing test harnesses.

use serde::Serialize;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Returns the generation identifier supplied by the transactional publisher.
pub fn generation_id() -> String {
    std::env::var("GAMAN_EVIDENCE_GENERATION").unwrap_or_else(|_| "local".to_string())
}

/// Serializes YAML and atomically publishes the complete document.
pub fn write_yaml_atomic<T: Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let yaml = serde_yaml::to_string(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_atomic(path, yaml.as_bytes())
}

/// Atomically publishes bytes through a synchronized sibling temporary file.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let temporary = temporary_path(path);
    let result = write_and_publish(&temporary, path, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

/// Writes and synchronizes one complete artifact before making it visible.
fn write_and_publish(temporary: &Path, destination: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()?;
    drop(file);
    fs::rename(temporary, destination)
}

/// Builds a unique temporary filename in the destination directory.
fn temporary_path(path: &Path) -> PathBuf {
    let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("evidence");
    path.with_file_name(format!(".{name}.{}.{}.tmp", std::process::id(), sequence))
}
