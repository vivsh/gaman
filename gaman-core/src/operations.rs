use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::states::types::{Dep, EntityKind};
use crate::states::{
    Column, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index, Table, TriggerDef,
    ViewDef,
};

/// All possible schema change operations.
/// Each variant carries the minimal data needed to describe the change.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Operation {
    CreateTable {
        table: Table,
    },
    DropTable {
        table: Table,
    },
    RenameTable {
        old_name: String,
        new_name: String,
    },
    AddColumn {
        table_name: String,
        column: Column,
    },
    DropColumn {
        table_name: String,
        column: Column,
        #[serde(default)]
        cascade: bool,
    },
    RenameColumn {
        table_name: String,
        old_name: String,
        new_name: String,
    },
    AlterColumn {
        table_name: String,
        old: Column,
        new: Column,
        #[serde(default)]
        cast_expr: Option<String>,
    },
    AddForeignKey {
        table_name: String,
        foreign_key: ForeignKey,
    },
    DropForeignKey {
        table_name: String,
        foreign_key: ForeignKey,
        #[serde(default)]
        cascade: bool,
    },
    AddIndex {
        table_name: String,
        index: Index,
        #[serde(default)]
        concurrent: bool,
    },
    DropIndex {
        table_name: String,
        index: Index,
        #[serde(default)]
        concurrent: bool,
    },
    AddConstraint {
        table_name: String,
        constraint: Constraint,
    },
    DropConstraint {
        table_name: String,
        constraint: Constraint,
    },
    Statement {
        up: String,
        down: Option<String>,
    },
    CreateFunction {
        function: FunctionDef,
    },
    AlterFunction {
        old: FunctionDef,
        new: FunctionDef,
    },
    DropFunction {
        function: FunctionDef,
    },
    CreateTrigger {
        table_name: String,
        trigger: TriggerDef,
    },
    AlterTrigger {
        table_name: String,
        old: TriggerDef,
        new: TriggerDef,
    },
    DropTrigger {
        table_name: String,
        trigger: TriggerDef,
    },
    CreateView {
        view: ViewDef,
    },
    DropView {
        view: ViewDef,
    },
    ReplaceView {
        old: ViewDef,
        new: ViewDef,
    },
    CreateExtension {
        extension: ExtensionDef,
    },
    DropExtension {
        extension: ExtensionDef,
    },
    CreateEnum {
        enum_def: EnumDef,
    },
    DropEnum {
        enum_def: EnumDef,
    },
    RenameEnumValue {
        enum_name: String,
        #[serde(default)]
        schema: Option<String>,
        old_value: String,
        new_value: String,
    },
    AlterEnum {
        old: EnumDef,
        new: EnumDef,
    },
}

impl Operation {
    pub fn inverse(&self) -> Option<Operation> {
        match self {
            Self::CreateTable { .. } | Self::DropTable { .. } | Self::RenameTable { .. } => {
                self.inverse_table_op()
            }
            Self::AddColumn { .. }
            | Self::DropColumn { .. }
            | Self::RenameColumn { .. }
            | Self::AlterColumn { .. }
            | Self::AddForeignKey { .. }
            | Self::DropForeignKey { .. }
            | Self::AddIndex { .. }
            | Self::DropIndex { .. }
            | Self::AddConstraint { .. }
            | Self::DropConstraint { .. } => self.inverse_table_item_op(),
            Self::Statement { .. }
            | Self::CreateFunction { .. }
            | Self::AlterFunction { .. }
            | Self::DropFunction { .. } => self.inverse_function_op(),
            Self::CreateTrigger { .. } | Self::AlterTrigger { .. } | Self::DropTrigger { .. } => {
                self.inverse_trigger_op()
            }
            Self::CreateView { .. } | Self::DropView { .. } | Self::ReplaceView { .. } => {
                self.inverse_view_op()
            }
            Self::CreateExtension { .. } | Self::DropExtension { .. } => {
                self.inverse_extension_op()
            }
            Self::CreateEnum { .. }
            | Self::DropEnum { .. }
            | Self::RenameEnumValue { .. }
            | Self::AlterEnum { .. } => self.inverse_enum_op(),
        }
    }

