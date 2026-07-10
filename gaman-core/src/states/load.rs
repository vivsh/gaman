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
    /// functions, extensions, enums) use last-writer-wins.
    pub fn merge(mut self, other: Schema) -> Result<Self, SchemaLoadError> {
        for (name, table) in other.tables {
            if self.tables.contains_key(&name) {
                return Err(SchemaLoadError::DuplicateTable(name));
            }
            self.tables.insert(name, table);
        }
        self.views.extend(other.views);
        self.functions.extend(other.functions);
        self.extensions.extend(other.extensions);
        self.enums.extend(other.enums);
        Ok(self)
    }

    pub fn builder(dialect: Dialect) -> SchemaBuilder {
        SchemaBuilder::new(dialect)
    }
}
