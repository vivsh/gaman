use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::operations::Operation;

#[derive(Debug, Error, PartialEq)]
pub enum ReplayError {
    #[error("table '{0}' already exists")]
    TableAlreadyExists(String),
    #[error("table '{0}' not found")]
    TableNotFound(String),
    #[error("table '{new}' already exists, cannot rename '{old}' to it")]
    RenameTargetExists { old: String, new: String },
    #[error("column '{column}' already exists on table '{table}'")]
    ColumnAlreadyExists { table: String, column: String },
    #[error("column '{column}' not found on table '{table}'")]
    ColumnNotFound { table: String, column: String },
    #[error("foreign key '{fk}' already exists on table '{table}'")]
    ForeignKeyAlreadyExists { table: String, fk: String },
    #[error("foreign key '{fk}' not found on table '{table}'")]
    ForeignKeyNotFound { table: String, fk: String },
    #[error("index '{index}' already exists on table '{table}'")]
    IndexAlreadyExists { table: String, index: String },
    #[error("index '{index}' not found on table '{table}'")]
    IndexNotFound { table: String, index: String },
    #[error("constraint '{constraint}' already exists on table '{table}'")]
    ConstraintAlreadyExists { table: String, constraint: String },
    #[error("constraint '{constraint}' not found on table '{table}'")]
    ConstraintNotFound { table: String, constraint: String },
    #[error("table '{0}' has multiple primary key columns")]
    MultiplePrimaryKeys(String),
    #[error("function '{0}' already exists")]
    FunctionAlreadyExists(String),
    #[error("function '{0}' not found")]
    FunctionNotFound(String),
    #[error("trigger '{trigger}' already exists on table '{table}'")]
    TriggerAlreadyExists { table: String, trigger: String },
    #[error("trigger '{trigger}' not found on table '{table}'")]
    TriggerNotFound { table: String, trigger: String },
    #[error("view '{0}' already exists")]
    ViewAlreadyExists(String),
    #[error("view '{0}' not found")]
    ViewNotFound(String),
}

#[derive(Debug, Error)]
pub enum SchemaLoadError {
    #[error("cannot read '{0}': {1}")]
    Io(String, #[source] std::io::Error),
    #[error("invalid YAML: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("invalid JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("table '{table}' defined in both '{a}' and '{b}'")]
    Merge { table: String, a: String, b: String },
}

/// Volatility classification for a function. Volatile is the default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Volatility {
    #[default]
    Volatile,
    Stable,
    Immutable,
}

/// A stored function or procedure definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FunctionDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub arguments: String,
    pub returns: String,
    pub language: String,
    pub body: String,
    #[serde(default, skip_serializing_if = "crate::states::is_volatile")]
    pub volatility: Volatility,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub security_definer: bool,
}

pub fn is_volatile(v: &Volatility) -> bool {
    *v == Volatility::Volatile
}

pub fn schema_qualified_key(name: &str, schema: Option<&str>) -> String {
    match schema {
        None | Some("public") => name.to_string(),
        Some(s) => format!("{s}.{name}"),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerTiming {
    Before,
    After,
    InsteadOf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum TriggerEvent {
    Delete,
    Insert,
    Truncate,
    Update,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerScope {
    Row,
    Statement,
}

/// A trigger attached to a table.
/// `body` and `language` are inline sugar: `normalize()` converts them into a
/// synthetic `FunctionDef` and sets `function_name`, then clears both fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TriggerDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    pub timing: TriggerTiming,
    pub events: Vec<TriggerEvent>,
    pub scope: TriggerScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub when: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
}

/// A database view definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ViewDef {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub definition: String,
}

/// Complete normalized schema state — a snapshot of all tables at a point in time.
/// Uses `BTreeMap` to guarantee deterministic ordering.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct SchemaState {
    pub tables: BTreeMap<String, Table>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub views: BTreeMap<String, ViewDef>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub functions: BTreeMap<String, FunctionDef>,
}

/// A single table definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Table {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<String>,
    pub columns: Vec<Column>,
    pub foreign_keys: Vec<ForeignKey>,
    pub indexes: Vec<Index>,
    pub constraints: Vec<Constraint>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triggers: Vec<TriggerDef>,
}

