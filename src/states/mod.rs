use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::dialects::Dialect;
use crate::operations::Operation;

pub mod errors;
pub mod types;
pub mod builder;

pub use errors::*;
pub use types::*;
pub use builder::*;

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
            for col in table.columns.iter_mut() {
                if let Some(r) = col.references.take() {
                    let fk_name = r.name.unwrap_or_else(|| {
                        format!("{}_{}_fkey", table_name, col.name)
                    });
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
                    let mut event_parts: Vec<&str> = trigger.events.iter().map(|e| match e {
                        TriggerEvent::Insert => "insert",
                        TriggerEvent::Update => "update",
                        TriggerEvent::Delete => "delete",
                        TriggerEvent::Truncate => "truncate",
                    }).collect();
                    event_parts.sort_unstable();
                    let timing_part = match trigger.timing {
                        TriggerTiming::Before => "before",
                        TriggerTiming::After => "after",
                        TriggerTiming::InsteadOf => "instead_of",
                    };
                    trigger.name = Some(format!("{}_{}_{}_trg", table_name, event_parts.join("_"), timing_part));
                }
                if let Some(body) = trigger.body.take() {
                    let trigger_name = trigger.name.as_deref().unwrap();
                    let fn_name = format!("{}_fn", trigger_name);
                    let lang = trigger.language.take().unwrap_or_else(|| "plpgsql".to_string());
                    trigger.function_name = Some(fn_name.clone());
                    new_functions.push((fn_name.clone(), FunctionDef {
                        name: fn_name,
                        schema: None,
                        arguments: String::new(),
                        returns: "trigger".to_string(),
                        language: lang,
                        body,
                        volatility: Volatility::Volatile,
                        security_definer: false,
                    }));
                }
            }
        }
        for (key, func) in self.functions.iter_mut() {
            func.name = key.clone();
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

    pub fn from_sql_str(s: &str) -> Result<Self, SchemaLoadError> {
        Ok(crate::sql::parse_sql(s)?)
    }

    pub fn from_sql_file(path: &std::path::Path) -> Result<Self, SchemaLoadError> {
        Ok(crate::sql::parse_sql_file(path)?)
    }

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

    pub fn validate(&self) -> Result<(), String> {
        for (name, table) in &self.tables {
            if table.name.is_empty() {
                return Err(format!("table with key '{name}' has an empty name — omit `name:` to inherit the key, or set it explicitly"));
            }
            let pk_count = table.columns.iter().filter(|c| c.primary_key).count();
            if pk_count > 1 {
                return Err(format!(
                    "table '{name}' has {pk_count} primary key columns; only one is allowed"
                ));
            }
            let mut seen = std::collections::HashSet::new();
            for col in &table.columns {
                if !seen.insert(col.name.as_str()) {
                    return Err(format!(
                        "table '{name}' has duplicate column '{}'",
                        col.name
                    ));
                }
            }
            for trigger in &table.triggers {
                if trigger.events.is_empty() {
                    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
                    return Err(format!(
                        "trigger '{tname}' on table '{name}' has no events"
                    ));
                }
                if trigger.function_name.is_none() {
                    let tname = trigger.name.as_deref().unwrap_or("<unnamed>");
                    return Err(format!(
                        "trigger '{tname}' on table '{name}' has no function_name (add `function_name` or inline `body`)"
                    ));
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
                let normalized = dialect.normalize_type(&col.col_type);
                if normalized != col.col_type {
                    col.col_type = normalized.to_string();
                }
            }
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

        fn rekey<T>(
            map: &mut BTreeMap<String, T>,
            key_fn: impl Fn(&T) -> String,
        ) {
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
    /// `Statement` and `Invoke` are no-ops: they carry raw SQL/code that cannot
    /// be reflected into the in-memory schema model.
    pub fn apply(&mut self, op: &Operation) -> Result<(), ReplayError> {
        match op {
            Operation::CreateTable { table } => {
                let key = table.qualified_name();
                if self.tables.contains_key(&key) {
                    return Err(ReplayError::TableAlreadyExists(key));
                }
                if table.columns.iter().filter(|c| c.primary_key).count() > 1 {
                    return Err(ReplayError::MultiplePrimaryKeys(key));
                }
                self.tables.insert(key, table.clone());
            }

            Operation::DropTable { table } => {
                let key = table.qualified_name();
                if self.tables.remove(&key).is_none() {
                    return Err(ReplayError::TableNotFound(key));
                }
            }

            Operation::RenameTable { old_name, new_name } => {
                let table = self.tables.remove(old_name).ok_or_else(|| {
                    ReplayError::TableNotFound(old_name.clone())
                })?;
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
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                if table.columns.iter().any(|c| c.name == column.name) {
                    return Err(ReplayError::ColumnAlreadyExists {
                        table: table_name.clone(),
                        column: column.name.clone(),
                    });
                }
                if column.primary_key && table.columns.iter().any(|c| c.primary_key) {
                    return Err(ReplayError::MultiplePrimaryKeys(table_name.clone()));
                }
                table.columns.push(column.clone());
            }

            Operation::DropColumn { table_name, column, .. } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let pos = table.columns.iter().position(|c| c.name == column.name).ok_or_else(|| {
                    ReplayError::ColumnNotFound { table: table_name.clone(), column: column.name.clone() }
                })?;
                table.columns.remove(pos);
            }

            Operation::RenameColumn { table_name, old_name, new_name } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let col = table.columns.iter_mut().find(|c| &c.name == old_name).ok_or_else(|| {
                    ReplayError::ColumnNotFound { table: table_name.clone(), column: old_name.clone() }
                })?;
                col.name = new_name.clone();
            }

            Operation::AlterColumn { table_name, old, new, .. } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                if new.primary_key && !old.primary_key
                    && table.columns.iter().filter(|c| c.name != old.name).any(|c| c.primary_key)
                {
                    return Err(ReplayError::MultiplePrimaryKeys(table_name.clone()));
                }
                let col = table.columns.iter_mut().find(|c| c.name == old.name).ok_or_else(|| {
                    ReplayError::ColumnNotFound { table: table_name.clone(), column: old.name.clone() }
                })?;
                *col = new.clone();
            }

            Operation::AddForeignKey { table_name, foreign_key } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                if table.foreign_keys.iter().any(|fk| fk.name == foreign_key.name) {
                    return Err(ReplayError::ForeignKeyAlreadyExists {
                        table: table_name.clone(),
                        fk: foreign_key.name.clone(),
                    });
                }
                table.foreign_keys.push(foreign_key.clone());
            }

            Operation::DropForeignKey { table_name, foreign_key, .. } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let pos = table.foreign_keys.iter().position(|fk| fk.name == foreign_key.name).ok_or_else(|| {
                    ReplayError::ForeignKeyNotFound { table: table_name.clone(), fk: foreign_key.name.clone() }
                })?;
                table.foreign_keys.remove(pos);
            }

            Operation::AddIndex { table_name, index, .. } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                if table.indexes.iter().any(|i| i.name == index.name) {
                    return Err(ReplayError::IndexAlreadyExists {
                        table: table_name.clone(),
                        index: index.name.clone(),
                    });
                }
                table.indexes.push(index.clone());
            }

            Operation::DropIndex { table_name, index, .. } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let pos = table.indexes.iter().position(|i| i.name == index.name).ok_or_else(|| {
                    ReplayError::IndexNotFound { table: table_name.clone(), index: index.name.clone() }
                })?;
                table.indexes.remove(pos);
            }

            Operation::AddConstraint { table_name, constraint } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                if table.constraints.iter().any(|c| c.name() == constraint.name()) {
                    return Err(ReplayError::ConstraintAlreadyExists {
                        table: table_name.clone(),
                        constraint: constraint.name().to_string(),
                    });
                }
                table.constraints.push(constraint.clone());
            }

            Operation::DropConstraint { table_name, constraint } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let pos = table.constraints.iter().position(|c| c.name() == constraint.name()).ok_or_else(|| {
                    ReplayError::ConstraintNotFound { table: table_name.clone(), constraint: constraint.name().to_string() }
                })?;
                table.constraints.remove(pos);
            }

            Operation::Statement { .. } | Operation::Invoke { .. } => {}

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

            Operation::CreateTrigger { table_name, trigger } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let tname = trigger.name.as_deref().unwrap_or("");
                if table.triggers.iter().any(|t| t.name.as_deref() == Some(tname)) {
                    return Err(ReplayError::TriggerAlreadyExists {
                        table: table_name.clone(),
                        trigger: tname.to_string(),
                    });
                }
                table.triggers.push(trigger.clone());
            }

            Operation::DropTrigger { table_name, trigger } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let tname = trigger.name.as_deref().unwrap_or("");
                let pos = table.triggers.iter().position(|t| t.name.as_deref() == Some(tname)).ok_or_else(|| {
                    ReplayError::TriggerNotFound { table: table_name.clone(), trigger: tname.to_string() }
                })?;
                table.triggers.remove(pos);
            }

            Operation::AlterTrigger { table_name, old, new } => {
                let table = self.tables.get_mut(table_name).ok_or_else(|| {
                    ReplayError::TableNotFound(table_name.clone())
                })?;
                let tname = old.name.as_deref().unwrap_or("");
                let pos = table.triggers.iter().position(|t| t.name.as_deref() == Some(tname)).ok_or_else(|| {
                    ReplayError::TriggerNotFound { table: table_name.clone(), trigger: tname.to_string() }
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
