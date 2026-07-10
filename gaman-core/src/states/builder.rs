use super::{
    Column, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef, Index,
    OpaqueMeta, PrimaryKey, Schema, SchemaLoadError, Table, TableOptionsMeta, TriggerDef, ViewDef,
    names, schema_qualified_key,
};
use crate::column_type::ColumnType;
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
            on_delete: None,
            on_update: None,
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

    /// Set a generated column expression.
    pub fn generated(mut self, expr: impl Into<String>) -> Self {
        let expr = expr.into();
        if !expr.trim().is_empty() {
            self.col.generated = Some(expr);
        }
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

    /// Set the inline foreign-key `ON DELETE` action.
    pub fn on_delete(mut self, action: impl Into<String>) -> Self {
        if let Some(reference) = &mut self.col.references {
            let action = action.into();
            if !action.trim().is_empty() {
                reference.on_delete = Some(action);
            }
        }
        self
    }

    /// Set the inline foreign-key `ON UPDATE` action.
    pub fn on_update(mut self, action: impl Into<String>) -> Self {
        if let Some(reference) = &mut self.col.references {
            let action = action.into();
            if !action.trim().is_empty() {
                reference.on_update = Some(action);
            }
        }
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
            opaque: OpaqueMeta::default(),
        });
        self
    }

    fn push_index_with(
        mut self,
        name: impl Into<String>,
        columns: &[&str],
        unique: bool,
        f: impl FnOnce(Index) -> Index,
    ) -> Self {
        let index = Index {
            name: name.into(),
            columns: columns.iter().map(|s| s.to_string()).collect(),
            unique,
            predicate: None,
            opaque: OpaqueMeta::default(),
        };
        self.table.indexes.push(f(index));
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
                options: TableOptionsMeta::default(),
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

    /// Add a column whose SQL type and default nullability are inferred from a Rust type.
    ///
    /// The caller supplies the dialect because type mapping is dialect-specific.
    /// The closure runs after inference and can override nullability or add
    /// primary-key, default, reference, and check metadata.
    pub fn column_from_type<T: ColumnType>(
        mut self,
        dialect: &Dialect,
        name: impl Into<String>,
        f: impl FnOnce(ColumnBuilder) -> ColumnBuilder,
    ) -> Self {
        let desc = T::column_desc(dialect);
        let b = ColumnBuilder {
            col: Column {
                name: name.into(),
                col_type: desc.sql_type.to_string(),
                nullable: desc.nullable,
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

    /// Add a foreign key and let the caller set advanced metadata.
    pub fn foreign_key_with(
        self,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
        f: impl FnOnce(ForeignKey) -> ForeignKey,
    ) -> Self {
        let from = from.into();
        let name = names::foreign_key(&self.table.name, &[from.as_str()]);
        let foreign_key = ForeignKey::single(name, from, to_table, to_column);
        self.foreign_key_obj(f(foreign_key))
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

    /// Add a named foreign key and let the caller set advanced metadata.
    pub fn foreign_key_named_with(
        self,
        fk_name: impl Into<String>,
        from: impl Into<String>,
        to_table: impl Into<String>,
        to_column: impl Into<String>,
        f: impl FnOnce(ForeignKey) -> ForeignKey,
    ) -> Self {
        let foreign_key = ForeignKey::single(fk_name, from, to_table, to_column);
        self.foreign_key_obj(f(foreign_key))
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

    /// Add a composite foreign key and let the caller set advanced metadata.
    pub fn foreign_key_columns_with(
        self,
        from_columns: &[&str],
        to_table: impl Into<String>,
        to_columns: &[&str],
        f: impl FnOnce(ForeignKey) -> ForeignKey,
    ) -> Self {
        let name = names::foreign_key(&self.table.name, from_columns);
        let foreign_key = ForeignKey::new(
            name,
            from_columns.iter().copied(),
            to_table,
            to_columns.iter().copied(),
        );
        self.foreign_key_obj(f(foreign_key))
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

    /// Add a named composite foreign key and let the caller set advanced metadata.
    pub fn foreign_key_named_columns_with(
        self,
        fk_name: impl Into<String>,
        from_columns: &[&str],
        to_table: impl Into<String>,
        to_columns: &[&str],
        f: impl FnOnce(ForeignKey) -> ForeignKey,
    ) -> Self {
        let foreign_key = ForeignKey::new(
            fk_name,
            from_columns.iter().copied(),
            to_table,
            to_columns.iter().copied(),
        );
        self.foreign_key_obj(f(foreign_key))
    }

    /// Add a fully constructed foreign key.
    pub fn foreign_key_obj(mut self, foreign_key: ForeignKey) -> Self {
        self.table.foreign_keys.push(foreign_key);
        self
    }

    pub fn index_columns(self, columns: &[&str]) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index(name, columns, false)
    }

    /// Add an index with generated name and advanced metadata.
    pub fn index_columns_with(self, columns: &[&str], f: impl FnOnce(Index) -> Index) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index_with(name, columns, false, f)
    }

    pub fn unique_index_columns(self, columns: &[&str]) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index(name, columns, true)
    }

    /// Add a unique index with generated name and advanced metadata.
    pub fn unique_index_columns_with(
        self,
        columns: &[&str],
        f: impl FnOnce(Index) -> Index,
    ) -> Self {
        let name = names::index(&self.table.name, columns);
        self.push_index_with(name, columns, true, f)
    }

    pub fn index(self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.push_index(name, columns, false)
    }

    /// Add an index and let the caller set advanced metadata.
    pub fn index_with(
        self,
        name: impl Into<String>,
        columns: &[&str],
        f: impl FnOnce(Index) -> Index,
    ) -> Self {
        self.push_index_with(name, columns, false, f)
    }

    pub fn unique_index(self, name: impl Into<String>, columns: &[&str]) -> Self {
        self.push_index(name, columns, true)
    }

    /// Add a unique index and let the caller set advanced metadata.
    pub fn unique_index_with(
        self,
        name: impl Into<String>,
        columns: &[&str],
        f: impl FnOnce(Index) -> Index,
    ) -> Self {
        self.push_index_with(name, columns, true, f)
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
                opaque: OpaqueMeta::default(),
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

    /// Add a fully constructed modeled table definition.
    pub fn table_def(mut self, table: Table) -> Self {
        let key = schema_qualified_key(&table.name, table.schema.as_deref());
        self.state.tables.insert(key, table);
        self
    }

    pub fn extension(self, name: impl Into<String>) -> Self {
        self.insert_extension(name, None)
    }

    pub fn extension_versioned(self, name: impl Into<String>, version: impl Into<String>) -> Self {
        self.insert_extension(name, Some(version.into()))
    }

    /// Add a fully constructed extension definition.
    pub fn extension_def(mut self, extension: ExtensionDef) -> Self {
        let key = schema_qualified_key(&extension.name, extension.schema.as_deref());
        self.state.extensions.insert(key, extension);
        self
    }

    pub fn view(mut self, name: impl Into<String>, definition: impl Into<String>) -> Self {
        let name = name.into();
        self.state.views.insert(
            name.clone(),
            ViewDef {
                name,
                schema: None,
                definition: definition.into(),
                opaque: OpaqueMeta::default(),
            },
        );
        self
    }

    /// Add a fully constructed view definition.
    pub fn view_def(mut self, view: ViewDef) -> Self {
        let key = schema_qualified_key(&view.name, view.schema.as_deref());
        self.state.views.insert(key, view);
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
                opaque: OpaqueMeta::default(),
            },
        );
        self
    }

    /// Add a fully constructed enum definition.
    pub fn enum_def(mut self, enum_def: EnumDef) -> Self {
        let key = schema_qualified_key(&enum_def.name, enum_def.schema.as_deref());
        self.state.enums.insert(key, enum_def);
        self
    }

    /// Build a schema through the same normalize and prepare lifecycle as file
    /// and SQL ingestion.
    pub fn build(self) -> Result<Schema, SchemaLoadError> {
        let dialect = self.dialect;
        self.build_raw().prepare_loaded(dialect)
    }

    /// Build a raw normalized schema without dialect preparation.
    ///
    /// This is intended for internal tests and low-level tooling that need to
    /// inspect pre-validation model state. User-facing builder paths should use
    /// [`SchemaBuilder::build`].
    pub(crate) fn build_raw(self) -> Schema {
        self.state
    }
}
