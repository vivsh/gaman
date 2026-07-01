use std::collections::BTreeMap;

use crate::dialects::Dialect;

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
}
