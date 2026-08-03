use crate::operations::Operation;

use super::normalize::normalize_table_primary_key;
use super::*;

impl Schema {
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
                rename_fk_table_references(self, old_name, new_name);
                rename_partition_parent_references(self, old_name, new_name);
            }

            Operation::AcknowledgeTableOptions {
                table_name, new, ..
            } => {
                let table = self
                    .tables
                    .get_mut(table_name)
                    .ok_or_else(|| ReplayError::TableNotFound(table_name.clone()))?;
                table.options = new.clone();
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
                rename_fk_source_columns(table, old_name, new_name);
                normalize_table_primary_key(table);
                rename_fk_target_columns(self, table_name, old_name, new_name);
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

fn rename_fk_table_references(schema: &mut Schema, old_name: &str, new_name: &str) {
    for table in schema.tables.values_mut() {
        for fk in &mut table.foreign_keys {
            if fk.to_table == old_name {
                fk.to_table = new_name.to_string();
            }
        }
    }
}

fn rename_partition_parent_references(schema: &mut Schema, old_name: &str, new_name: &str) {
    for table in schema.tables.values_mut() {
        let Some(PostgresPartitionMeta::Child { parent, .. }) =
            &mut table.options.postgres_partition
        else {
            continue;
        };
        if parent == old_name {
            *parent = new_name.to_string();
        }
    }
}

fn rename_fk_source_columns(table: &mut Table, old_name: &str, new_name: &str) {
    for fk in &mut table.foreign_keys {
        for column in &mut fk.columns {
            if column == old_name {
                *column = new_name.to_string();
            }
        }
    }
}

fn rename_fk_target_columns(schema: &mut Schema, table_name: &str, old_name: &str, new_name: &str) {
    for table in schema.tables.values_mut() {
        for fk in &mut table.foreign_keys {
            if fk.to_table == table_name {
                for column in &mut fk.to_columns {
                    if column == old_name {
                        *column = new_name.to_string();
                    }
                }
            }
        }
    }
}
