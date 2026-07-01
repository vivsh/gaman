use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::dialects::Dialect;
use crate::operations::Operation;

pub mod builder;
pub mod errors;
pub mod types;

pub use builder::*;
pub use errors::*;
pub use types::*;

#[cfg(test)]
mod tests;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Schema {
    pub tables: BTreeMap<String, Table>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, ViewDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: BTreeMap<String, FunctionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, ExtensionDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enums: BTreeMap<String, EnumDef>,
}

impl Schema {
    /// Folds inline `references` and `check` fields declared on columns into the
    /// table-level `foreign_keys` and `constraints` vecs. Call this once after
    /// deserializing a user-authored schema file before passing it to diff or validate.
    pub fn normalize(&mut self) {
        let mut new_functions: Vec<(String, FunctionDef)> = Vec::new();
        for (table_name, table) in self.tables.iter_mut() {
            let table_name = table_name.clone();
            if table.name.is_empty() {
                table.name = table_name.clone();
            }
            normalize_table_primary_key(table);
            for col in table.columns.iter_mut() {
                if let Some(r) = col.references.take() {
                    let fk_name = r
                        .name
                        .unwrap_or_else(|| format!("{}_{}_fkey", table_name, col.name));
                    table.foreign_keys.push(ForeignKey {
                        name: fk_name,
                        from_column: col.name.clone(),
                        to_table: r.table,
                        to_column: r.column,
                    });
                }
                if let Some(expr) = col.check.take() {
                    table.constraints.push(Constraint::Check {
                        name: format!("{}_{}_check", table_name, col.name),
                        expression: expr,
                    });
                }
            }
            for trigger in table.triggers.iter_mut() {
                if trigger.name.is_none() {
                    let mut event_parts: Vec<&str> = trigger
                        .events
                        .iter()
                        .map(|e| match e {
                            TriggerEvent::Insert => "insert",
                            TriggerEvent::Update => "update",
                            TriggerEvent::Delete => "delete",
                            TriggerEvent::Truncate => "truncate",
                        })
                        .collect();
                    event_parts.sort_unstable();
                    let timing_part = match trigger.timing {
                        TriggerTiming::Before => "before",
                        TriggerTiming::After => "after",
                        TriggerTiming::InsteadOf => "instead_of",
                    };
                    trigger.name = Some(format!(
                        "{}_{}_{}_trg",
                        table_name,
                        event_parts.join("_"),
                        timing_part
                    ));
                }
                if let Some(body) = trigger.body.take() {
                    let trigger_name = trigger.name.as_deref().unwrap();
                    let fn_name = format!("{}_fn", trigger_name);
                    let lang = trigger
                        .language
                        .take()
                        .unwrap_or_else(|| "plpgsql".to_string());
                    trigger.function_name = Some(fn_name.clone());
                    new_functions.push((
                        fn_name.clone(),
                        FunctionDef {
                            name: fn_name,
                            schema: None,
                            arguments: String::new(),
                            returns: "trigger".to_string(),
                            language: lang,
                            body,
                            volatility: Volatility::Volatile,
                            security_definer: false,
                        },
                    ));
                }
            }
        }
        for (key, func) in self.functions.iter_mut() {
            if func.name.is_empty() {
                func.name = key.clone();
            }
        }
        for (key, func) in new_functions {
            self.functions.insert(key, func);
        }
    }

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

    pub fn prepare(mut self, dialect: Dialect) -> Result<Self, SchemaValidationError> {
        self.prepare_mut(&dialect)?;
        Ok(self)
    }

    pub fn prepare_mut(&mut self, dialect: &Dialect) -> Result<(), SchemaValidationError> {
        self.normalize();
        self.canonicalize(dialect);
        self.validate_checked()?;
        dialect.validate_schema(self)?;
        Ok(())
    }

    pub fn validate(&self) -> Result<(), String> {
        self.validate_checked().map_err(|err| err.to_string())
    }

