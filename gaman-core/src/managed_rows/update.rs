//! Validation and classification of managed-row declaration transitions.

use super::ManagedRow;

/// Requires a nonempty, unchanged identity and an actual declaration change.
pub(crate) fn validate(key: &[String], old: &ManagedRow, new: &ManagedRow) -> Result<(), String> {
    if key.is_empty() {
        return Err("managed row update requires a nonempty key".to_string());
    }
    if old.identity(key)? != new.identity(key)? {
        return Err("managed row key changes require delete plus insert".to_string());
    }
    if old == new {
        return Err("managed row update has no changed values".to_string());
    }
    Ok(())
}

/// Identifies a strict subset of unchanged values, not a database value assignment.
pub(crate) fn removes_only(old: &ManagedRow, new: &ManagedRow) -> bool {
    old.values.len() > new.values.len()
        && new
            .values
            .iter()
            .all(|(name, value)| old.values.get(name) == Some(value))
}
