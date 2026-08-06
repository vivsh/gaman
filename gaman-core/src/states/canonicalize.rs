use std::collections::BTreeMap;

use crate::dialects::Dialect;
use crate::states::types::EntityKind;

use super::normalize::normalize_table_primary_key;
use super::*;

impl Schema {
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
        self.normalize_schemas(dialect);
        crate::managed_rows::canonicalize_schema(self);
    }

    fn normalize_schemas(&mut self, dialect: &Dialect) {
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
            table.schema =
                dialect.canonicalize_schema_name(EntityKind::Table, table.schema.as_deref());
        }
        rekey(&mut self.tables, |t| t.qualified_name());

        for func in self.functions.values_mut() {
            func.schema =
                dialect.canonicalize_schema_name(EntityKind::Function, func.schema.as_deref());
        }
        rekey(&mut self.functions, |f| f.qualified_name());

        for view in self.views.values_mut() {
            view.schema =
                dialect.canonicalize_schema_name(EntityKind::View, view.schema.as_deref());
        }
        rekey(&mut self.views, |v| v.qualified_name());

        for ext in self.extensions.values_mut() {
            ext.schema =
                dialect.canonicalize_schema_name(EntityKind::Extension, ext.schema.as_deref());
        }
        rekey(&mut self.extensions, |e| e.qualified_name());

        for en in self.enums.values_mut() {
            en.schema = dialect.canonicalize_schema_name(EntityKind::Enum, en.schema.as_deref());
        }
        rekey(&mut self.enums, |e| e.qualified_name());
    }
}