    pub fn validate_checked(&self) -> Result<(), SchemaValidationError> {
        for (name, table) in &self.tables {
            if table.name.is_empty() {
                return Err(SchemaValidationError::Invalid(format!(
                    "table with key '{name}' has an empty name — omit `name:` to inherit the key, or set it explicitly"
                )));
            }
            validate_table_primary_key(name, table)?;
            let mut seen = HashSet::new();
            for col in &table.columns {
                if !seen.insert(col.name.as_str()) {
                    return Err(SchemaValidationError::Invalid(format!(
                        "table '{name}' has duplicate column '{}'",
                        col.name
                    )));
                }
            }
            validate_table_references(self, name, table)?;
            for trigger in &table.triggers {
                if trigger.events.is_empty() {
                    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
                    return Err(SchemaValidationError::Invalid(format!(
                        "trigger '{tname}' on table '{name}' has no events"
                    )));
                }
                if trigger.function_name.is_none() {
                    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
                    return Err(SchemaValidationError::Invalid(format!(
                        "trigger '{tname}' on table '{name}' has no function_name (add `function_name` or inline `body`)"
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn builder(dialect: Dialect) -> SchemaBuilder {
        SchemaBuilder::new(dialect)
    }

    pub fn canonicalize(&mut self, dialect: &Dialect) {
        for table in self.tables.values_mut() {
            for col in &mut table.columns {
                let normalized = dialect.canonical_type(&col.col_type);
                if normalized != col.col_type {
                    col.col_type = normalized;
                }
            }
            normalize_table_primary_key(table);
        }
        self.normalize_schemas();
    }

    fn normalize_schemas(&mut self) {
        fn normalize_schema(schema: &mut Option<String>) {
            if let Some(s) = schema {
                if s == "public" {
                    *schema = None;
                }
            }
        }

        fn normalize_extension_schema(schema: &mut Option<String>) {
            if let Some(s) = schema {
                if s == "public" || s == "pg_catalog" {
                    *schema = None;
                }
            }
        }

        fn rekey<T>(map: &mut BTreeMap<String, T>, key_fn: impl Fn(&T) -> String) {
            let stale: Vec<String> = map
                .iter()
                .filter(|(k, v)| **k != key_fn(v))
                .map(|(k, _)| k.clone())
                .collect();
            for old_key in stale {
                if let Some(val) = map.remove(&old_key) {
                    let new_key = key_fn(&val);
                    map.insert(new_key, val);
                }
            }
        }

        for table in self.tables.values_mut() {
            normalize_schema(&mut table.schema);
        }
        rekey(&mut self.tables, |t| t.qualified_name());

        for func in self.functions.values_mut() {
            normalize_schema(&mut func.schema);
        }
        rekey(&mut self.functions, |f| f.qualified_name());

        for view in self.views.values_mut() {
            normalize_schema(&mut view.schema);
        }
        rekey(&mut self.views, |v| v.qualified_name());

        for ext in self.extensions.values_mut() {
            normalize_extension_schema(&mut ext.schema);
        }
        rekey(&mut self.extensions, |e| e.qualified_name());

        for en in self.enums.values_mut() {
            normalize_schema(&mut en.schema);
        }
        rekey(&mut self.enums, |e| e.qualified_name());
    }

    /// Apply a single operation to this state, mutating it in place.
    /// `Statement` is a no-op: it carries raw SQL that cannot
    /// be reflected into the in-memory schema model.
    pub fn apply(&mut self, op: &Operation) -> Result<(), ReplayError> {
        match op {
            Operation::CreateTable { table } => {
                let key = table.qualified_name();
                if self.tables.contains_key(&key) {
                    return Err(ReplayError::TableAlreadyExists(key));
                }
                let mut table = table.clone();
                normalize_table_primary_key(&mut table);
                self.tables.insert(key, table);
            }

            Operation::DropTable { table } => {
                let key = table.qualified_name();
                if self.tables.remove(&key).is_none() {
                    return Err(ReplayError::TableNotFound(key));
                }
            }

            Operation::RenameTable { old_name, new_name } => {
                let table = self
                    .tables
                    .remove(old_name)
                    .ok_or_else(|| ReplayError::TableNotFound(old_name.clone()))?;
                if self.tables.contains_key(new_name) {
                    return Err(ReplayError::RenameTargetExists {
                        old: old_name.clone(),
                        new: new_name.clone(),
                    });
                }
                let mut table = table;
                table.name = new_name.clone();
                self.tables.insert(new_name.clone(), table);
            }

            Operation::AddColumn { table_name, column } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if table.columns.iter().any(|c| c.name == column.name) {
                    return Err(ReplayError::ColumnAlreadyExists {
                        table: table_name.clone(),
                        column: column.name.clone(),
                    });
                }
                if column.primary_key {
                    return Err(ReplayError::PrimaryKeyMutation(table_name.clone()));
                }
                table.columns.push(column.clone());
                normalize_table_primary_key(table);
            }

            Operation::DropColumn {
                table_name, column, ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let pos = table
                    .columns
                    .iter()
                    .position(|c| c.name == column.name)
                    .ok_or_else(|| ReplayError::ColumnNotFound {
                        table: table_name.clone(),
                        column: column.name.clone(),
                    })?;
                if table.is_primary_key_column(&column.name) {
                    return Err(ReplayError::PrimaryKeyMutation(table_name.clone()));
                }
                table.columns.remove(pos);
                normalize_table_primary_key(table);
            }

            Operation::RenameColumn {
                table_name,
                old_name,
                new_name,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if table.is_primary_key_column(old_name) {
                    return Err(ReplayError::PrimaryKeyMutation(table_name.clone()));
                }
                let col = table
                    .columns
                    .iter_mut()
                    .find(|c| &c.name == old_name)
                    .ok_or_else(|| ReplayError::ColumnNotFound {
                        table: table_name.clone(),
                        column: old_name.clone(),
                    })?;
                col.name = new_name.clone();
                normalize_table_primary_key(table);
            }

            Operation::AlterColumn {
                table_name,
                old,
                new,
                ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if old.primary_key != new.primary_key {
                    return Err(ReplayError::PrimaryKeyMutation(table_name.clone()));
                }
                if table.is_primary_key_column(&old.name) && new.nullable {
                    return Err(ReplayError::PrimaryKeyMutation(table_name.clone()));
                }
                let col = table
                    .columns
                    .iter_mut()
                    .find(|c| c.name == old.name)
                    .ok_or_else(|| ReplayError::ColumnNotFound {
                        table: table_name.clone(),
                        column: old.name.clone(),
                    })?;
                *col = new.clone();
                normalize_table_primary_key(table);
            }

            Operation::AddForeignKey {
                table_name,
                foreign_key,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if table
                    .foreign_keys
                    .iter()
                    .any(|fk| fk.name == foreign_key.name)
                {
                    return Err(ReplayError::ForeignKeyAlreadyExists {
                        table: table_name.clone(),
                        fk: foreign_key.name.clone(),
                    });
                }
                table.foreign_keys.push(foreign_key.clone());
            }

            Operation::DropForeignKey {
                table_name,
                foreign_key,
                ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let pos = table
                    .foreign_keys
                    .iter()
                    .position(|fk| fk.name == foreign_key.name)
                    .ok_or_else(|| ReplayError::ForeignKeyNotFound {
                        table: table_name.clone(),
                        fk: foreign_key.name.clone(),
                    })?;
                table.foreign_keys.remove(pos);
            }

            Operation::AddIndex {
                table_name, index, ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if table.indexes.iter().any(|i| i.name == index.name) {
                    return Err(ReplayError::IndexAlreadyExists {
                        table: table_name.clone(),
                        index: index.name.clone(),
                    });
                }
                table.indexes.push(index.clone());
            }

            Operation::DropIndex {
                table_name, index, ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let pos = table
                    .indexes
                    .iter()
                    .position(|i| i.name == index.name)
                    .ok_or_else(|| ReplayError::IndexNotFound {
                        table: table_name.clone(),
                        index: index.name.clone(),
                    })?;
                table.indexes.remove(pos);
            }

            Operation::AddConstraint {
                table_name,
                constraint,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                if table
                    .constraints
                    .iter()
                    .any(|c| c.name() == constraint.name())
                {
                    return Err(ReplayError::ConstraintAlreadyExists {
                        table: table_name.clone(),
                        constraint: constraint.name().to_string(),
                    });
                }
                table.constraints.push(constraint.clone());
                normalize_table_primary_key(table);
            }

            Operation::DropConstraint {
                table_name,
                constraint,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let pos = table
                    .constraints
                    .iter()
                    .position(|c| c.name() == constraint.name())
                    .ok_or_else(|| ReplayError::ConstraintNotFound {
                        table: table_name.clone(),
                        constraint: constraint.name().to_string(),
                    })?;
                table.constraints.remove(pos);
                normalize_table_primary_key(table);
            }

            Operation::Statement { .. } => {}

            Operation::CreateFunction { function } => {
                let key = function.qualified_name();
                if self.functions.contains_key(&key) {
                    return Err(ReplayError::FunctionAlreadyExists(key));
                }
                self.functions.insert(key, function.clone());
            }

            Operation::DropFunction { function } => {
                let key = function.qualified_name();
                if self.functions.remove(&key).is_none() {
                    return Err(ReplayError::FunctionNotFound(key));
                }
            }

            Operation::AlterFunction { old, new } => {
                let old_key = old.qualified_name();
                if self.functions.remove(&old_key).is_none() {
                    return Err(ReplayError::FunctionNotFound(old_key));
                }
                let new_key = new.qualified_name();
                self.functions.insert(new_key, new.clone());
            }

            Operation::CreateTrigger {
                table_name,
                trigger,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let tname = trigger.name.as_deref().unwrap_or("");
                if table
                    .triggers
                    .iter()
                    .any(|t| t.name.as_deref() == Some(tname))
                {
                    return Err(ReplayError::TriggerAlreadyExists {
                        table: table_name.clone(),
                        trigger: tname.to_string(),
                    });
                }
                table.triggers.push(trigger.clone());
            }

            Operation::DropTrigger {
                table_name,
                trigger,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let tname = trigger.name.as_deref().unwrap_or("");
                let pos = table
                    .triggers
                    .iter()
                    .position(|t| t.name.as_deref() == Some(tname))
                    .ok_or_else(|| ReplayError::TriggerNotFound {
                        table: table_name.clone(),
                        trigger: tname.to_string(),
                    })?;
                table.triggers.remove(pos);
            }

            Operation::AlterTrigger {
                table_name,
                old,
                new,
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                let tname = old.name.as_deref().unwrap_or("");
                let pos = table
                    .triggers
                    .iter()
                    .position(|t| t.name.as_deref() == Some(tname))
                    .ok_or_else(|| ReplayError::TriggerNotFound {
                        table: table_name.clone(),
                        trigger: tname.to_string(),
                    })?;
                table.triggers[pos] = new.clone();
            }

            Operation::CreateView { view } => {
                let key = view.qualified_name();
                if self.views.contains_key(&key) {
                    return Err(ReplayError::ViewAlreadyExists(key));
                }
                self.views.insert(key, view.clone());
            }

            Operation::DropView { view } => {
                let key = view.qualified_name();
                if self.views.remove(&key).is_none() {
                    return Err(ReplayError::ViewNotFound(key));
                }
            }

            Operation::ReplaceView { old, new } => {
                let old_key = old.qualified_name();
                if self.views.remove(&old_key).is_none() {
                    return Err(ReplayError::ViewNotFound(old_key));
                }
                let new_key = new.qualified_name();
                self.views.insert(new_key, new.clone());
            }

            Operation::CreateExtension { extension } => {
                let key = extension.qualified_name();
                if self.extensions.contains_key(&key) {
                    return Err(ReplayError::ExtensionAlreadyExists(key));
                }
                self.extensions.insert(key, extension.clone());
            }

            Operation::DropExtension { extension } => {
                let key = extension.qualified_name();
                if self.extensions.remove(&key).is_none() {
                    return Err(ReplayError::ExtensionNotFound(key));
                }
            }

            Operation::CreateEnum { enum_def } => {
                let key = enum_def.qualified_name();
                if self.enums.contains_key(&key) {
                    return Err(ReplayError::EnumAlreadyExists(key));
                }
                self.enums.insert(key, enum_def.clone());
            }

            Operation::DropEnum { enum_def } => {
                let key = enum_def.qualified_name();
                if self.enums.remove(&key).is_none() {
                    return Err(ReplayError::EnumNotFound(key));
                }
            }

            Operation::RenameEnumValue {
                enum_name,
                schema,
                old_value,
                new_value,
            } => {
                let key = schema_qualified_key(enum_name, schema.as_deref());
                let enum_def = self
                    .enums
                    .get_mut(&key)
                    .ok_or_else(|| ReplayError::EnumNotFound(key.clone()))?;
                let value = enum_def
                    .values
                    .iter_mut()
                    .find(|value| *value == old_value)
                    .ok_or_else(|| ReplayError::EnumNotFound(format!("{key}.{old_value}")))?;
                *value = new_value.clone();
            }

            Operation::AlterEnum { old, new } => {
                let old_key = old.qualified_name();
                if self.enums.remove(&old_key).is_none() {
                    return Err(ReplayError::EnumNotFound(old_key));
                }
                let new_key = new.qualified_name();
                self.enums.insert(new_key, new.clone());
            }
        }
        Ok(())
    }
}

fn normalize_table_primary_key(table: &mut Table) {
    let flagged: Vec<String> = table
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.clone())
        .collect();

    if table.primary_key.is_none() && !flagged.is_empty() {
        table.primary_key = Some(PrimaryKey {
            name: table.pk_constraint_name(),
            columns: flagged.clone(),
        });
    }

    let Some(pk) = &table.primary_key else {
        return;
    };

    if !flagged.is_empty() && !same_string_set(&flagged, &pk.columns) {
        return;
    }

    let pk_columns = pk.columns.clone();
    for column in &mut table.columns {
        column.primary_key = pk_columns.iter().any(|name| name == &column.name);
        if column.primary_key {
            column.nullable = false;
        }
    }
}

fn validate_table_references(
    schema: &Schema,
    table_name: &str,
    table: &Table,
) -> Result<(), SchemaValidationError> {
    let column_names: HashSet<&str> = table
        .columns
        .iter()
        .map(|column| column.name.as_str())
        .collect();

    let mut index_names = HashSet::new();
    for index in &table.indexes {
        if !index_names.insert(index.name.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate index '{}'",
                index.name
            )));
        }
        for column in &index.columns {
            if !column_names.contains(column.as_str()) {
                return Err(SchemaValidationError::Invalid(format!(
                    "table {table_name} index {}: unknown column '{column}'",
                    index.name
                )));
            }
        }
    }

    let mut constraint_names = HashSet::new();
    for constraint in &table.constraints {
        if !constraint_names.insert(constraint.name()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate constraint '{}'",
                constraint.name()
            )));
        }
        if let Constraint::Unique { name, columns } = constraint {
            for column in columns {
                if !column_names.contains(column.as_str()) {
                    return Err(SchemaValidationError::Invalid(format!(
                        "table {table_name} constraint {name}: unknown column '{column}'"
                    )));
                }
            }
        }
    }

    let mut fk_names = HashSet::new();
    for fk in &table.foreign_keys {
        if !fk_names.insert(fk.name.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate foreign key '{}'",
                fk.name
            )));
        }
        if !column_names.contains(fk.from_column.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: unknown source column '{}'",
                fk.name, fk.from_column
            )));
        }
        let Some((_, target)) = table_by_reference(schema, &fk.to_table) else {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: referenced table {} not found",
                fk.name, fk.to_table
            )));
        };
        if !target
            .columns
            .iter()
            .any(|column| column.name == fk.to_column)
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table {table_name} foreign key {}: referenced column '{}.{}' not found",
                fk.name, fk.to_table, fk.to_column
            )));
        }
    }

    let mut trigger_names = HashSet::new();
    for trigger in &table.triggers {
        if let Some(name) = &trigger.name
            && !trigger_names.insert(name.as_str())
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' has duplicate trigger '{name}'"
            )));
        }
    }

    Ok(())
}

