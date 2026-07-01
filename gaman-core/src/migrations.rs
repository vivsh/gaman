use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::operations::Operation;
use crate::states::types::EntityKind;

fn bool_true() -> bool {
    true
}

/// A single migration: an ordered, dependency-aware set of operations.
/// `id` is derived from the filename at load time and is not stored in the file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Migration {
    #[serde(skip)]
    pub id: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
    pub operations: Vec<Operation>,
    /// When false, this migration runs outside a transaction. Required for
    /// operations that cannot run inside a transaction (e.g. CREATE INDEX CONCURRENTLY).
    /// Defaults to true so existing migration files are unaffected.
    #[serde(default = "bool_true", skip_serializing_if = "std::ops::Not::not")]
    pub atomic: bool,
}

impl Migration {
    pub fn from_yaml_str(s: &str) -> Result<Self, serde_yaml::Error> {
        serde_yaml::from_str(s)
    }

    pub fn to_yaml_string(&self) -> Result<String, serde_yaml::Error> {
        serde_yaml::to_string(self)
    }

    /// Returns the set of top-level entities touched by this migration.
    /// Includes FK target tables so callers can detect cross-namespace references.
    pub fn get_entities(&self) -> HashSet<(EntityKind, String)> {
        let mut out = HashSet::new();
        for op in &self.operations {
            match op {
                Operation::CreateTable { table } | Operation::DropTable { table } => {
                    out.insert((EntityKind::Table, table.qualified_name()));
                    for fk in &table.foreign_keys {
                        out.insert((EntityKind::Table, fk.to_table.clone()));
                    }
                }
                Operation::AddForeignKey {
                    table_name,
                    foreign_key,
                }
                | Operation::DropForeignKey {
                    table_name,
                    foreign_key,
                    ..
                } => {
                    out.insert((EntityKind::Table, table_name.clone()));
                    out.insert((EntityKind::Table, foreign_key.to_table.clone()));
                }
                Operation::CreateEnum { enum_def }
                | Operation::DropEnum { enum_def }
                | Operation::AlterEnum { new: enum_def, .. } => {
                    out.insert((EntityKind::Enum, enum_def.qualified_name()));
                }
                Operation::RenameEnumValue {
                    enum_name, schema, ..
                } => {
                    out.insert((
                        EntityKind::Enum,
                        crate::states::schema_qualified_key(enum_name, schema.as_deref()),
                    ));
                }
                Operation::CreateFunction { function }
                | Operation::DropFunction { function }
                | Operation::AlterFunction { new: function, .. } => {
                    out.insert((EntityKind::Function, function.qualified_name()));
                }
                Operation::CreateView { view }
                | Operation::DropView { view }
                | Operation::ReplaceView { new: view, .. } => {
                    out.insert((EntityKind::View, view.qualified_name()));
                }
                Operation::CreateExtension { extension }
                | Operation::DropExtension { extension } => {
                    out.insert((EntityKind::Extension, extension.qualified_name()));
                }
                Operation::AddColumn { table_name, .. }
                | Operation::DropColumn { table_name, .. }
                | Operation::AlterColumn { table_name, .. }
                | Operation::RenameColumn { table_name, .. }
                | Operation::AddIndex { table_name, .. }
                | Operation::DropIndex { table_name, .. }
                | Operation::AddConstraint { table_name, .. }
                | Operation::DropConstraint { table_name, .. }
                | Operation::CreateTrigger { table_name, .. }
                | Operation::AlterTrigger { table_name, .. }
                | Operation::DropTrigger { table_name, .. } => {
                    out.insert((EntityKind::Table, table_name.clone()));
                }
                Operation::RenameTable { old_name, .. } => {
                    out.insert((EntityKind::Table, old_name.clone()));
                }
                Operation::Statement { .. } => {}
            }
        }
        out
    }
}
