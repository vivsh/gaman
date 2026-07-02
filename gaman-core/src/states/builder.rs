use super::{
    Column, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index,
    PrimaryKey, Schema, SchemaLoadError, Table, TriggerDef, ViewDef, names, schema_qualified_key,
};
use crate::dialects::Dialect;

/// Map a Rust type to a table definition.
pub trait IntoTable {
    fn into_table(dialect: &Dialect) -> Table;
}

pub struct ColumnBuilder {
    col: Column,
}

impl ColumnBuilder {
    fn with_reference(
        mut self,
        name: Option<String>,
        table: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        self.col.references = Some(ColumnRef {
            table: table.into(),
            column: column.into(),
            name,
        });
        self
    }

    pub fn nullable(mut self) -> Self {
        self.col.nullable = true;
        self
    }

    pub fn not_null(mut self) -> Self {
        self.col.nullable = false;
        self
    }

    pub fn primary_key(mut self) -> Self {
        self.col.primary_key = true;
        self.col.nullable = false;
        self
    }

    pub fn default(mut self, expr: impl Into<String>) -> Self {
        self.col.default = Some(expr.into());
        self
    }

    pub fn references(self, table: impl Into<String>, column: impl Into<String>) -> Self {
        self.with_reference(None, table, column)
    }

    pub fn references_named(
        self,
        name: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        self.with_reference(Some(name.into()), table, column)
    }

    pub fn check(mut self, expr: impl Into<String>) -> Self {
        self.col.check = Some(expr.into());
        self
    }

    fn finish(self) -> Column {
        self.col
    }
}

pub struct TableBuilder {
    table: Table,
}

impl TableBuilder {
    fn push_foreign_key(
        mut self,
        name: String,
        from_columns: impl IntoIterator<Item = impl Into<String>>,
        to_table: impl Into<String>,
        to_columns: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.table
            .foreign_keys
            .push(ForeignKey::new(name, from_columns, to_table, to_columns));
        self
    }