fn table_by_reference<'a>(schema: &'a Schema, reference: &str) -> Option<(&'a String, &'a Table)> {
    if let Some(table) = schema.tables.get_key_value(reference) {
        return Some(table);
    }

    let mut matches = schema
        .tables
        .iter()
        .filter(|(_, table)| table.name == reference);
    let found = matches.next()?;
    matches.next().is_none().then_some(found)
}

fn validate_table_primary_key(
    table_name: &str,
    table: &Table,
) -> Result<(), SchemaValidationError> {
    let flagged: Vec<&str> = table
        .columns
        .iter()
        .filter(|column| column.primary_key)
        .map(|column| column.name.as_str())
        .collect();

    let Some(pk) = &table.primary_key else {
        return Ok(());
    };

    if pk.name.is_empty() {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' has a primary key with an empty name"
        )));
    }
    if pk.columns.is_empty() {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' has a primary key with no columns"
        )));
    }

    let mut pk_seen = HashSet::new();
    for column in &pk.columns {
        if !pk_seen.insert(column.as_str()) {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key '{}' repeats column '{column}'",
                pk.name
            )));
        }
        if !table
            .columns
            .iter()
            .any(|candidate| candidate.name == *column)
        {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key '{}' references unknown column '{column}'",
                pk.name
            )));
        }
    }

    if !flagged.is_empty()
        && !same_str_set(
            &flagged,
            &pk.columns.iter().map(String::as_str).collect::<Vec<_>>(),
        )
    {
        return Err(SchemaValidationError::Invalid(format!(
            "table '{table_name}' primary key column flags conflict with explicit primary_key '{}'",
            pk.name
        )));
    }

    for column in table.primary_key_columns() {
        if column.nullable {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{table_name}' primary key column '{}' must be non-null",
                column.name
            )));
        }
    }

    Ok(())
}

fn same_string_set(left: &[String], right: &[String]) -> bool {
    let left: HashSet<&str> = left.iter().map(String::as_str).collect();
    let right: HashSet<&str> = right.iter().map(String::as_str).collect();
    left == right
}

fn same_str_set(left: &[&str], right: &[&str]) -> bool {
    let left: HashSet<&str> = left.iter().copied().collect();
    let right: HashSet<&str> = right.iter().copied().collect();
    left == right
}