impl Table {
    pub fn pk_constraint_name(&self) -> String {
        format!("{}_pkey", self.name)
    }

    pub fn pk_constraint_name_for(table_name: &str) -> String {
        format!("{}_pkey", table_name)
    }
}

/// An index on one or more columns.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Index {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub predicate: Option<String>,
}

/// A named table constraint (unique, check).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Constraint {
    Unique { name: String, columns: Vec<String> },
    Check { name: String, expression: String },
}

impl Constraint {
    pub fn name(&self) -> &str {
        match self {
            Constraint::Unique { name, .. } => name,
            Constraint::Check { name, .. } => name,
        }
    }
}

/// Inline foreign-key reference declared on a column.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ColumnRef {
    pub table: String,
    pub column: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// A single column definition.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub col_type: String,
    pub nullable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default)]
    pub primary_key: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub references: Option<ColumnRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check: Option<String>,
}

/// A foreign key constraint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ForeignKey {
    pub name: String,
    pub from_column: String,
    pub to_table: String,
    pub to_column: String,
}

impl SchemaState {
    /// Folds inline `references` and `check` fields declared on columns into the
    /// table-level `foreign_keys` and `constraints` vecs. Call this once after
    /// deserializing a user-authored schema file before passing it to diff or validate.
    pub fn normalize(&mut self) {
        let mut new_functions: Vec<(String, FunctionDef)> = Vec::new();
        for (table_name, table) in self.tables.iter_mut() {
            let table_name = table_name.clone();
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

    pub fn load(path: &std::path::Path) -> Result<Self, SchemaLoadError> {
        if path.is_dir() { Self::from_yaml_dir(path) } else { Self::from_yaml_file(path) }
    }

    pub fn validate(&self) -> Result<(), String> {
        for (name, table) in &self.tables {
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

    /// Apply a single operation to this state, mutating it in place.
    /// `Statement` and `Invoke` are no-ops: they carry raw SQL/code that cannot
    /// be reflected into the in-memory schema model.
    pub fn apply(&mut self, op: &Operation) -> Result<(), ReplayError> {
        match op {
            Operation::CreateTable { table } => {
                if self.tables.contains_key(&table.name) {
                    return Err(ReplayError::TableAlreadyExists(table.name.clone()));
                }
                if table.columns.iter().filter(|c| c.primary_key).count() > 1 {
                    return Err(ReplayError::MultiplePrimaryKeys(table.name.clone()));
                }
                self.tables.insert(table.name.clone(), table.clone());
            }

            Operation::DropTable { table } => {
                if self.tables.remove(&table.name).is_none() {
                    return Err(ReplayError::TableNotFound(table.name.clone()));
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

            Operation::AddIndex { table_name, index } => {
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

            Operation::DropIndex { table_name, index } => {
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

            // Raw SQL and invocations cannot be reflected into the in-memory model.
            Operation::Statement { .. } | Operation::Invoke { .. } => {}

            Operation::CreateFunction { function } => {
                if self.functions.contains_key(&function.name) {
                    return Err(ReplayError::FunctionAlreadyExists(function.name.clone()));
                }
                self.functions.insert(function.name.clone(), function.clone());
            }

            Operation::DropFunction { function } => {
                if self.functions.remove(&function.name).is_none() {
                    return Err(ReplayError::FunctionNotFound(function.name.clone()));
                }
            }

            Operation::AlterFunction { old, new } => {
                if self.functions.remove(&old.name).is_none() {
                    return Err(ReplayError::FunctionNotFound(old.name.clone()));
                }
                self.functions.insert(new.name.clone(), new.clone());
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
                let key = schema_qualified_key(&view.name, view.schema.as_deref());
                if self.views.contains_key(&key) {
                    return Err(ReplayError::ViewAlreadyExists(key));
                }
                self.views.insert(key, view.clone());
            }

            Operation::DropView { view } => {
                let key = schema_qualified_key(&view.name, view.schema.as_deref());
                if self.views.remove(&key).is_none() {
                    return Err(ReplayError::ViewNotFound(key));
                }
            }

            Operation::ReplaceView { old, new } => {
                let old_key = schema_qualified_key(&old.name, old.schema.as_deref());
                if self.views.remove(&old_key).is_none() {
                    return Err(ReplayError::ViewNotFound(old_key));
                }
                let new_key = schema_qualified_key(&new.name, new.schema.as_deref());
                self.views.insert(new_key, new.clone());
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operations::Operation;

    fn basic_table(name: &str) -> Table {
        Table {
            name: name.to_string(),
            schema: None,
            columns: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        }
    }

    fn text_col(name: &str) -> Column {
        Column { name: name.to_string(), col_type: "text".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() }
    }

    fn apply_ok(state: &mut SchemaState, op: Operation) {
        state.apply(&op).expect("apply should succeed");
    }

    /// CreateTable inserts the table into the state.
    #[test]
    fn create_table() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        assert!(s.tables.contains_key("users"));
    }

    /// CreateTable on an existing table name returns TableAlreadyExists.
    #[test]
    fn create_table_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let err = s.apply(&Operation::CreateTable { table: basic_table("users") }).unwrap_err();
        assert_eq!(err, ReplayError::TableAlreadyExists("users".to_string()));
    }

    /// DropTable removes the table from state.
    #[test]
    fn drop_table() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::DropTable { table: basic_table("users") });
        assert!(!s.tables.contains_key("users"));
    }

    /// DropTable on a nonexistent table returns TableNotFound.
    #[test]
    fn drop_table_not_found() {
        let mut s = SchemaState::default();
        let err = s.apply(&Operation::DropTable { table: basic_table("ghost") }).unwrap_err();
        assert_eq!(err, ReplayError::TableNotFound("ghost".to_string()));
    }

    /// RenameTable moves the table to the new name and updates its `name` field.
    #[test]
    fn rename_table() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::RenameTable { old_name: "users".to_string(), new_name: "accounts".to_string() });
        assert!(!s.tables.contains_key("users"));
        assert_eq!(s.tables["accounts"].name, "accounts");
    }

    /// RenameTable errors if the target name already exists.
    #[test]
    fn rename_table_target_exists() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("accounts") });
        let err = s.apply(&Operation::RenameTable {
            old_name: "users".to_string(),
            new_name: "accounts".to_string(),
        }).unwrap_err();
        assert!(matches!(err, ReplayError::RenameTargetExists { .. }));
    }

    /// AddColumn appends the column to the table.
    #[test]
    fn add_column() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") });
        assert_eq!(s.tables["users"].columns[0].name, "email");
    }

    /// AddColumn on an existing column name returns ColumnAlreadyExists.
    #[test]
    fn add_column_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") });
        let err = s.apply(&Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") }).unwrap_err();
        assert!(matches!(err, ReplayError::ColumnAlreadyExists { .. }));
    }

    /// DropColumn removes the named column.
    #[test]
    fn drop_column() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") });
        apply_ok(&mut s, Operation::DropColumn { table_name: "users".to_string(), column: text_col("email"), cascade: false });
        assert!(s.tables["users"].columns.is_empty());
    }