    fn push_index(mut self, name: impl Into<String>, columns: &[&str], unique: bool) -> Self {
        self.table.indexes.push(Index {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
            predicate: None,
        });
        self
    }

    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            table: Table {
                name,
                schema: None,
                primary_key: None,
                columns: vec![],
                foreign_keys: vec![],
                indexes: vec![],
                constraints: vec![],
                triggers: vec![],
            },
        }
    }

    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.table.schema = Some(schema.into());
        self
    }

    pub fn column(
        mut self,
        name: impl Into<String>,
        col_type: impl Into<String>,
        f: impl FnOnce(ColumnBuilder) -> ColumnBuilder,
    ) -> Self {
        let b = ColumnBuilder {
            col: Column {
                name: name.into(),
                col_type: col_type.into(),
                ..Default::default()
            },
        };
        self.table.columns.push(f(b).finish());
        self
    }

    /// Shorthand: adds a `bigserial` primary key column named `id`.
    pub fn id(self) -> Self {
        self.column("id", "bigserial", |c| c.primary_key())
    }

    pub fn foreign_key(
        self,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
    ) -> Self {
        let from = from.into();
        let name = names::foreign_key(&self.table.name, &[from.as_str()]);
        self.push_foreign_key(name, [from], to_table, [to_column.into()])
    }

    pub fn foreign_key_named(
        self,
        fk_name: impl Into<String>,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
    ) -> Self {
        self.push_foreign_key(fk_name.into(), [from.into()], to_table, [to_column.into()])
    }

    pub fn foreign_key_columns(
        self,
        from_columns: &[&str],
        to_table: impl Into<String>,
        to_columns: &[&str],
    ) -> Self {
        let name = names::foreign_key(&self.table.name, from_columns);
        self.push_foreign_key(
            name,
            from_columns.iter().copied(),
            to_table,
            to_columns.iter().copied(),
        )
    }

    pub fn foreign_key_named_columns(
        self,
        fk_name: impl Into<String>,
        from_columns: &[&str],
        to_table: impl Into<String>,
        to_columns: &[&str],
    ) -> Self {
        self.push_foreign_key(
            fk_name.into(),
            from_columns.iter().copied(),
            to_table,
            to_columns.iter().copied(),
        )
    }

    pub fn index_columns(self, columns: &[&str]) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index(name, columns, false)
    }

    pub fn unique_index_columns(self, columns: &[&str]) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index(name, columns, true)
    }

    pub fn index(self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.push_index(name, columns, false)
    }

    pub fn unique_index(self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.push_index(name, columns, true)
    }

    pub fn check(mut self, name: impl Into<String>, expression: impl Into<String>) -> Self {
        self.table.constraints.push(Constraint::Check {
            name: name.into(),
            expression: expression.into(),
        });
        self
    }

    pub fn check_expr(self, expression: impl Into<String>) -> Self {
        let name = names::table_check(&self.table.name);
        self.check(name, expression)
    }

    pub fn unique(mut self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.table.constraints.push(Constraint::Unique {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn unique_columns(self, columns: &[&str]) -> Self {
        let name = names::unique(&self.table.name, columns);
        self.unique(name, columns)
    }

    pub fn primary_key(mut self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.table.primary_key = Some(PrimaryKey {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn primary_key_columns(self, columns: &[&str]) -> Self {
        let name = names::primary_key(&self.table.name);
        self.primary_key(name, columns)
    }

    pub fn trigger(self, trigger: TriggerDef) -> Self {
        let mut this = self;
        this.table.triggers.push(trigger);
        this
    }

    pub fn build(mut self) -> Table {
        if self.table.primary_key.is_none() {
            let columns: Vec<String> = self
                .table
                .columns
                .iter()
                .filter(|column| column.primary_key)
                .map(|column| column.name.clone())
                .collect();
            if !columns.is_empty() {
                self.table.primary_key = Some(PrimaryKey {
                    name: names::primary_key(&self.table.name),
                    columns,
                });
            }
        }
        if let Some(pk) = &self.table.primary_key {
            for column in &mut self.table.columns {
                column.primary_key = pk.columns.iter().any(|name| name == &column.name);
                if column.primary_key {
                    column.nullable = false;
                }
            }
        }
        self.table
    }
}

pub struct SchemaBuilder {
    dialect: Dialect,
    state: Schema,
}

impl SchemaBuilder {
    fn insert_extension(mut self, name: impl Into<String>, version: Option<String>) -> Self {
        let name = name.into();
        self.state.extensions.insert(
            name.clone(),
            ExtensionDef {
                name,
                schema: None,
                version,
            },
        );
        self
    }

    pub fn new(dialect: Dialect) -> Self {
        Self {
            dialect,
            state: Schema::default(),
        }
    }

    /// Add a table from any type that implements [`IntoTable`].
    pub fn table<T: IntoTable>(mut self) -> Self {
        let t = T::into_table(&self.dialect);
        let key = schema_qualified_key(&t.name, t.schema.as_deref());
        self.state.tables.insert(key, t);
        self
    }

    pub fn extension(self, name: impl Into<String>) -> Self {
        self.insert_extension(name, None)
    }

    pub fn extension_versioned(self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.insert_extension(name, Some(version.into()))
    }

    pub fn view(mut self, name: impl Into<String>, definition: impl Into<String>) -> Self {
        let name = name.into();
        self.state.views.insert(
            name.clone(),
            ViewDef {
                name,
                schema: None,
                definition: definition.into(),
            },
        );
        self
    }

    pub fn function(mut self, f: FunctionDef) -> Self {
        let key = schema_qualified_key(&f.name, f.schema.as_deref());
        self.state.functions.insert(key, f);
        self
    }

    pub fn enum_type(mut self, name: impl Into<String>, values: &[&str]) -> Self {
        let name = name.into();
        self.state.enums.insert(
            name.clone(),
            EnumDef {
                name,
                schema: None,
                values: values.iter().map(|s| s.to_string()).collect(),
            },
        );
        self
    }

    pub fn build(self) -> Schema {
        let mut state = self.state;
        state.normalize();
        state
    }

    pub fn build_checked(self) -> Result<Schema, SchemaLoadError> {
        let dialect = self.dialect;
        Ok(self.build().prepare(dialect)?)
    }

    /// Load schema from a `.yaml`, `.sql`, or directory path.
    #[cfg(feature = "fs")]
    pub fn load_file(self, path: impl AsRef<std::path::Path>) -> Result<Schema, SchemaLoadError> {
        Schema::from_file(path.as_ref())
    }

    /// Load schema from a directory of `.yaml` or `.sql` files (merged in alphabetical order).
    #[cfg(feature = "fs")]
    pub fn load_dir(self, path: impl AsRef<std::path::Path>) -> Result<Schema, SchemaLoadError> {
        Schema::from_dir(path.as_ref())
    }
}

/// Allows both `Schema` and `Result<Schema, E>` to be returned from the `with_schema` closure.
/// `Schema` is treated as infallible; `Result<Schema, E>` propagates the error.
pub trait IntoSchema {
    fn into_schema(self) -> Result<Schema, SchemaLoadError>;
}

impl IntoSchema for Schema {
    fn into_schema(self) -> Result<Schema, SchemaLoadError> {
        Ok(self)
    }
}

impl IntoSchema for Result<Schema, SchemaLoadError> {
    fn into_schema(self) -> Result<Schema, SchemaLoadError> {
        self
    }
}
