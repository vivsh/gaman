use super::{
    Column, ColumnRef, Constraint, EnumDef, ExtensionDef, ForeignKey, FunctionDef,
    FunctionParameter, Index, OpaqueMeta, PrimaryKey, Schema, SchemaBuilderIssue, SchemaLoadError,
    Table, TableOptionsMeta, TriggerDef, ViewDef, names, parse_qualified_name,
    schema_qualified_key,
};
use crate::column_type::ColumnType;
use crate::dialects::Dialect;
use crate::parsers::{OpaqueDeclaration, opaque_parse_reason, parse_opaque_create};
use serde::Serialize;

/// Converts either a complete function definition or a fluent declaration into schema state.
pub trait IntoFunctionDef {
    /// Records declaration issues and returns the function value to register.
    fn into_function_def(self, issues: &mut Vec<SchemaBuilderIssue>) -> FunctionDef;
}

impl IntoFunctionDef for FunctionDef {
    fn into_function_def(self, _: &mut Vec<SchemaBuilderIssue>) -> FunctionDef {
        self
    }
}

/// Fluent, typed declaration for a modeled stored function.
pub struct FunctionBuilder {
    function: FunctionDef,
    issues: Vec<String>,
}

impl FunctionBuilder {
    /// Starts a function declaration with an unqualified or schema-qualified name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            function: FunctionDef {
                name: name.into(),
                schema: None,
                parameters: Vec::new(),
                arguments: String::new(),
                returns: String::new(),
                language: String::new(),
                body: String::new(),
                depends_on: Vec::new(),
                volatility: Default::default(),
                security_definer: false,
                opaque: OpaqueMeta::default(),
            },
            issues: Vec::new(),
        }
    }

    /// Sets the PostgreSQL schema containing the function.
    pub fn schema(mut self, schema: impl Into<String>) -> Self {
        self.function.schema = Some(schema.into());
        self
    }

    /// Adds one named parameter with an SQL type.
    pub fn parameter(mut self, name: impl Into<String>, type_name: impl Into<String>) -> Self {
        self.function.parameters.push(FunctionParameter {
            name: name.into(),
            type_name: type_name.into(),
            default: None,
        });
        self
    }

    /// Adds one named parameter with a raw SQL default expression.
    pub fn parameter_default(
        mut self,
        name: impl Into<String>,
        type_name: impl Into<String>,
        default: impl Into<String>,
    ) -> Self {
        self.function.parameters.push(FunctionParameter {
            name: name.into(),
            type_name: type_name.into(),
            default: Some(default.into()),
        });
        self
    }

    /// Sets the SQL return type.
    pub fn returns(mut self, returns: impl Into<String>) -> Self {
        self.function.returns = returns.into();
        self
    }

    /// Sets the function language.
    pub fn language(mut self, language: impl Into<String>) -> Self {
        self.function.language = language.into();
        self
    }

    /// Sets the executable function body.
    pub fn body(mut self, body: impl Into<String>) -> Self {
        self.function.body = body.into();
        self
    }

    /// Registers explicit root-entity dependencies using canonical selectors.
    pub fn depends_on<I, S>(mut self, dependencies: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        for value in dependencies {
            match crate::EntitySelector::parse_dependency(value.as_ref()) {
                Ok(dependency) => self.function.depends_on.push(dependency),
                Err(reason) => self.issues.push(reason),
            }
        }
        self
    }
}

