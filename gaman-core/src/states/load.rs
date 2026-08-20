use crate::dialects::Dialect;

use super::*;

fn yaml_input_schema(s: &str) -> Result<Schema, SchemaLoadError> {
    Ok(serde_yaml::from_str::<InputSchema>(s)?.into_schema())
}

fn json_input_schema(s: &str) -> Result<Schema, SchemaLoadError> {
    Ok(serde_json::from_str::<InputSchema>(s)?.into_schema())
}

impl Schema {
    /// Prepare a schema loaded from any authored source.
    ///
    /// This is the shared ingestion boundary for YAML, JSON, SQL lowering, and
    /// Rust builder input. It normalizes model shape, canonicalizes
    /// dialect-specific names/types, and validates the result exactly once.
    pub fn prepare_loaded(self, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        self.prepare_loaded_with_issues(dialect, Vec::new())
    }

    /// Completes the shared authored-input boundary with accumulated builder failures.
    pub(crate) fn prepare_loaded_with_issues(
        self,
        dialect: Dialect,
        mut issues: Vec<SchemaBuilderIssue>,
    ) -> Result<Self, SchemaLoadError> {
        issues.extend(validate_authored_raw(&self, dialect));
        if !issues.is_empty() {
            return Err(SchemaValidationError::Builder(SchemaBuilderErrors::new(issues)).into());
        }
        Ok(self.prepare(dialect)?)
    }

    pub fn from_yaml_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        yaml_input_schema(s)?.prepare_loaded(dialect)
    }

    pub fn from_json_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        json_input_schema(s)?.prepare_loaded(dialect)
    }

    pub fn from_sql_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(crate::parsers::parse_sql(s, dialect)?)
    }

    #[doc(hidden)]
    pub fn from_sql_str_raw(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(crate::parsers::parse_sql_raw(s, dialect)?)
    }

    #[doc(hidden)]
    pub fn from_yaml_str_input_raw(s: &str) -> Result<Self, SchemaLoadError> {
        yaml_input_schema(s)
    }

    #[doc(hidden)]
    pub fn from_json_str_input_raw(s: &str) -> Result<Self, SchemaLoadError> {
        json_input_schema(s)
    }

    /// Merge `other` into `self`. Duplicate table names are an error; other objects (views,
    /// functions, extensions, sequences, enums) use their declared merge policy.
    pub fn merge(mut self, other: Schema) -> Result<Self, SchemaLoadError> {
        for (name, table) in other.tables {
            if self.tables.contains_key(&name) {
                return Err(SchemaLoadError::DuplicateTable(name));
            }
            self.tables.insert(name, table);
        }
        for (table, incoming) in other.managed_rows {
            match self.managed_rows.get_mut(&table) {
                Some(existing) => {
                    crate::managed_rows::merge_declaration(&table, existing, incoming)
                        .map_err(SchemaValidationError::Invalid)?
                }
                None => {
                    self.managed_rows.insert(table, incoming);
                }
            }
        }
        self.views.extend(other.views);
        for (name, function) in other.functions {
            if self.functions.insert(name.clone(), function).is_some() {
                return Err(SchemaValidationError::Invalid(format!("duplicate function '{name}' when merging schemas")).into());
            }
        }
        self.extensions.extend(other.extensions);
        for (name, sequence) in other.sequences {
            if self.sequences.insert(name.clone(), sequence).is_some() {
                return Err(SchemaValidationError::Invalid(format!(
                    "duplicate sequence '{name}' when merging schemas"
                ))
                .into());
            }
        }
        self.enums.extend(other.enums);
        Ok(self)
    }

    pub fn builder(dialect: Dialect) -> SchemaBuilder {
        SchemaBuilder::new(dialect)
    }
}
