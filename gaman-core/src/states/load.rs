use crate::dialects::Dialect;

use super::*;

impl Schema {
    pub fn from_yaml_str(s: &str) -> Result<Self, SchemaLoadError> {
        let mut state: Self = serde_yaml::from_str(s)?;
        state.normalize();
        Ok(state)
    }

    pub fn from_yaml_str_for_dialect(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(Self::from_yaml_str(s)?.prepare(dialect)?)
    }

    #[cfg(feature = "fs")]
    pub fn from_yaml_file(path: &std::path::Path) -> Result<Self, SchemaLoadError> {
        let raw = std::fs::read_to_string(path)
            .map_err(|e| SchemaLoadError::Io(path.display().to_string(), e))?;
        Self::from_yaml_str(&raw)
    }

    pub fn from_json_str(s: &str) -> Result<Self, SchemaLoadError> {
        let mut state: Self = serde_json::from_str(s)?;
        state.normalize();
        Ok(state)
    }

    pub fn from_json_str_for_dialect(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(Self::from_json_str(s)?.prepare(dialect)?)
    }

    pub fn from_sql_str(s: &str) -> Result<Self, SchemaLoadError> {
        Ok(crate::sql::parse_sql(s)?)
    }

    pub fn from_sql_str_for_dialect(s: &str, dialect: Dialect) -> Result<Self, SchemaLoadError> {
        Ok(Self::from_sql_str(s)?.prepare(dialect)?)
    }

    #[cfg(feature = "fs")]
    pub fn from_sql_file(path: &std::path::Path) -> Result<Self, SchemaLoadError> {
        Ok(crate::sql::parse_sql_file(path)?)
    }

    #[cfg(feature = "fs")]
    pub fn from_yaml_dir(dir: &std::path::Path) -> Result<Self, SchemaLoadError> {
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| SchemaLoadError::Io(dir.display().to_string(), e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("yaml"))
            .collect();
        entries.sort();
        let mut merged = Self::default();
        for path in &entries {
            let fragment = Self::from_yaml_file(path)?;
            for (name, table) in fragment.tables {
                if merged.tables.contains_key(&name) {
                    return Err(SchemaLoadError::Merge {
                        table: name,
                        a: dir.display().to_string(),
                        b: path.display().to_string(),
                    });
                }
                merged.tables.insert(name, table);
            }
        }
        Ok(merged)
    }

    #[cfg(feature = "fs")]
    pub fn from_dir(dir: &std::path::Path) -> Result<Self, SchemaLoadError> {
        let mut entries: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .map_err(|e| SchemaLoadError::Io(dir.display().to_string(), e))?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("yaml") | Some("sql")
                )
            })
            .collect();
        entries.sort();
        let mut merged = Self::default();
        for path in &entries {
            let fragment = Self::from_file(path)?;
            for (name, table) in fragment.tables {
                if merged.tables.contains_key(&name) {
                    return Err(SchemaLoadError::Merge {
                        table: name,
                        a: dir.display().to_string(),
                        b: path.display().to_string(),
                    });
                }
                merged.tables.insert(name, table);
            }
            for (k, v) in fragment.views {
                merged.views.insert(k, v);
            }
            for (k, v) in fragment.functions {
                merged.functions.insert(k, v);
            }
            for (k, v) in fragment.extensions {
                merged.extensions.insert(k, v);
            }
            for (k, v) in fragment.enums {
                merged.enums.insert(k, v);
            }
        }
        Ok(merged)
    }

    #[cfg(feature = "fs")]
    pub fn from_file(path: &std::path::Path) -> Result<Self, SchemaLoadError> {
        if path.is_dir() {
            Self::from_dir(path)
        } else if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            Self::from_sql_file(path)
        } else {
            Self::from_yaml_file(path)
        }
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