    fn inverse_table_op(&self) -> Option<Operation> {
        match self {
            Self::CreateTable { table } => Some(Self::DropTable {
                table: table.clone(),
            }),
            Self::DropTable { table } => Some(Self::CreateTable {
                table: table.clone(),
            }),
            Self::RenameTable { old_name, new_name } => Some(Self::RenameTable {
                old_name: new_name.clone(),
                new_name: old_name.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_table_item_op(&self) -> Option<Operation> {
        match self {
            Self::AddColumn { table_name, column } => Some(Self::DropColumn {
                table_name: table_name.clone(),
                column: column.clone(),
                cascade: false,
            }),
            Self::DropColumn {
                table_name, column, ..
            } => Some(Self::AddColumn {
                table_name: table_name.clone(),
                column: column.clone(),
            }),
            Self::RenameColumn {
                table_name,
                old_name,
                new_name,
            } => Some(Self::RenameColumn {
                table_name: table_name.clone(),
                old_name: new_name.clone(),
                new_name: old_name.clone(),
            }),
            Self::AlterColumn {
                table_name,
                old,
                new,
                ..
            } => Some(Self::AlterColumn {
                table_name: table_name.clone(),
                old: new.clone(),
                new: old.clone(),
                cast_expr: None,
            }),
            Self::AddForeignKey {
                table_name,
                foreign_key,
            } => Some(Self::DropForeignKey {
                table_name: table_name.clone(),
                foreign_key: foreign_key.clone(),
                cascade: false,
            }),
            Self::DropForeignKey {
                table_name,
                foreign_key,
                ..
            } => Some(Self::AddForeignKey {
                table_name: table_name.clone(),
                foreign_key: foreign_key.clone(),
            }),
            Self::AddIndex {
                table_name,
                index,
                concurrent,
            } => Some(Self::DropIndex {
                table_name: table_name.clone(),
                index: index.clone(),
                concurrent: *concurrent,
            }),
            Self::DropIndex {
                table_name,
                index,
                concurrent,
            } => Some(Self::AddIndex {
                table_name: table_name.clone(),
                index: index.clone(),
                concurrent: *concurrent,
            }),
            Self::AddConstraint {
                table_name,
                constraint,
            } => Some(Self::DropConstraint {
                table_name: table_name.clone(),
                constraint: constraint.clone(),
            }),
            Self::DropConstraint {
                table_name,
                constraint,
            } => Some(Self::AddConstraint {
                table_name: table_name.clone(),
                constraint: constraint.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_function_op(&self) -> Option<Operation> {
        match self {
            Self::Statement {
                up,
                down: Some(down),
            } => Some(Self::Statement {
                up: down.clone(),
                down: Some(up.clone()),
            }),
            Self::Statement { down: None, .. } => None,
            Self::CreateFunction { function } => Some(Self::DropFunction {
                function: function.clone(),
            }),
            Self::DropFunction { function } => Some(Self::CreateFunction {
                function: function.clone(),
            }),
            Self::AlterFunction { old, new } => Some(Self::AlterFunction {
                old: new.clone(),
                new: old.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_trigger_op(&self) -> Option<Operation> {
        match self {
            Self::CreateTrigger {
                table_name,
                trigger,
            } => Some(Self::DropTrigger {
                table_name: table_name.clone(),
                trigger: trigger.clone(),
            }),
            Self::DropTrigger {
                table_name,
                trigger,
            } => Some(Self::CreateTrigger {
                table_name: table_name.clone(),
                trigger: trigger.clone(),
            }),
            Self::AlterTrigger {
                table_name,
                old,
                new,
            } => Some(Self::AlterTrigger {
                table_name: table_name.clone(),
                old: new.clone(),
                new: old.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_view_op(&self) -> Option<Operation> {
        match self {
            Self::CreateView { view } => Some(Self::DropView { view: view.clone() }),
            Self::DropView { view } => Some(Self::CreateView { view: view.clone() }),
            Self::ReplaceView { old, new } => Some(Self::ReplaceView {
                old: new.clone(),
                new: old.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_extension_op(&self) -> Option<Operation> {
        match self {
            Self::CreateExtension { extension } => Some(Self::DropExtension {
                extension: extension.clone(),
            }),
            Self::DropExtension { extension } => Some(Self::CreateExtension {
                extension: extension.clone(),
            }),
            _ => None,
        }
    }

    fn inverse_enum_op(&self) -> Option<Operation> {
        match self {
            Self::CreateEnum { enum_def } => Some(Self::DropEnum {
                enum_def: enum_def.clone(),
            }),
            Self::DropEnum { enum_def } => Some(Self::CreateEnum {
                enum_def: enum_def.clone(),
            }),
            Self::RenameEnumValue {
                enum_name,
                schema,
                old_value,
                new_value,
            } => Some(Self::RenameEnumValue {
                enum_name: enum_name.clone(),
                schema: schema.clone(),
                old_value: new_value.clone(),
                new_value: old_value.clone(),
            }),
            Self::AlterEnum { .. } => None,
            _ => None,
        }
    }

    pub fn table_name(&self) -> Option<&str> {
        match self {
            Self::CreateTable { table } | Self::DropTable { table } => Some(&table.name),
            Self::RenameTable { old_name, .. } => Some(old_name),
            Self::AddColumn { table_name, .. }
            | Self::DropColumn { table_name, .. }
            | Self::RenameColumn { table_name, .. }
            | Self::AlterColumn { table_name, .. }
            | Self::AddForeignKey { table_name, .. }
            | Self::DropForeignKey { table_name, .. }
            | Self::AddIndex { table_name, .. }
            | Self::DropIndex { table_name, .. }
            | Self::AddConstraint { table_name, .. }
            | Self::DropConstraint { table_name, .. }
            | Self::CreateTrigger { table_name, .. }
            | Self::AlterTrigger { table_name, .. }
            | Self::DropTrigger { table_name, .. } => Some(table_name),
            _ => None,
        }
    }

    pub fn entity_name(&self) -> Cow<'_, str> {
        match self {
            Self::CreateTable { table } | Self::DropTable { table } => {
                if table.schema.is_some() {
                    Cow::Owned(table.qualified_name())
                } else {
                    Cow::Borrowed(&table.name)
                }
            }
            Self::RenameTable { old_name, .. } => Cow::Borrowed(old_name),
            Self::AddColumn { column, .. } | Self::DropColumn { column, .. } => {
                Cow::Borrowed(&column.name)
            }
            Self::RenameColumn { old_name, .. } => Cow::Borrowed(old_name),
            Self::AlterColumn { old, .. } => Cow::Borrowed(&old.name),
            Self::AddForeignKey { foreign_key, .. } | Self::DropForeignKey { foreign_key, .. } => {
                Cow::Borrowed(&foreign_key.name)
            }
            Self::AddIndex { index, .. } | Self::DropIndex { index, .. } => {
                Cow::Borrowed(&index.name)
            }
            Self::AddConstraint { constraint, .. } | Self::DropConstraint { constraint, .. } => {
                Cow::Borrowed(constraint.name())
            }
            Self::CreateTrigger { trigger, .. } | Self::DropTrigger { trigger, .. } => {
                Cow::Borrowed(trigger.name.as_deref().unwrap_or(""))
            }
            Self::AlterTrigger { old, .. } => Cow::Borrowed(old.name.as_deref().unwrap_or("")),
            Self::CreateFunction { function } | Self::DropFunction { function } => {
                if function.schema.is_some() {
                    Cow::Owned(function.qualified_name())
                } else {
                    Cow::Borrowed(&function.name)
                }
            }
            Self::AlterFunction { old, .. } => {
                if old.schema.is_some() {
                    Cow::Owned(old.qualified_name())
                } else {
                    Cow::Borrowed(&old.name)
                }
            }
            Self::CreateView { view } | Self::DropView { view } => {
                if view.schema.is_some() {
                    Cow::Owned(view.qualified_name())
                } else {
                    Cow::Borrowed(&view.name)
                }
            }
            Self::ReplaceView { old, .. } => {
                if old.schema.is_some() {
                    Cow::Owned(old.qualified_name())
                } else {
                    Cow::Borrowed(&old.name)
                }
            }
            Self::CreateExtension { extension } | Self::DropExtension { extension } => {
                if extension.schema.is_some() {
                    Cow::Owned(extension.qualified_name())
                } else {
                    Cow::Borrowed(&extension.name)
                }
            }
            Self::CreateEnum { enum_def } | Self::DropEnum { enum_def } => {
                if enum_def.schema.is_some() {
                    Cow::Owned(enum_def.qualified_name())
                } else {
                    Cow::Borrowed(&enum_def.name)
                }
            }
            Self::RenameEnumValue {
                enum_name, schema, ..
            } => {
                if let Some(schema) = schema {
                    Cow::Owned(format!("{schema}.{enum_name}"))
                } else {
                    Cow::Borrowed(enum_name)
                }
            }
            Self::AlterEnum { old, .. } => {
                if old.schema.is_some() {
                    Cow::Owned(old.qualified_name())
                } else {
                    Cow::Borrowed(&old.name)
                }
            }
            Self::Statement { up, .. } => Cow::Borrowed(up),
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
            Self::RenameEnumValue { .. } => "rename_enum_value",
            Self::AlterEnum { .. } => "alter_enum",
        }
    }

    pub fn entity_kind(&self) -> Option<EntityKind> {
        match self {
            Self::CreateTable { .. } | Self::DropTable { .. } => Some(EntityKind::Table),
            Self::CreateFunction { .. }
            | Self::AlterFunction { .. }
            | Self::DropFunction { .. } => Some(EntityKind::Function),
            Self::CreateEnum { .. }
            | Self::RenameEnumValue { .. }
            | Self::AlterEnum { .. }
            | Self::DropEnum { .. } => Some(EntityKind::Enum),
            Self::CreateExtension { .. } | Self::DropExtension { .. } => {
                Some(EntityKind::Extension)
            }
            Self::CreateView { .. } | Self::DropView { .. } | Self::ReplaceView { .. } => {
                Some(EntityKind::View)
            }
            Self::AddColumn { .. }
            | Self::DropColumn { .. }
            | Self::AlterColumn { .. }
            | Self::RenameColumn { .. } => Some(EntityKind::Column),
            Self::AddForeignKey { .. } | Self::DropForeignKey { .. } => {
                Some(EntityKind::ForeignKey)
            }
            Self::AddIndex { .. } | Self::DropIndex { .. } => Some(EntityKind::Index),
            Self::AddConstraint { .. } | Self::DropConstraint { .. } => {
                Some(EntityKind::Constraint)
            }
            Self::CreateTrigger { .. } | Self::AlterTrigger { .. } | Self::DropTrigger { .. } => {
                Some(EntityKind::Trigger)
            }
            Self::RenameTable { .. } | Self::Statement { .. } => None,
        }
    }

    pub fn is_create(&self) -> bool {
        matches!(
            self,
            Self::CreateTable { .. }
                | Self::CreateFunction { .. }
                | Self::CreateEnum { .. }
                | Self::CreateExtension { .. }
                | Self::CreateView { .. }
                | Self::CreateTrigger { .. }
                | Self::RenameEnumValue { .. }
                | Self::AlterEnum { .. }
                | Self::AddColumn { .. }
                | Self::AddForeignKey { .. }
                | Self::AddIndex { .. }
                | Self::AddConstraint { .. }
        )
    }

    pub fn is_drop(&self) -> bool {
        matches!(
            self,
            Self::DropTable { .. }
                | Self::DropFunction { .. }
                | Self::DropEnum { .. }
                | Self::DropExtension { .. }
                | Self::DropView { .. }
                | Self::DropTrigger { .. }
                | Self::DropColumn { .. }
                | Self::DropForeignKey { .. }
                | Self::DropIndex { .. }
                | Self::DropConstraint { .. }
        )
    }

    pub fn forward_deps(&self) -> Vec<Dep> {
        match self {
            Self::CreateTable { table } | Self::DropTable { table } => {
                let mut deps = vec![Dep::all_of(EntityKind::Extension)];
                for col in &table.columns {
                    deps.push(Dep::new(EntityKind::Enum, &col.col_type));
                }
                deps
            }
            Self::AddColumn { table_name, column }
            | Self::DropColumn {
                table_name, column, ..
            } => {
                vec![
                    Dep::new(EntityKind::Table, table_name),
                    Dep::new(EntityKind::Enum, &column.col_type),
                ]
            }
            Self::AlterColumn {
                table_name, new, ..
            } => {
                vec![
                    Dep::new(EntityKind::Table, table_name),
                    Dep::new(EntityKind::Enum, &new.col_type),
                ]
            }
            Self::AddForeignKey {
                table_name,
                foreign_key,
            }
            | Self::DropForeignKey {
                table_name,
                foreign_key,
                ..
            } => {
                vec![
                    Dep::new(EntityKind::Table, table_name),
                    Dep::new(EntityKind::Table, &foreign_key.to_table),
                ]
            }
            Self::AddIndex { table_name, .. } | Self::DropIndex { table_name, .. } => {
                vec![Dep::new(EntityKind::Table, table_name)]
            }
            Self::AddConstraint { table_name, .. } | Self::DropConstraint { table_name, .. } => {
                vec![Dep::new(EntityKind::Table, table_name)]
            }
            Self::CreateTrigger {
                table_name,
                trigger,
            }
            | Self::AlterTrigger {
                table_name,
                new: trigger,
                ..
            } => {
                let mut deps = vec![Dep::new(EntityKind::Table, table_name)];
                if let Some(fn_name) = trigger.function_name.as_deref() {
                    deps.push(Dep::new(EntityKind::Function, fn_name));
                }
                deps
            }
            Self::DropTrigger {
                table_name,
                trigger,
            } => {
                let mut deps = vec![Dep::new(EntityKind::Table, table_name)];
                if let Some(fn_name) = trigger.function_name.as_deref() {
                    deps.push(Dep::new(EntityKind::Function, fn_name));
                }
                deps
            }
            Self::CreateFunction { .. }
            | Self::AlterFunction { .. }
            | Self::DropFunction { .. } => {
                vec![Dep::all_of(EntityKind::Extension)]
            }
            Self::CreateView { .. } | Self::ReplaceView { .. } | Self::DropView { .. } => {
                vec![
                    Dep::all_of(EntityKind::Table),
                    Dep::all_of(EntityKind::Function),
                ]
            }
            Self::CreateExtension { .. } | Self::DropExtension { .. } => vec![],
            Self::CreateEnum { .. }
            | Self::RenameEnumValue { .. }
            | Self::AlterEnum { .. }
            | Self::DropEnum { .. } => vec![],
            Self::RenameTable { .. } | Self::RenameColumn { .. } | Self::Statement { .. } => vec![],
        }
    }

    pub fn backward_deps(&self) -> Vec<Dep> {
        match self {
            Self::AlterColumn {
                table_name, old, ..
            } => {
                vec![
                    Dep::new(EntityKind::Table, table_name),
                    Dep::new(EntityKind::Enum, &old.col_type),
                ]
            }
            _ => self.forward_deps(),
        }
    }
}
