use serde::{Deserialize, Serialize};

use crate::states::{Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, Table, TriggerDef, ViewDef};

/// All possible schema change operations.
/// Each variant carries the minimal data needed to describe the change.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Operation {
    CreateTable { table: Table },
    DropTable { table: Table },
    RenameTable { old_name: String, new_name: String },
    AddColumn { table_name: String, column: Column },
    DropColumn { table_name: String, column: Column, #[serde(default)] cascade: bool },
    RenameColumn { table_name: String, old_name: String, new_name: String },
    AlterColumn { table_name: String, old: Column, new: Column, #[serde(default)] cast_expr: Option<String> },
    AddForeignKey { table_name: String, foreign_key: ForeignKey },
    DropForeignKey { table_name: String, foreign_key: ForeignKey, #[serde(default)] cascade: bool },
    AddIndex { table_name: String, index: Index, #[serde(default)] concurrent: bool },
    DropIndex { table_name: String, index: Index, #[serde(default)] concurrent: bool },
    AddConstraint { table_name: String, constraint: Constraint },
    DropConstraint { table_name: String, constraint: Constraint },
    Statement { up: String, down: Option<String> },
    Invoke { up: String, down: Option<String> },
    CreateFunction { function: FunctionDef },
    AlterFunction { old: FunctionDef, new: FunctionDef },
    DropFunction { function: FunctionDef },
    CreateTrigger { table_name: String, trigger: TriggerDef },
    AlterTrigger { table_name: String, old: TriggerDef, new: TriggerDef },
    DropTrigger { table_name: String, trigger: TriggerDef },
    CreateView { view: ViewDef },
    DropView { view: ViewDef },
    ReplaceView { old: ViewDef, new: ViewDef },
    CreateExtension { extension: ExtensionDef },
    DropExtension { extension: ExtensionDef },
    CreateEnum { enum_def: EnumDef },
    DropEnum { enum_def: EnumDef },
    AlterEnum { old: EnumDef, new: EnumDef },
}

impl Operation {
    pub fn inverse(&self) -> Option<Operation> {
        match self {
            Self::CreateTable { table } => Some(Self::DropTable { table: table.clone() }),
            Self::DropTable { table } => Some(Self::CreateTable { table: table.clone() }),
            Self::RenameTable { old_name, new_name } => Some(Self::RenameTable {
                old_name: new_name.clone(),
                new_name: old_name.clone(),
            }),
            Self::AddColumn { table_name, column } => Some(Self::DropColumn {
                table_name: table_name.clone(),
                column: column.clone(),
                cascade: false,
            }),
            Self::DropColumn { table_name, column, .. } => Some(Self::AddColumn {
                table_name: table_name.clone(),
                column: column.clone(),
            }),
            Self::RenameColumn { table_name, old_name, new_name } => Some(Self::RenameColumn {
                table_name: table_name.clone(),
                old_name: new_name.clone(),
                new_name: old_name.clone(),
            }),
            Self::AlterColumn { table_name, old, new, .. } => Some(Self::AlterColumn {
                table_name: table_name.clone(),
                old: new.clone(),
                new: old.clone(),
                cast_expr: None,
            }),
            Self::AddForeignKey { table_name, foreign_key } => Some(Self::DropForeignKey {
                table_name: table_name.clone(),
                foreign_key: foreign_key.clone(),
                cascade: false,
            }),
            Self::DropForeignKey { table_name, foreign_key, .. } => Some(Self::AddForeignKey {
                table_name: table_name.clone(),
                foreign_key: foreign_key.clone(),
            }),
            Self::AddIndex { table_name, index, concurrent } => Some(Self::DropIndex {
                table_name: table_name.clone(),
                index: index.clone(),
                concurrent: *concurrent,
            }),
            Self::DropIndex { table_name, index, concurrent } => Some(Self::AddIndex {
                table_name: table_name.clone(),
                index: index.clone(),
                concurrent: *concurrent,
            }),
            Self::AddConstraint { table_name, constraint } => Some(Self::DropConstraint {
                table_name: table_name.clone(),
                constraint: constraint.clone(),
            }),
            Self::DropConstraint { table_name, constraint } => Some(Self::AddConstraint {
                table_name: table_name.clone(),
                constraint: constraint.clone(),
            }),
            Self::Statement { up, down: Some(d) } => Some(Self::Statement {
                up: d.clone(),
                down: Some(up.clone()),
            }),
            Self::Statement { down: None, .. } => None,
            Self::Invoke { up, down: Some(d) } => Some(Self::Invoke {
                up: d.clone(),
                down: Some(up.clone()),
            }),
            Self::Invoke { down: None, .. } => None,
            Self::CreateFunction { function } => Some(Self::DropFunction { function: function.clone() }),
            Self::DropFunction { function } => Some(Self::CreateFunction { function: function.clone() }),
            Self::AlterFunction { old, new } => Some(Self::AlterFunction { old: new.clone(), new: old.clone() }),
            Self::CreateTrigger { table_name, trigger } => Some(Self::DropTrigger {
                table_name: table_name.clone(),
                trigger: trigger.clone(),
            }),
            Self::DropTrigger { table_name, trigger } => Some(Self::CreateTrigger {
                table_name: table_name.clone(),
                trigger: trigger.clone(),
            }),
            Self::AlterTrigger { table_name, old, new } => Some(Self::AlterTrigger {
                table_name: table_name.clone(),
                old: new.clone(),
                new: old.clone(),
            }),
            Self::CreateView { view } => Some(Self::DropView { view: view.clone() }),
            Self::DropView { view } => Some(Self::CreateView { view: view.clone() }),
            Self::ReplaceView { old, new } => Some(Self::ReplaceView { old: new.clone(), new: old.clone() }),
            Self::CreateExtension { extension } => Some(Self::DropExtension { extension: extension.clone() }),
            Self::DropExtension { extension } => Some(Self::CreateExtension { extension: extension.clone() }),
            Self::CreateEnum { enum_def } => Some(Self::DropEnum { enum_def: enum_def.clone() }),
            Self::DropEnum { enum_def } => Some(Self::CreateEnum { enum_def: enum_def.clone() }),
            // Adding enum values is irreversible in PostgreSQL — there's no DROP VALUE.
            Self::AlterEnum { .. } => None,
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Self::CreateTable { .. } => "create_table",
            Self::DropTable { .. } => "drop_table",
            Self::RenameTable { .. } => "rename_table",
            Self::AddColumn { .. } => "add_column",
            Self::DropColumn { .. } => "drop_column",
            Self::RenameColumn { .. } => "rename_column",
            Self::AlterColumn { .. } => "alter_column",
            Self::AddForeignKey { .. } => "add_foreign_key",
            Self::DropForeignKey { .. } => "drop_foreign_key",
            Self::AddIndex { .. } => "add_index",
            Self::DropIndex { .. } => "drop_index",
            Self::AddConstraint { .. } => "add_constraint",
            Self::DropConstraint { .. } => "drop_constraint",
            Self::Statement { .. } => "statement",
            Self::Invoke { .. } => "invoke",
            Self::CreateFunction { .. } => "create_function",
            Self::AlterFunction { .. } => "alter_function",
            Self::DropFunction { .. } => "drop_function",
            Self::CreateTrigger { .. } => "create_trigger",
            Self::AlterTrigger { .. } => "alter_trigger",
            Self::DropTrigger { .. } => "drop_trigger",
            Self::CreateView { .. } => "create_view",
            Self::DropView { .. } => "drop_view",
            Self::ReplaceView { .. } => "replace_view",
            Self::CreateExtension { .. } => "create_extension",
            Self::DropExtension { .. } => "drop_extension",
            Self::CreateEnum { .. } => "create_enum",
            Self::DropEnum { .. } => "drop_enum",
            Self::AlterEnum { .. } => "alter_enum",
        }
    }
}
