use crate::operations::Operation;
use crate::states::{ReplayError, Schema};

use super::ManagedRows;

/// Applies one managed-row operation to replayed state.
pub(crate) fn apply_operation(
    schema: &mut Schema,
    operation: &Operation,
) -> Result<bool, ReplayError> {
    match operation {
        Operation::InsertRow {
            table_name,
            key,
            row,
        } => {
            let declaration = schema
                .managed_rows
                .entry(table_name.clone())
                .or_insert_with(|| ManagedRows { rows: Vec::new() });
            if declaration
                .rows
                .iter()
                .any(|known| known.identity(key).ok() == row.identity(key).ok())
            {
                return Err(ReplayError::InvalidMigration(format!(
                    "duplicate or incompatible managed row on '{table_name}'"
                )));
            }
            declaration.rows.push(row.clone());
            Ok(true)
        }
        Operation::UpdateRow {
            table_name,
            key,
            old,
            new,
        } => {
            let declaration = schema.managed_rows.get_mut(table_name).ok_or_else(|| {
                ReplayError::InvalidMigration(format!("managed rows for '{table_name}' not found"))
            })?;
            let identity = old.identity(key).map_err(ReplayError::InvalidMigration)?;
            let row = declaration
                .rows
                .iter_mut()
                .find(|row| row.identity(key).ok().as_deref() == Some(identity.as_str()))
                .ok_or_else(|| {
                    ReplayError::InvalidMigration(format!(
                        "managed row '{table_name}[{identity}]' not found"
                    ))
                })?;
            if row != old {
                return Err(ReplayError::InvalidMigration(format!(
                    "managed row '{table_name}[{identity}]' does not match expected old state"
                )));
            }
            *row = new.clone();
            Ok(true)
        }
        Operation::DeleteRow {
            table_name,
            key,
            row,
        } => {
            let declaration = schema.managed_rows.get_mut(table_name).ok_or_else(|| {
                ReplayError::InvalidMigration(format!("managed rows for '{table_name}' not found"))
            })?;
            let identity = row.identity(key).map_err(ReplayError::InvalidMigration)?;
            let position = declaration
                .rows
                .iter()
                .position(|known| known.identity(key).ok().as_deref() == Some(identity.as_str()))
                .ok_or_else(|| {
                    ReplayError::InvalidMigration(format!(
                        "managed row '{table_name}[{identity}]' not found"
                    ))
                })?;
            declaration.rows.remove(position);
            if declaration.rows.is_empty() {
                schema.managed_rows.remove(table_name);
            }
            Ok(true)
        }
        _ => Ok(false),
    }
}
