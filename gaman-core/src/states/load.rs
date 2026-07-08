use crate::dialects::Dialect;

use super::*;

impl Schema {
    pub fn from_yaml_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        let mut state: Self = serde_yaml::from_str(s)?;
        state.normalize();
        Ok(state.prepare(dialect)?)
    }

    pub fn from_json_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        let mut state: Self = serde_json::from_str(s)?;
        state.normalize();
        Ok(state.prepare(dialect)?)
    }

    pub fn from_sql_str(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(crate::parsers::parse_sql(s, dialect)?.prepare(dialect)?)
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