impl IntoFunctionDef for FunctionBuilder {
    fn into_function_def(self, issues: &mut Vec<SchemaBuilderIssue>) -> FunctionDef {
        let identity = self.function.qualified_name();
        issues.extend(self.issues.into_iter().map(|reason| {
            SchemaBuilderIssue::InvalidFunctionDefinition {
                function: identity.clone(),
                reason,
            }
        }));
        self.function
    }
}

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

    /// Set the storage behavior for a generated column.
    pub fn generated_storage(mut self, storage: crate::states::GeneratedStorage) -> Self {
        self.col.generated_storage = Some(storage);
        self
    }

    /// Configures MySQL-only column properties.
    pub fn mysql(
        mut self,
        configure: impl FnOnce(crate::states::MysqlColumnOptions) -> crate::states::MysqlColumnOptions,
    ) -> Self {
        self.col.dialect_options.mysql = Some(configure(Default::default()));
        self
    }

    /// Configures MariaDB-only column properties.
    pub fn mariadb(
        mut self,
        configure: impl FnOnce(
            crate::states::MariadbColumnOptions,
        ) -> crate::states::MariadbColumnOptions,
    ) -> Self {
        self.col.dialect_options.mariadb = Some(configure(Default::default()));
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
    /// Continues fluent construction from an existing modeled table.
    fn from_table(table: Table) -> Self {
        Self { table }
    }

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

    /// Adds a modeled index to this table.
    ///
    /// Advanced index SQL belongs to [`SchemaBuilder::opaque`], which preserves
    /// its ownership and lifecycle without pretending it is a modeled index.
    pub fn index(mut self, index: Index) -> Self {
        self.table.indexes.push(index);
        self
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

    /// Adds an unsupported named table constraint as an untrusted definition clause.
    pub fn opaque_constraint(
        mut self,
        name: impl Into<String>,
        definition_clause: impl Into<String>,
    ) -> Self {
        self.table
            .constraints
            .push(Constraint::from_raw(name, definition_clause));
        self
    }

    /// Adds raw syntax rendered between CREATE and TABLE while keeping the table body modeled.
    pub fn unmanaged_prefix(mut self, clause: impl Into<String>) -> Self {
        let mut header = self.table.options.header_raw.clone();
        header.push(clause.into());
        self.replace_unmanaged_options(header, self.table.options.tail_raw.clone());
        self
    }

    /// Adds raw syntax rendered after the modeled CREATE TABLE body.
    pub fn unmanaged_suffix(mut self, clause: impl Into<String>) -> Self {
        let mut tail = self.table.options.tail_raw.clone();
        tail.push(clause.into());
        self.replace_unmanaged_options(self.table.options.header_raw.clone(), tail);
        self
    }

    /// Recomputes unmanaged-option trust and fingerprint without losing modeled partition metadata.
    fn replace_unmanaged_options(&mut self, header: Vec<String>, tail: Vec<String>) {
        let partition = self.table.options.postgres_partition.clone();
        self.table.options = TableOptionsMeta::from_parts(header, tail);
        self.table.options.postgres_partition = partition;
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
    issues: Vec<SchemaBuilderIssue>,
    opaque_declarations: Vec<OpaqueDeclaration>,
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
            issues: Vec::new(),
            opaque_declarations: Vec::new(),
        }
    }

    /// Continues fluent schema construction from an existing modeled schema.
    pub fn from_schema(dialect: Dialect, state: Schema) -> Self {
        Self {
            dialect,
            state,
            issues: Vec::new(),
            opaque_declarations: Vec::new(),
        }
    }

    /// Adds one opaque CREATE statement through the same fallback used by SQL ingestion.
    pub fn opaque(mut self, create_sql: impl Into<String>) -> Self {
        let create_sql = create_sql.into();
        match parse_opaque_create(&create_sql, self.dialect) {
            Ok(declaration) => self.opaque_declarations.push(declaration),
            Err(error) => self
                .issues
                .push(SchemaBuilderIssue::InvalidOpaqueDefinition {
                    kind: "entity".to_string(),
                    entity: "CREATE statement".to_string(),
                    reason: opaque_parse_reason(&error),
                }),
        }
        self
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

    /// Adds top-level managed rows independently from their target table declaration order.
    pub fn managed_rows<T: Serialize>(
        mut self,
        table: impl Into<String>,
        rows: impl IntoIterator<Item = T>,
    ) -> Self {
        let table = table.into();
        match crate::managed_rows::ManagedRows::from_serializable(rows) {
            Ok(incoming) => match self.state.managed_rows.get_mut(&table) {
                Some(existing) => {
                    if let Err(reason) =
                        crate::managed_rows::merge_declaration(&table, existing, incoming)
                    {
                        self.issues
                            .push(SchemaBuilderIssue::InvalidManagedRows { table, reason });
                    }
                }
                None => {
                    self.state.managed_rows.insert(table, incoming);
                }
            },
            Err(reason) => self
                .issues
                .push(SchemaBuilderIssue::InvalidManagedRows { table, reason }),
        }
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

    /// Additively extends an existing table without permitting identity changes.
    pub fn extend_table(
        mut self,
        name: impl Into<String>,
        configure: impl FnOnce(TableBuilder) -> TableBuilder,
    ) -> Self {
        let Some((name, schema)) = self.qualified_name(name.into()) else {
            return self;
        };
        self.extend_table_key(schema_qualified_key(&name, schema.as_deref()), configure)
    }

    /// Parses one public builder identity while accumulating malformed input.
    fn qualified_name(&mut self, source: String) -> Option<(String, Option<String>)> {
        match parse_qualified_name(self.dialect, &source) {
            Ok(identity) => Some(identity),
            Err(reason) => {
                self.issues.push(SchemaBuilderIssue::InvalidQualifiedName {
                    name: source,
                    reason,
                });
                None
            }
        }
    }

    /// Applies one table extension while retaining deterministic builder failures.
    fn extend_table_key(
        mut self,
        key: String,
        configure: impl FnOnce(TableBuilder) -> TableBuilder,
    ) -> Self {
        let Some(table) = self.state.tables.remove(&key) else {
            self.issues
                .push(SchemaBuilderIssue::MissingTable { table: key });
            return self;
        };
        let expected = schema_qualified_key(&table.name, table.schema.as_deref());
        let table = configure(TableBuilder::from_table(table)).build();
        let observed = schema_qualified_key(&table.name, table.schema.as_deref());
        if expected != observed {
            self.issues.push(SchemaBuilderIssue::TableIdentityChanged {
                expected: expected.clone(),
                observed,
            });
        }
        self.state.tables.insert(expected, table);
        self
    }

    /// Add a fully constructed view definition.
    pub fn view_def(mut self, view: ViewDef) -> Self {
        let key = schema_qualified_key(&view.name, view.schema.as_deref());
        self.state.views.insert(key, view);
        self
    }

    /// Adds a complete function definition or a [`FunctionBuilder`] declaration.
    pub fn function<F: IntoFunctionDef>(mut self, function: F) -> Self {
        let f = function.into_function_def(&mut self.issues);
        let key = f.identity_key();
        if self.state.functions.insert(key.clone(), f).is_some() {
            self.issues.push(SchemaBuilderIssue::DuplicateEntity {
                kind: "function".to_string(),
                entity: key,
            });
        }
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
        let Self {
            dialect,
            mut state,
            mut issues,
            opaque_declarations,
        } = self;
        apply_opaque_declarations(&mut state, &mut issues, opaque_declarations);
        state.prepare_loaded_with_issues(dialect, issues)
    }
}

/// Merges parser-classified opaque declarations after all modeled tables are available.
fn apply_opaque_declarations(
    state: &mut Schema,
    issues: &mut Vec<SchemaBuilderIssue>,
    declarations: Vec<OpaqueDeclaration>,
) {
    for declaration in declarations {
        apply_opaque_declaration(state, issues, declaration);
    }
}

fn apply_opaque_declaration(
    state: &mut Schema,
    issues: &mut Vec<SchemaBuilderIssue>,
    declaration: OpaqueDeclaration,
) {
    match declaration {
        OpaqueDeclaration::Index { table, value } => {
            let Some(owner) = state.tables.get_mut(&table) else {
                issues.push(SchemaBuilderIssue::MissingTable { table });
                return;
            };
            insert_owned_index(owner, issues, &table, value);
        }
        OpaqueDeclaration::Trigger { table, value } => {
            let Some(owner) = state.tables.get_mut(&table) else {
                issues.push(SchemaBuilderIssue::MissingTable { table });
                return;
            };
            insert_owned_trigger(owner, issues, &table, value);
        }
        OpaqueDeclaration::Function { key, value } => {
            insert_root(&mut state.functions, issues, "function", key, value)
        }
        OpaqueDeclaration::View { key, value } => {
            insert_root(&mut state.views, issues, "view", key, value)
        }
        OpaqueDeclaration::Extension { key, value } => {
            insert_root(&mut state.extensions, issues, "extension", key, value)
        }
        OpaqueDeclaration::Sequence { key, value } => {
            insert_root(&mut state.sequences, issues, "sequence", key, value)
        }
        OpaqueDeclaration::Enum { key, value } => {
            insert_root(&mut state.enums, issues, "enum", key, value)
        }
    }
}

fn insert_root<T>(
    entities: &mut std::collections::BTreeMap<String, T>,
    issues: &mut Vec<SchemaBuilderIssue>,
    kind: &str,
    key: String,
    value: T,
) {
    match entities.entry(key) {
        std::collections::btree_map::Entry::Vacant(entry) => {
            entry.insert(value);
        }
        std::collections::btree_map::Entry::Occupied(entry) => {
            issues.push(SchemaBuilderIssue::DuplicateEntity {
                kind: kind.to_string(),
                entity: entry.key().clone(),
            });
        }
    }
}

fn insert_owned_index(
    table: &mut Table,
    issues: &mut Vec<SchemaBuilderIssue>,
    table_key: &str,
    index: Index,
) {
    if table
        .indexes
        .iter()
        .any(|current| current.name == index.name)
    {
        issues.push(SchemaBuilderIssue::DuplicateEntity {
            kind: "index".to_string(),
            entity: format!("{table_key}.{}", index.name),
        });
    } else {
        table.indexes.push(index);
    }
}

fn insert_owned_trigger(
    table: &mut Table,
    issues: &mut Vec<SchemaBuilderIssue>,
    table_key: &str,
    trigger: TriggerDef,
) {
    let name = trigger.name.clone().unwrap_or_default();
    if table
        .triggers
        .iter()
        .any(|current| current.name == trigger.name)
    {
        issues.push(SchemaBuilderIssue::DuplicateEntity {
            kind: "trigger".to_string(),
            entity: format!("{table_key}.{name}"),
        });
    } else {
        table.triggers.push(trigger);
    }
}