    /// DropColumn on a missing column returns ColumnNotFound.
    #[test]
    fn drop_column_not_found() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let err = s.apply(&Operation::DropColumn { table_name: "users".to_string(), column: text_col("ghost"), cascade: false }).unwrap_err();
        assert!(matches!(err, ReplayError::ColumnNotFound { .. }));
    }

    /// RenameColumn changes the column's name in place.
    #[test]
    fn rename_column() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") });
        apply_ok(&mut s, Operation::RenameColumn {
            table_name: "users".to_string(),
            old_name: "email".to_string(),
            new_name: "email_address".to_string(),
        });
        assert_eq!(s.tables["users"].columns[0].name, "email_address");
    }

    /// AlterColumn replaces the column definition.
    #[test]
    fn alter_column() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("bio") });
        let new_col = Column { name: "bio".to_string(), col_type: "varchar(500)".to_string(), nullable: true, default: None, primary_key: false, ..Default::default() };
        apply_ok(&mut s, Operation::AlterColumn {
            table_name: "users".to_string(),
            old: text_col("bio"),
            new: new_col,
            cast_expr: None,
        });
        assert_eq!(s.tables["users"].columns[0].col_type, "varchar(500)");
        assert!(s.tables["users"].columns[0].nullable);
    }

    /// AddForeignKey attaches a FK to the table.
    #[test]
    fn add_foreign_key() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("posts") });
        let fk = ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        apply_ok(&mut s, Operation::AddForeignKey { table_name: "posts".to_string(), foreign_key: fk });
        assert_eq!(s.tables["posts"].foreign_keys[0].name, "fk_user");
    }

    /// AddForeignKey with a duplicate name returns ForeignKeyAlreadyExists.
    #[test]
    fn add_foreign_key_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("posts") });
        let fk = ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        apply_ok(&mut s, Operation::AddForeignKey { table_name: "posts".to_string(), foreign_key: fk.clone() });
        let err = s.apply(&Operation::AddForeignKey { table_name: "posts".to_string(), foreign_key: fk }).unwrap_err();
        assert!(matches!(err, ReplayError::ForeignKeyAlreadyExists { .. }));
    }

    /// DropForeignKey removes the FK from the table.
    #[test]
    fn drop_foreign_key() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("posts") });
        let fk = ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() };
        apply_ok(&mut s, Operation::AddForeignKey { table_name: "posts".to_string(), foreign_key: fk });
        apply_ok(&mut s, Operation::DropForeignKey { table_name: "posts".to_string(), foreign_key: ForeignKey { name: "fk_user".to_string(), from_column: "user_id".to_string(), to_table: "users".to_string(), to_column: "id".to_string() }, cascade: false });
        assert!(s.tables["posts"].foreign_keys.is_empty());
    }

    /// AddIndex attaches an index to the table.
    #[test]
    fn add_index() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let idx = Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None };
        apply_ok(&mut s, Operation::AddIndex { table_name: "users".to_string(), index: idx });
        assert_eq!(s.tables["users"].indexes[0].name, "idx_email");
    }

    /// AddIndex with a duplicate name returns IndexAlreadyExists.
    #[test]
    fn add_index_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let idx = Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None };
        apply_ok(&mut s, Operation::AddIndex { table_name: "users".to_string(), index: idx.clone() });
        let err = s.apply(&Operation::AddIndex { table_name: "users".to_string(), index: idx }).unwrap_err();
        assert!(matches!(err, ReplayError::IndexAlreadyExists { .. }));
    }

    /// DropIndex removes the index.
    #[test]
    fn drop_index() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let idx = Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None };
        apply_ok(&mut s, Operation::AddIndex { table_name: "users".to_string(), index: idx });
        apply_ok(&mut s, Operation::DropIndex { table_name: "users".to_string(), index: Index { name: "idx_email".to_string(), columns: vec!["email".to_string()], unique: true, predicate: None } });
        assert!(s.tables["users"].indexes.is_empty());
    }

    /// AddConstraint attaches a check constraint to the table.
    #[test]
    fn add_constraint() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let c = Constraint::Check { name: "chk_age".to_string(), expression: "age > 0".to_string() };
        apply_ok(&mut s, Operation::AddConstraint { table_name: "users".to_string(), constraint: c });
        assert_eq!(s.tables["users"].constraints[0].name(), "chk_age");
    }

    /// AddConstraint with a duplicate name returns ConstraintAlreadyExists.
    #[test]
    fn add_constraint_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let c = Constraint::Unique { name: "uq_email".to_string(), columns: vec!["email".to_string()] };
        apply_ok(&mut s, Operation::AddConstraint { table_name: "users".to_string(), constraint: c.clone() });
        let err = s.apply(&Operation::AddConstraint { table_name: "users".to_string(), constraint: c }).unwrap_err();
        assert!(matches!(err, ReplayError::ConstraintAlreadyExists { .. }));
    }

    /// DropConstraint removes the named constraint.
    #[test]
    fn drop_constraint() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let c = Constraint::Unique { name: "uq_email".to_string(), columns: vec!["email".to_string()] };
        apply_ok(&mut s, Operation::AddConstraint { table_name: "users".to_string(), constraint: c });
        apply_ok(&mut s, Operation::DropConstraint { table_name: "users".to_string(), constraint: Constraint::Unique { name: "uq_email".to_string(), columns: vec!["email".to_string()] } });
        assert!(s.tables["users"].constraints.is_empty());
    }

    /// Statement and Invoke are no-ops — state is unchanged.
    #[test]
    fn statement_and_invoke_are_noops() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::Statement { up: "SELECT 1".to_string(), down: None });
        apply_ok(&mut s, Operation::Invoke { up: "seed_data".to_string(), down: None });
        assert!(s.tables.is_empty());
    }

    /// Replaying a sequence of operations builds the expected state end-to-end.
    #[test]
    fn end_to_end_replay() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("email") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "users".to_string(), column: text_col("bio") });
        apply_ok(&mut s, Operation::DropColumn { table_name: "users".to_string(), column: text_col("bio"), cascade: false });
        apply_ok(&mut s, Operation::RenameColumn { table_name: "users".to_string(), old_name: "email".to_string(), new_name: "email_address".to_string() });
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("posts") });
        apply_ok(&mut s, Operation::AddColumn { table_name: "posts".to_string(), column: text_col("title") });

        assert_eq!(s.tables.len(), 2);
        assert_eq!(s.tables["users"].columns.len(), 1);
        assert_eq!(s.tables["users"].columns[0].name, "email_address");
        assert_eq!(s.tables["posts"].columns[0].name, "title");
    }

    /// Operations on a nonexistent table return TableNotFound.
    #[test]
    fn table_not_found_propagates() {
        let mut s = SchemaState::default();
        let err = s.apply(&Operation::AddColumn { table_name: "ghost".to_string(), column: text_col("x") }).unwrap_err();
        assert_eq!(err, ReplayError::TableNotFound("ghost".to_string()));
    }

    /// CreateTable with two primary_key columns returns MultiplePrimaryKeys.
    #[test]
    fn create_table_with_multiple_pk_returns_error() {
        let mut s = SchemaState::default();
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![
                Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
                Column { name: "alt_id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
            ],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        let err = s.apply(&Operation::CreateTable { table }).unwrap_err();
        assert_eq!(err, ReplayError::MultiplePrimaryKeys("users".to_string()));
    }

    /// AddColumn with primary_key=true when one already exists returns MultiplePrimaryKeys.
    #[test]
    fn add_pk_column_when_pk_exists_returns_error() {
        let mut s = SchemaState::default();
        let pk_col = Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() };
        let table = Table { name: "users".to_string(), schema: None, columns: vec![pk_col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        s.apply(&Operation::CreateTable { table }).unwrap();
        let second_pk = Column { name: "alt_id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() };
        let err = s.apply(&Operation::AddColumn { table_name: "users".to_string(), column: second_pk }).unwrap_err();
        assert_eq!(err, ReplayError::MultiplePrimaryKeys("users".to_string()));
    }

    /// AlterColumn that promotes a column to primary_key when one already exists returns MultiplePrimaryKeys.
    #[test]
    fn alter_column_to_pk_when_pk_exists_returns_error() {
        let mut s = SchemaState::default();
        let pk_col = Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() };
        let other_col = Column { name: "other".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: false, ..Default::default() };
        let table = Table { name: "users".to_string(), schema: None, columns: vec![pk_col, other_col.clone()], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        s.apply(&Operation::CreateTable { table }).unwrap();
        let promoted = Column { name: "other".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() };
        let err = s.apply(&Operation::AlterColumn {
            table_name: "users".to_string(),
            old: other_col,
            new: promoted,
            cast_expr: None,
        }).unwrap_err();
        assert_eq!(err, ReplayError::MultiplePrimaryKeys("users".to_string()));
    }

    /// validate() returns Ok for a state with at most one PK column per table.
    #[test]
    fn validate_single_pk_ok() {
        let mut s = SchemaState::default();
        let pk_col = Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() };
        let table = Table { name: "users".to_string(), schema: None, columns: vec![pk_col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        s.tables.insert("users".to_string(), table);
        assert!(s.validate().is_ok());
    }

    /// validate() returns Err when a table has more than one primary key column.
    #[test]
    fn validate_multiple_pk_returns_err() {
        let mut s = SchemaState::default();
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![
                Column { name: "id".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
                Column { name: "alt".to_string(), col_type: "bigint".to_string(), nullable: false, default: None, primary_key: true, ..Default::default() },
            ],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![],
        };
        s.tables.insert("users".to_string(), table);
        assert!(s.validate().is_err());
    }

    /// normalize() moves an inline `references` into `table.foreign_keys` and clears the column field.
    /// The FK name is auto-generated as `{table}_{column}_fkey` when no explicit name is given.
    #[test]
    fn normalize_moves_inline_fk_to_foreign_keys() {
        let col = Column {
            name: "user_id".to_string(),
            col_type: "bigint".to_string(),
            nullable: false,
            default: None,
            primary_key: false,
            references: Some(ColumnRef { table: "users".to_string(), column: "id".to_string(), name: None }),
            check: None,
        };
        let table = Table { name: "posts".to_string(), schema: None, columns: vec![col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        let mut s = SchemaState::default();
        s.tables.insert("posts".to_string(), table);
        s.normalize();
        let t = &s.tables["posts"];
        assert_eq!(t.foreign_keys.len(), 1);
        assert_eq!(t.foreign_keys[0].name, "posts_user_id_fkey");
        assert_eq!(t.foreign_keys[0].from_column, "user_id");
        assert_eq!(t.foreign_keys[0].to_table, "users");
        assert_eq!(t.foreign_keys[0].to_column, "id");
        assert!(t.columns[0].references.is_none());
    }

    /// normalize() uses the explicit `name` from ColumnRef instead of auto-generating when provided.
    #[test]
    fn normalize_inline_fk_uses_explicit_name_when_provided() {
        let col = Column {
            name: "user_id".to_string(),
            col_type: "bigint".to_string(),
            nullable: false,
            default: None,
            primary_key: false,
            references: Some(ColumnRef { table: "users".to_string(), column: "id".to_string(), name: Some("fk_posts_user".to_string()) }),
            check: None,
        };
        let table = Table { name: "posts".to_string(), schema: None, columns: vec![col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        let mut s = SchemaState::default();
        s.tables.insert("posts".to_string(), table);
        s.normalize();
        assert_eq!(s.tables["posts"].foreign_keys[0].name, "fk_posts_user");
    }

    /// normalize() moves an inline `check` into `table.constraints` and clears the column field.
    /// The constraint name is auto-generated as `{table}_{column}_check`.
    #[test]
    fn normalize_moves_inline_check_to_constraints() {
        let col = Column {
            name: "score".to_string(),
            col_type: "integer".to_string(),
            nullable: false,
            default: None,
            primary_key: false,
            references: None,
            check: Some("score >= 0".to_string()),
        };
        let table = Table { name: "results".to_string(), schema: None, columns: vec![col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        let mut s = SchemaState::default();
        s.tables.insert("results".to_string(), table);
        s.normalize();
        let t = &s.tables["results"];
        assert_eq!(t.constraints.len(), 1);
        assert!(matches!(&t.constraints[0], Constraint::Check { name, expression } if name == "results_score_check" && expression == "score >= 0"));
        assert!(t.columns[0].check.is_none());
    }

    /// normalize() is idempotent — running it twice produces the same result.
    #[test]
    fn normalize_is_idempotent() {
        let col = Column {
            name: "user_id".to_string(),
            col_type: "bigint".to_string(),
            nullable: false,
            default: None,
            primary_key: false,
            references: Some(ColumnRef { table: "users".to_string(), column: "id".to_string(), name: None }),
            check: None,
        };
        let table = Table { name: "posts".to_string(), schema: None, columns: vec![col], foreign_keys: vec![], indexes: vec![], constraints: vec![], triggers: vec![] };
        let mut s = SchemaState::default();
        s.tables.insert("posts".to_string(), table);
        s.normalize();
        s.normalize();
        assert_eq!(s.tables["posts"].foreign_keys.len(), 1);
    }

    /// Column.col_type serializes as the key "type" in YAML, not "col_type".
    #[test]
    fn col_type_serializes_as_type_in_yaml() {
        let col = Column {
            name: "id".to_string(),
            col_type: "bigint".to_string(),
            nullable: false,
            default: None,
            primary_key: true,
            references: None,
            check: None,
        };
        let yaml = serde_yaml::to_string(&col).expect("serialize");
        assert!(yaml.contains("type: bigint"), "expected 'type: bigint' in: {yaml}");
        assert!(!yaml.contains("col_type"), "col_type should not appear in: {yaml}");
        let back: Column = serde_yaml::from_str(&yaml).expect("deserialize");
        assert_eq!(back.col_type, "bigint");
    }

    fn basic_function(name: &str) -> FunctionDef {
        FunctionDef {
            name: name.to_string(),
            schema: None,
            arguments: String::new(),
            returns: "void".to_string(),
            language: "sql".to_string(),
            body: "SELECT 1".to_string(),
            volatility: Volatility::Volatile,
            security_definer: false,
        }
    }

    fn basic_trigger(name: &str) -> TriggerDef {
        TriggerDef {
            name: Some(name.to_string()),
            timing: TriggerTiming::After,
            events: vec![TriggerEvent::Insert],
            scope: TriggerScope::Row,
            function_name: Some("some_fn".to_string()),
            when: None,
            body: None,
            language: None,
        }
    }

    /// CreateFunction inserts the function into state.
    #[test]
    fn create_function() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateFunction { function: basic_function("notify") });
        assert!(s.functions.contains_key("notify"));
    }

    /// CreateFunction on an existing name returns FunctionAlreadyExists.
    #[test]
    fn create_function_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateFunction { function: basic_function("notify") });
        let err = s.apply(&Operation::CreateFunction { function: basic_function("notify") }).unwrap_err();
        assert_eq!(err, ReplayError::FunctionAlreadyExists("notify".to_string()));
    }

    /// DropFunction removes the function from state.
    #[test]
    fn drop_function() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateFunction { function: basic_function("notify") });
        apply_ok(&mut s, Operation::DropFunction { function: basic_function("notify") });
        assert!(!s.functions.contains_key("notify"));
    }

    /// DropFunction on a nonexistent function returns FunctionNotFound.
    #[test]
    fn drop_function_not_found() {
        let mut s = SchemaState::default();
        let err = s.apply(&Operation::DropFunction { function: basic_function("ghost") }).unwrap_err();
        assert_eq!(err, ReplayError::FunctionNotFound("ghost".to_string()));
    }

    /// AlterFunction replaces the old function entry with the new one.
    #[test]
    fn alter_function() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateFunction { function: basic_function("notify") });
        let mut updated = basic_function("notify");
        updated.body = "SELECT 2".to_string();
        apply_ok(&mut s, Operation::AlterFunction { old: basic_function("notify"), new: updated.clone() });
        assert_eq!(s.functions["notify"].body, "SELECT 2");
    }

    /// CreateTrigger attaches a trigger to the table.
    #[test]
    fn create_trigger() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        });
        assert_eq!(s.tables["users"].triggers.len(), 1);
    }

    /// CreateTrigger with a duplicate name returns TriggerAlreadyExists.
    #[test]
    fn create_trigger_duplicate() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        });
        let err = s.apply(&Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        }).unwrap_err();
        assert_eq!(err, ReplayError::TriggerAlreadyExists { table: "users".to_string(), trigger: "audit_trg".to_string() });
    }

    /// DropTrigger removes the trigger from the table.
    #[test]
    fn drop_trigger() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        apply_ok(&mut s, Operation::CreateTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        });
        apply_ok(&mut s, Operation::DropTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("audit_trg"),
        });
        assert!(s.tables["users"].triggers.is_empty());
    }

    /// DropTrigger on a nonexistent trigger returns TriggerNotFound.
    #[test]
    fn drop_trigger_not_found() {
        let mut s = SchemaState::default();
        apply_ok(&mut s, Operation::CreateTable { table: basic_table("users") });
        let err = s.apply(&Operation::DropTrigger {
            table_name: "users".to_string(),
            trigger: basic_trigger("ghost_trg"),
        }).unwrap_err();
        assert_eq!(err, ReplayError::TriggerNotFound { table: "users".to_string(), trigger: "ghost_trg".to_string() });
    }

    /// normalize() auto-generates a trigger name from table + events + timing.
    #[test]
    fn normalize_auto_generates_trigger_name() {
        let yaml = r#"
tables:
  users:
    name: users
    columns: []
    foreign_keys: []
    indexes: []
    constraints: []
    triggers:
      - timing: after
        events: [update, insert]
        scope: row
        function_name: some_fn
"#;
        let state = SchemaState::from_yaml_str(yaml).expect("parse");
        let trigger = &state.tables["users"].triggers[0];
        assert_eq!(trigger.name.as_deref(), Some("users_insert_update_after_trg"));
    }

    /// normalize() converts an inline trigger body into a FunctionDef and sets function_name.
    #[test]
    fn normalize_inline_trigger_body_to_function() {
        let yaml = r#"
tables:
  orders:
    name: orders
    columns: []
    foreign_keys: []
    indexes: []
    constraints: []
    triggers:
      - name: orders_insert_after_trg
        timing: after
        events: [insert]
        scope: row
        body: "BEGIN RETURN NEW; END;"
"#;
        let state = SchemaState::from_yaml_str(yaml).expect("parse");
        let trigger = &state.tables["orders"].triggers[0];
        assert_eq!(trigger.function_name.as_deref(), Some("orders_insert_after_trg_fn"));
        assert!(trigger.body.is_none(), "body should be cleared after normalize");
        let func = state.functions.get("orders_insert_after_trg_fn").expect("function should exist");
        assert_eq!(func.returns, "trigger");
        assert_eq!(func.language, "plpgsql");
        assert_eq!(func.body, "BEGIN RETURN NEW; END;");
    }

    /// validate() rejects a trigger with no events.
    #[test]
    fn validate_trigger_empty_events() {
        let mut s = SchemaState::default();
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![TriggerDef {
                name: Some("t".to_string()),
                timing: TriggerTiming::After,
                events: vec![],
                scope: TriggerScope::Row,
                function_name: Some("fn".to_string()),
                when: None,
                body: None,
                language: None,
            }],
        };
        s.tables.insert("users".to_string(), table);
        assert!(s.validate().unwrap_err().contains("no events"));
    }

    /// validate() rejects a trigger with no function_name after normalize.
    #[test]
    fn validate_trigger_no_function_name() {
        let mut s = SchemaState::default();
        let table = Table {
            name: "users".to_string(),
            schema: None,
            columns: vec![],
            foreign_keys: vec![],
            indexes: vec![],
            constraints: vec![],
            triggers: vec![TriggerDef {
                name: Some("t".to_string()),
                timing: TriggerTiming::After,
                events: vec![TriggerEvent::Insert],
                scope: TriggerScope::Row,
                function_name: None,
                when: None,
                body: None,
                language: None,
            }],
        };
        s.tables.insert("users".to_string(), table);
        assert!(s.validate().unwrap_err().contains("no function_name"));
    }
}
