use crate::dialects::Dialect;
use super::{
    Column, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index,
    Schema, Table, ViewDef, schema_qualified_key,
};

/// A Rust struct that maps to a database table. Implement this to make the
/// struct usable as a schema definition that gaman can diff and migrate against.
pub trait IntoTable {
    fn into_table(dialect: &Dialect) -> Table;
}

pub struct ColumnBuilder {
    col: Column,
}

impl ColumnBuilder {
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
        self
    }

    pub fn default(mut self, expr: impl Into<String>) -> Self {
        self.col.default = Some(expr.into());
        self
    }

    pub fn references(mut self, table: impl Into<String>, column: impl Into<String>) -> Self {
        self.col.references = Some(ColumnRef {
            table: table.into(),
            column: column.into(),
            name: None,
        });
        self
    }

    pub fn references_named(
        mut self,
        name: impl Into<String>,
        table: impl Into<String>,
        column: impl Into<String>,
    ) -> Self {
        self.col.references = Some(ColumnRef {
            table: table.into(),
            column: column.into(),
            name: Some(name.into()),
        });
        self
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
    pub fn new(name: impl Into<String>) -> Self {
        let name = name.into();
        Self {
            table: Table {
                name,
                schema: None,
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
        mut self,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
    ) -> Self {
        let from = from.into();
        let name = format!("{}_{}_fkey", self.table.name, from);
        self.table.foreign_keys.push(ForeignKey {
            name,
            from_column: from,
            to_table: to_table.into(),
            to_column: to_column.into(),
        });
        self
    }

    pub fn foreign_key_named(
        mut self,
        fk_name: impl Into<String>,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
    ) -> Self {
        self.table.foreign_keys.push(ForeignKey {
            name: fk_name.into(),
            from_column: from.into(),
            to_table: to_table.into(),
            to_column: to_column.into(),
        });
        self
    }

    pub fn index(mut self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.table.indexes.push(Index {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique: false,
            predicate: None,
        });
        self
    }

    pub fn unique_index(mut self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.table.indexes.push(Index {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique: true,
            predicate: None,
        });
        self
    }

    pub fn check(mut self, name: impl Into<String>, expression: impl Into<String>) -> Self {
        self.table.constraints.push(Constraint::Check {
            name: name.into(),
            expression: expression.into(),
        });
        self
    }

    pub fn unique(mut self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.table.constraints.push(Constraint::Unique {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
        });
        self
    }

    pub fn build(self) -> Table {
        self.table
    }
}

pub struct SchemaBuilder {
    dialect: Dialect,
    state: Schema,
}

impl SchemaBuilder {
    pub fn new(dialect: Dialect) -> Self {
        Self { dialect, state: Schema::default() }
    }

    /// Add a table from any type that implements [`IntoTable`].
    pub fn table<T: IntoTable>(mut self) -> Self {
        let t = T::into_table(&self.dialect);
        let key = schema_qualified_key(&t.name, t.schema.as_deref());
        self.state.tables.insert(key, t);
        self
    }

    pub fn extension(mut self, name: impl Into<String>) -> Self {
        let name = name.into();
        self.state
            .extensions
            .insert(name.clone(), ExtensionDef { name, schema: None, version: None });
        self
    }

    pub fn extension_versioned(
        mut self,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Self {
        let name = name.into();
        self.state.extensions.insert(
            name.clone(),
            ExtensionDef { name, schema: None, version: Some(version.into()) },
        );
        self
    }

    pub fn view(mut self, name: impl Into<String>, definition: impl Into<String>) -> Self {
        let name = name.into();
        self.state.views.insert(
            name.clone(),
            ViewDef { name, schema: None, definition: definition.into() },
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
        self.state
    }
}
