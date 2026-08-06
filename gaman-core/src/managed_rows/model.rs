use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One safely serializable value in a managed database row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ManagedValue(pub serde_json::Value);

impl ManagedValue {
    /// Reports whether this value is SQL NULL.
    pub fn is_null(&self) -> bool {
        self.0.is_null()
    }
}

/// Column values owned by one managed-row identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct ManagedRow {
    /// Values keyed by database column name.
    pub values: BTreeMap<String, ManagedValue>,
}

impl ManagedRow {
    /// Converts one Serde record into a string-keyed managed row.
    pub fn from_serializable<T: Serialize>(value: &T) -> Result<Self, String> {
        let validation = serde_yaml::to_value(value).map_err(|error| error.to_string())?;
        validate_serialized_value(&validation)?;
        let value = serde_json::to_value(value).map_err(|error| error.to_string())?;
        let serde_json::Value::Object(values) = value else {
            return Err("managed row must serialize as an object".to_string());
        };
        Ok(Self {
            values: values
                .into_iter()
                .map(|(name, value)| (name, ManagedValue(value)))
                .collect(),
        })
    }

    /// Returns a deterministic row identity for the ordered key columns.
    pub fn identity(&self, key: &[String]) -> Result<String, String> {
        key.iter()
            .map(|column| {
                let value = self
                    .values
                    .get(column)
                    .ok_or_else(|| format!("missing key column '{column}'"))?;
                if value.is_null() {
                    return Err(format!("key column '{column}' cannot be null"));
                }
                Ok(format!(
                    "{column}={}",
                    serde_json::to_string(&value.0).map_err(|error| error.to_string())?
                ))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|parts| parts.join(","))
    }
}

/// Rejects values that JSON canonicalization would otherwise erase or
/// reinterpret, preserving managed-row equality and SQL preconditions.
fn validate_serialized_value(value: &serde_yaml::Value) -> Result<(), String> {
    match value {
        serde_yaml::Value::Number(number)
            if number.as_f64().is_some_and(|value| !value.is_finite()) =>
        {
            Err("managed row numbers must be finite".to_string())
        }
        serde_yaml::Value::Sequence(values) => {
            values.iter().try_for_each(validate_serialized_value)
        }
        serde_yaml::Value::Mapping(values) => values.iter().try_for_each(|(key, value)| {
            if !matches!(key, serde_yaml::Value::String(_)) {
                return Err("managed row objects must use string keys".to_string());
            }
            validate_serialized_value(value)
        }),
        serde_yaml::Value::Tagged(tagged) => validate_serialized_value(&tagged.value),
        _ => Ok(()),
    }
}

/// One complete managed-row declaration for a target table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ManagedRows {
    /// Desired rows owned by migration history.
    pub rows: Vec<ManagedRow>,
}

impl ManagedRows {
    /// Serializes a collection of records into one managed-row declaration.
    pub fn from_serializable<T: Serialize>(
        rows: impl IntoIterator<Item = T>,
    ) -> Result<Self, String> {
        Ok(Self {
            rows: rows
                .into_iter()
                .map(|row| ManagedRow::from_serializable(&row))
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    pub(crate) fn row_map(&self, key: &[String]) -> Result<BTreeMap<String, ManagedRow>, String> {
        self.rows
            .iter()
            .cloned()
            .map(|row| Ok((row.identity(key)?, row)))
            .collect()
    }
}
