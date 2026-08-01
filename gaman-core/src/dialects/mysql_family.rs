//! Shared MySQL-family schema rules that are identical across supported products.

use crate::dialects::DialectError;
use crate::migrations::Migration;
use crate::operations::Operation;
use crate::states::{
    Column, ColumnDialectOptions, Constraint, ForeignKey, GeneratedStorage, Index, Schema,
    SchemaValidationError, Table,
};

#[derive(Clone, Copy)]
pub(super) enum FamilyFlavor {
    Mysql,
    Mariadb,
}

impl FamilyFlavor {
    fn name(self) -> &'static str {
        match self {
            Self::Mysql => "mysql",
            Self::Mariadb => "mariadb",
        }
    }

    fn type_key(self, value: &str) -> String {
        match self {
            Self::Mysql => super::mysql::type_compare::key(value),
            Self::Mariadb => super::mariadb::type_compare::key(value),
        }
    }
}

pub(super) fn migration_to_sql(
    flavor: FamilyFlavor,
    migration: &Migration,
    start: &Schema,
) -> Result<Vec<String>, DialectError> {
    validate_migration_with_state(migration, start, flavor)?;
    let mut sql = Vec::new();
    for operation in &migration.operations {
        sql.extend(operation_sql(operation, flavor)?);
    }
    Ok(sql)
}

pub(super) fn validate_schema(
    schema: &Schema,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    if !schema.extensions.is_empty() || !schema.enums.is_empty() {
        return Err(SchemaValidationError::Invalid(format!(
            "{} does not support top-level extensions or enums",
            flavor.name()
        )));
    }
    for table in schema.tables.values() {
        let auto_increment = table
            .columns
            .iter()
            .filter(|column| {
                family_options(column, flavor).is_some_and(|options| options.auto_increment)
            })
            .count();
        if auto_increment > 1 {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{}' has more than one auto-increment column",
                table.name
            )));
        }
        if let Some(column) = table.columns.iter().find(|column| {
            family_options(column, flavor).is_some_and(|options| options.auto_increment)
        }) && !column_is_indexed(table, &column.name)
        {
            return Err(SchemaValidationError::Invalid(format!(
                "auto-increment column '{}.{}' must be indexed",
                table.name, column.name
            )));
        }
        for column in &table.columns {
            validate_column(column, &table.name, flavor)?;
        }
        if table.foreign_keys.iter().any(|foreign_key| {
            foreign_key.on_delete.as_deref() == Some("set_default")
                || foreign_key.on_update.as_deref() == Some("set_default")
        }) {
            return Err(SchemaValidationError::Invalid(format!(
                "{} table '{}' uses unsupported SET DEFAULT referential action",
                flavor.name(),
                table.name
            )));
        }
        let invisible = table
            .columns
            .iter()
            .filter(|column| {
                family_options(column, flavor).is_some_and(|options| options.invisible)
            })
            .count();
        if !table.columns.is_empty() && invisible == table.columns.len() {
            return Err(SchemaValidationError::Invalid(format!(
                "table '{}' cannot make every column invisible",
                table.name
            )));
        }
        if table
            .indexes
            .iter()
            .any(|index| index.predicate.is_some() && !index.is_opaque())
        {
            return Err(SchemaValidationError::Invalid(format!(
                "{} does not support partial indexes on table '{}'",
                flavor.name(),
                table.name
            )));
        }
    }
    validate_modeled_keys(schema, flavor)?;
    Ok(())
}

/// Rejects modeled keys whose database syntax requires an unmodeled prefix length.
fn validate_modeled_keys(
    schema: &Schema,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    for table in schema.tables.values() {
        if let Some(primary_key) = &table.primary_key {
            validate_key_columns(table, &primary_key.columns, "primary key", flavor)?;
        }
        for index in &table.indexes {
            if !index.is_opaque() {
                validate_key_columns(
                    table,
                    &index.columns,
                    &format!("index '{}'", index.name),
                    flavor,
                )?;
            }
        }
        validate_constraint_keys(table, flavor)?;
        validate_foreign_key_columns(schema, table, flavor)?;
    }
    Ok(())
}

/// Validates unique constraints because their modeled shape also renders as an index.
fn validate_constraint_keys(
    table: &Table,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    for constraint in &table.constraints {
        if let Constraint::Unique { name, columns } = constraint {
            validate_key_columns(
                table,
                columns,
                &format!("unique constraint '{name}'"),
                flavor,
            )?;
        }
    }
    Ok(())
}

/// Validates both sides of foreign keys because MySQL-family databases index both identities.
fn validate_foreign_key_columns(
    schema: &Schema,
    table: &Table,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    for foreign_key in &table.foreign_keys {
        let label = format!("foreign key '{}'", foreign_key.name);
        validate_key_columns(table, &foreign_key.columns, &label, flavor)?;
        if let Some(target) = schema.tables.get(&foreign_key.to_table) {
            validate_key_columns(target, &foreign_key.to_columns, &label, flavor)?;
        }
    }
    Ok(())
}

/// Reports a precise validation failure for a modeled key over an unbounded value.
fn validate_key_columns(
    table: &Table,
    columns: &[String],
    key: &str,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    for name in columns {
        let Some(column) = table.columns.iter().find(|column| column.name == *name) else {
            continue;
        };
        if type_requires_index_prefix(&column.col_type, flavor) {
            return Err(SchemaValidationError::Invalid(format!(
                "{} table '{}' {key} uses column '{}' of type '{}', which requires an index prefix; modeled indexes cannot represent prefix lengths; use VARCHAR/VARBINARY or opaque raw SQL",
                flavor.name(),
                table.name,
                column.name,
                column.col_type
            )));
        }
    }
    Ok(())
}

/// Identifies native unbounded text and binary types from product canonical comparison keys.
fn type_requires_index_prefix(value: &str, flavor: FamilyFlavor) -> bool {
    let key = flavor.type_key(value);
    let base = key.split('\u{1f}').next().unwrap_or_default();
    matches!(
        base,
        "tinytext"
            | "text"
            | "mediumtext"
            | "longtext"
            | "tinyblob"
            | "blob"
            | "mediumblob"
            | "longblob"
    )
}

fn column_is_indexed(table: &Table, column: &str) -> bool {
    table.primary_key.as_ref().is_some_and(|key| key.columns.first().is_some_and(|name| name == column))
        || table.indexes.iter().any(|index| index.columns.first().is_some_and(|name| name == column))
        || table.constraints.iter().any(|constraint| matches!(constraint,
            Constraint::Unique { columns, .. } if columns.first().is_some_and(|name| name == column)))
}

fn validate_column(
    column: &Column,
    table: &str,
    flavor: FamilyFlavor,
) -> Result<(), SchemaValidationError> {
    let options = family_options(column, flavor).ok_or_else(|| {
        SchemaValidationError::Invalid(format!(
            "column '{table}.{}' has options for a different database product",
            column.name
        ))
    })?;
    if column.generated_storage.is_some() && column.generated.is_none() {
        return Err(SchemaValidationError::Invalid(format!(
            "column '{table}.{}' sets generated storage without a generated expression",
            column.name
        )));
    }
    if column.generated.is_some() && options.auto_increment {
        return Err(SchemaValidationError::Invalid(format!(
            "column '{table}.{}' cannot be generated and auto-increment",
            column.name
        )));
    }
    if options.invisible
        && !column.nullable
        && column.default.is_none()
        && column.generated.is_none()
    {
        return Err(SchemaValidationError::Invalid(format!(
            "invisible column '{table}.{}' must be nullable, generated, or have a default",
            column.name
        )));
    }
    if options.auto_increment && !is_auto_increment_type(&column.col_type) {
        return Err(SchemaValidationError::Invalid(format!(
            "auto-increment column '{table}.{}' must use an integer type",
            column.name
        )));
    }
    Ok(())
}

fn is_auto_increment_type(value: &str) -> bool {
    matches!(
        value
            .trim()
            .split(['(', ' '])
            .next()
            .unwrap_or("")
            .to_ascii_lowercase()
            .as_str(),
        "tinyint" | "smallint" | "mediumint" | "int" | "integer" | "bigint"
    )
}

pub(super) fn validate_migration(
    migration: &Migration,
    flavor: FamilyFlavor,
) -> Result<(), DialectError> {
    if migration.atomic
        && migration
            .operations
            .iter()
            .any(|operation| !matches!(operation, Operation::Statement { .. }))
    {
        return Err(DialectError::Unsupported(flavor.name().to_string(), "schema DDL implicitly commits; migrations containing modeled schema operations must set atomic: false".to_string()));
    }
    for operation in &migration.operations {
        if migration.atomic
            && let Operation::Statement { up, .. } = operation
        {
            validate_atomic_statement(up, flavor)?;
        }
        if matches!(
            operation,
            Operation::CreateExtension { .. }
                | Operation::DropExtension { .. }
                | Operation::CreateEnum { .. }
                | Operation::DropEnum { .. }
                | Operation::RenameEnumValue { .. }
                | Operation::AlterEnum { .. }
        ) {
            return Err(DialectError::Unsupported(
                operation.type_name().to_string(),
                format!("{} does not support this entity operation", flavor.name()),
            ));
        }
        if let Operation::AlterColumn { old, new, .. } = operation
            && old.generated_storage != new.generated_storage
            && old.generated.is_some()
            && new.generated.is_some()
        {
            return Err(DialectError::Unsupported(
                "alter_column".to_string(),
                format!(
                    "{} cannot change generated storage with MODIFY COLUMN; use an explicit drop/add migration",
                    flavor.name()
                ),
            ));
        }
        if matches!(
            operation,
            Operation::AddIndex {
                concurrent: true,
                ..
            } | Operation::DropIndex {
                concurrent: true,
                ..
            }
        ) {
            return Err(DialectError::Unsupported(
                operation.type_name().to_string(),
                "concurrent indexes are PostgreSQL-specific".to_string(),
            ));
        }
    }
    Ok(())
}

/// Validates migration policy and the modeled target schema before rendering SQL.
pub(super) fn validate_migration_with_state(
    migration: &Migration,
    start: &Schema,
    flavor: FamilyFlavor,
) -> Result<(), DialectError> {
    validate_migration(migration, flavor)?;
    let mut target = start.clone();
    crate::replay::ReplayEngine::apply_migration(&mut target, migration).map_err(|error| {
        DialectError::Unsupported(
            "migration".to_string(),
            format!("{} migration cannot be replayed: {error}", flavor.name()),
        )
    })?;
    validate_schema(&target, flavor).map_err(|error| {
        DialectError::Unsupported(
            "migration".to_string(),
            format!("{} target schema is invalid: {error}", flavor.name()),
        )
    })
}

/// Allows transactional raw migrations only when every segment is confidently DML.
fn validate_atomic_statement(sql: &str, flavor: FamilyFlavor) -> Result<(), DialectError> {
    let dialect = match flavor {
        FamilyFlavor::Mysql => crate::dialects::Dialect::Mysql,
        FamilyFlavor::Mariadb => crate::dialects::Dialect::Mariadb,
    };
    let segments = crate::parsers::segment_sql(sql, dialect)
        .map_err(|error| DialectError::Unsupported("statement".to_string(), error.to_string()))?;
    if segments
        .iter()
        .all(|segment| matches!(segment.kind, Some(crate::parsers::SqlStatementKind::Dml(_))))
    {
        Ok(())
    } else {
        Err(DialectError::Unsupported(
            "statement".to_string(),
            format!(
                "{} atomic raw migrations require confidently classified DML only",
                flavor.name()
            ),
        ))
    }
}

fn quote_ident(value: &str) -> String {
    format!("`{}`", value.replace('`', "``"))
}
fn quote_name(value: &str) -> String {
    value
        .split('.')
        .map(quote_ident)
        .collect::<Vec<_>>()
        .join(".")
}
fn quote_columns(values: &[String]) -> String {
    values
        .iter()
        .map(|value| quote_ident(value))
        .collect::<Vec<_>>()
        .join(", ")
}
fn trim_sql(value: &str) -> &str {
    value.trim().trim_end_matches(';').trim_end()
}

fn column_sql(column: &Column, flavor: FamilyFlavor) -> String {
    let options = family_options(column, flavor).unwrap_or_default();
    let mut sql = format!("{} {}", quote_ident(&column.name), column.col_type);
    if let Some(character_set) = options.character_set {
        sql.push_str(&format!(" CHARACTER SET {character_set}"));
    }
    if let Some(collation) = options.collation {
        sql.push_str(&format!(" COLLATE {collation}"));
    }
    if let Some(expression) = &column.generated {
        sql.push_str(&format!(" GENERATED ALWAYS AS ({expression})"));
        if let Some(storage) = column.generated_storage {
            let storage = match (flavor, storage) {
                (FamilyFlavor::Mariadb, GeneratedStorage::Stored) => "PERSISTENT",
                (_, GeneratedStorage::Stored) => "STORED",
                (_, GeneratedStorage::Virtual) => "VIRTUAL",
            };
            sql.push_str(&format!(" {storage}"));
        }
    } else if let Some(default) = &column.default {
        sql.push_str(&format!(" DEFAULT {default}"));
    }
    sql.push_str(if column.nullable {
        " NULL"
    } else {
        " NOT NULL"
    });
    if options.auto_increment {
        sql.push_str(" AUTO_INCREMENT");
    }
    if let Some(expression) = options.on_update_expression {
        sql.push_str(&format!(" ON UPDATE {expression}"));
    }
    if options.invisible {
        sql.push_str(" INVISIBLE");
    }
    if let Some(comment) = options.comment {
        sql.push_str(&format!(" COMMENT '{}'", comment.replace('\'', "''")));
    }
    sql
}

#[derive(Clone, Copy, Default)]
struct FamilyColumnOptions<'a> {
    auto_increment: bool,
    on_update_expression: Option<&'a String>,
    character_set: Option<&'a String>,
    collation: Option<&'a String>,
    invisible: bool,
    comment: Option<&'a String>,
}

/// Selects only metadata belonging to the active product.
fn family_options(column: &Column, flavor: FamilyFlavor) -> Option<FamilyColumnOptions<'_>> {
    match (&column.dialect_options, flavor) {
        (options, _) if options.mysql.is_none() && options.mariadb.is_none() => {
            Some(Default::default())
        }
        (
            ColumnDialectOptions {
                mysql: Some(mysql),
                mariadb: None,
            },
            FamilyFlavor::Mysql,
        ) => Some(FamilyColumnOptions {
            auto_increment: mysql.auto_increment,
            on_update_expression: mysql.on_update_expression.as_ref(),
            character_set: mysql.character_set.as_ref(),
            collation: mysql.collation.as_ref(),
            invisible: mysql.invisible,
            comment: mysql.comment.as_ref(),
        }),
        (
            ColumnDialectOptions {
                mysql: None,
                mariadb: Some(mariadb),
            },
            FamilyFlavor::Mariadb,
        ) => Some(FamilyColumnOptions {
            auto_increment: mariadb.auto_increment,
            on_update_expression: mariadb.on_update_expression.as_ref(),
            character_set: mariadb.character_set.as_ref(),
            collation: mariadb.collation.as_ref(),
            invisible: mariadb.invisible,
            comment: mariadb.comment.as_ref(),
        }),
        _ => None,
    }
}

/// Carries unpinned catalog metadata into a full MySQL-family repair definition.
pub(super) fn column_for_repair(
    expected: &Column,
    observed: &Column,
    flavor: FamilyFlavor,
) -> Column {
    let mut repaired = expected.clone();
    match flavor {
        FamilyFlavor::Mysql => {
            let observed = observed.dialect_options.mysql.as_ref();
            let expected = repaired.dialect_options.mysql.get_or_insert_default();
            preserve_optional_column_metadata(expected, observed);
        }
        FamilyFlavor::Mariadb => {
            let observed = observed.dialect_options.mariadb.as_ref();
            let expected = repaired.dialect_options.mariadb.get_or_insert_default();
            preserve_optional_column_metadata(expected, observed);
        }
    }
    repaired
}

/// Preserves unpinned charset, collation, and comments from the observed definition.
fn preserve_optional_column_metadata<T>(expected: &mut T, observed: Option<&T>)
where
    T: OptionalColumnMetadata,
{
    let Some(observed) = observed else {
        return;
    };
    expected.preserve_from(observed);
}

/// Abstracts the optional metadata shared by MySQL-family column option structs.
trait OptionalColumnMetadata {
    fn preserve_from(&mut self, observed: &Self);
}

impl OptionalColumnMetadata for crate::states::MysqlColumnOptions {
    fn preserve_from(&mut self, observed: &Self) {
        if self.character_set.is_none() {
            self.character_set = observed.character_set.clone();
        }
        if self.collation.is_none() {
            self.collation = observed.collation.clone();
        }
        if self.comment.is_none() {
            self.comment = observed.comment.clone();
        }
    }
}

impl OptionalColumnMetadata for crate::states::MariadbColumnOptions {
    fn preserve_from(&mut self, observed: &Self) {
        if self.character_set.is_none() {
            self.character_set = observed.character_set.clone();
        }
        if self.collation.is_none() {
            self.collation = observed.collation.clone();
        }
        if self.comment.is_none() {
            self.comment = observed.comment.clone();
        }
    }
}

fn foreign_key_sql(foreign_key: &ForeignKey) -> Result<String, DialectError> {
    let mut sql = format!(
        "CONSTRAINT {} FOREIGN KEY ({}) REFERENCES {} ({})",
        quote_ident(&foreign_key.name),
        quote_columns(&foreign_key.columns),
        quote_name(&foreign_key.to_table),
        quote_columns(&foreign_key.to_columns)
    );
    if let Some(action) = &foreign_key.on_delete {
        sql.push_str(&format!(" ON DELETE {}", action_sql(action)?));
    }
    if let Some(action) = &foreign_key.on_update {
        sql.push_str(&format!(" ON UPDATE {}", action_sql(action)?));
    }
    Ok(sql)
}

fn action_sql(action: &str) -> Result<&'static str, DialectError> {
    match action {
        "cascade" => Ok("CASCADE"),
        "restrict" => Ok("RESTRICT"),
        "set_null" => Ok("SET NULL"),
        "set_default" => Err(DialectError::Unsupported(
            "foreign_key".to_string(),
            "MySQL and MariaDB do not support SET DEFAULT referential actions".to_string(),
        )),
        value => Err(DialectError::Unsupported(
            "foreign_key".to_string(),
            format!("unsupported referential action '{value}'"),
        )),
    }
}

fn constraint_sql(constraint: &Constraint) -> String {
    match constraint {
        Constraint::Unique { name, columns } => format!(
            "CONSTRAINT {} UNIQUE ({})",
            quote_ident(name),
            quote_columns(columns)
        ),
        Constraint::Check { name, expression } => {
            format!("CONSTRAINT {} CHECK ({expression})", quote_ident(name))
        }
        Constraint::Opaque { .. } => constraint.raw_sql().map(trim_sql).unwrap_or("").to_string(),
    }
}

fn create_table_sql(table: &Table, flavor: FamilyFlavor) -> Result<Vec<String>, DialectError> {
    let mut definitions = table
        .columns
        .iter()
        .map(|column| column_sql(column, flavor))
        .collect::<Vec<_>>();
    if let Some(pk) = &table.primary_key {
        definitions.push(format!(
            "CONSTRAINT {} PRIMARY KEY ({})",
            quote_ident(&pk.name),
            quote_columns(&pk.columns)
        ));
    }
    for foreign_key in &table.foreign_keys {
        definitions.push(foreign_key_sql(foreign_key)?);
    }
    definitions.extend(table.constraints.iter().map(constraint_sql));
    let header = if table.options.header_raw.is_empty() {
        String::new()
    } else {
        format!("{} ", table.options.header_raw.join(" "))
    };
    let tail = if table.options.tail_raw.is_empty() {
        String::new()
    } else {
        format!(" {}", table.options.tail_raw.join(" "))
    };
    let mut sql = vec![format!(
        "CREATE {header}TABLE {} ({}){tail}",
        quote_name(&table.qualified_name()),
        definitions.join(", ")
    )];
    for index in &table.indexes {
        sql.push(create_index_sql(&table.qualified_name(), index)?);
    }
    Ok(sql)
}

fn create_index_sql(table: &str, index: &Index) -> Result<String, DialectError> {
    if let Some(raw) = index.raw_sql() {
        return Ok(trim_sql(raw).to_string());
    }
    if index.predicate.is_some() {
        return Err(DialectError::Unsupported(
            "index".to_string(),
            "MySQL-family databases do not support partial indexes".to_string(),
        ));
    }
    Ok(format!(
        "CREATE {}INDEX {} ON {} ({})",
        if index.unique { "UNIQUE " } else { "" },
        quote_ident(&index.name),
        quote_name(table),
        quote_columns(&index.columns)
    ))
}

fn operation_sql(operation: &Operation, flavor: FamilyFlavor) -> Result<Vec<String>, DialectError> {
    crate::migration_normalize::ensure_operation_is_canonical(operation).map_err(|error| {
        DialectError::Unsupported("migration invariant".to_string(), error.to_string())
    })?;
    match operation {
        Operation::CreateTable { table } => create_table_sql(table, flavor),
        Operation::DropTable { table } => Ok(vec![format!(
            "DROP TABLE {}",
            quote_name(&table.qualified_name())
        )]),
        Operation::RenameTable { old_name, new_name } => Ok(vec![format!(
            "RENAME TABLE {} TO {}",
            quote_name(old_name),
            quote_name(new_name)
        )]),
        Operation::AcknowledgeTableOptions { .. } => Ok(Vec::new()),
        Operation::AddColumn { table_name, column } => Ok(vec![format!(
            "ALTER TABLE {} ADD COLUMN {}",
            quote_name(table_name),
            column_sql(column, flavor)
        )]),
        Operation::DropColumn {
            table_name, column, ..
        } => Ok(vec![format!(
            "ALTER TABLE {} DROP COLUMN {}",
            quote_name(table_name),
            quote_ident(&column.name)
        )]),
        Operation::RenameColumn {
            table_name,
            old_name,
            new_name,
        } => Ok(vec![format!(
            "ALTER TABLE {} RENAME COLUMN {} TO {}",
            quote_name(table_name),
            quote_ident(old_name),
            quote_ident(new_name)
        )]),
        Operation::AlterColumn {
            table_name, new, ..
        } => Ok(vec![format!(
            "ALTER TABLE {} MODIFY COLUMN {}",
            quote_name(table_name),
            column_sql(new, flavor)
        )]),
        Operation::AddForeignKey {
            table_name,
            foreign_key,
        } => Ok(vec![format!(
            "ALTER TABLE {} ADD {}",
            quote_name(table_name),
            foreign_key_sql(foreign_key)?
        )]),
        Operation::DropForeignKey {
            table_name,
            foreign_key,
            ..
        } => Ok(vec![format!(
            "ALTER TABLE {} DROP FOREIGN KEY {}",
            quote_name(table_name),
            quote_ident(&foreign_key.name)
        )]),
        Operation::AddIndex {
            table_name, index, ..
        } => Ok(vec![create_index_sql(table_name, index)?]),
        Operation::DropIndex {
            table_name, index, ..
        } => Ok(vec![format!(
            "DROP INDEX {} ON {}",
            quote_ident(&index.name),
            quote_name(table_name)
        )]),
        Operation::AddConstraint {
            table_name,
            constraint,
        } => Ok(vec![format!(
            "ALTER TABLE {} ADD {}",
            quote_name(table_name),
            constraint_sql(constraint)
        )]),
        Operation::DropConstraint {
            table_name,
            constraint,
        } => Ok(vec![format!(
            "ALTER TABLE {} DROP {}",
            quote_name(table_name),
            if matches!(constraint, Constraint::Unique { .. }) {
                format!("INDEX {}", quote_ident(constraint.name()))
            } else {
                format!("CHECK {}", quote_ident(constraint.name()))
            }
        )]),
        Operation::Statement { up, .. } => Ok(vec![up.clone()]),
        Operation::CreateFunction { function } => raw_create(function.raw_sql(), "function"),
        Operation::DropFunction { function } => Ok(vec![format!(
            "DROP FUNCTION {}",
            quote_name(&function.qualified_name())
        )]),
        Operation::AlterFunction { old, new } => {
            let mut sql = vec![format!(
                "DROP FUNCTION {}",
                quote_name(&old.qualified_name())
            )];
            sql.extend(raw_create(new.raw_sql(), "function")?);
            Ok(sql)
        }
        Operation::CreateTrigger { trigger, .. } => raw_create(trigger.raw_sql(), "trigger"),
        Operation::AlterTrigger { old, new, .. } => {
            let mut sql = vec![format!(
                "DROP TRIGGER {}",
                quote_ident(old.name.as_deref().unwrap_or(""))
            )];
            sql.extend(raw_create(new.raw_sql(), "trigger")?);
            Ok(sql)
        }
        Operation::DropTrigger { trigger, .. } => Ok(vec![format!(
            "DROP TRIGGER {}",
            quote_ident(trigger.name.as_deref().unwrap_or(""))
        )]),
        Operation::CreateView { view } => {
            if let Some(raw) = view.raw_sql() {
                Ok(vec![trim_sql(raw).to_string()])
            } else {
                Ok(vec![format!(
                    "CREATE VIEW {} AS {}",
                    quote_name(&view.qualified_name()),
                    view.definition
                )])
            }
        }
        Operation::DropView { view } => Ok(vec![format!(
            "DROP VIEW {}",
            quote_name(&view.qualified_name())
        )]),
        Operation::ReplaceView { new, .. } => {
            if let Some(raw) = new.raw_sql() {
                Ok(vec![trim_sql(raw).to_string()])
            } else {
                Ok(vec![format!(
                    "CREATE OR REPLACE VIEW {} AS {}",
                    quote_name(&new.qualified_name()),
                    new.definition
                )])
            }
        }
        _ => Err(DialectError::Unsupported(
            operation.type_name().to_string(),
            format!("{} does not support this operation", flavor.name()),
        )),
    }
}

fn raw_create(raw: Option<&str>, kind: &str) -> Result<Vec<String>, DialectError> {
    raw.map(|sql| vec![trim_sql(sql).to_string()]).ok_or_else(|| DialectError::Unsupported(format!("create_{kind}"), format!("modeled {kind} rendering is not supported; use SQL input so the definition is preserved as opaque source")))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies MySQL rendering preserves modeled auto-increment and update metadata.
    #[test]
    fn mysql_create_table_renders_family_column_metadata() {
        let schema = crate::parsers::parse_sql("CREATE TABLE users (id BIGINT AUTO_INCREMENT PRIMARY KEY, updated_at TIMESTAMP ON UPDATE CURRENT_TIMESTAMP)", crate::dialects::Dialect::Mysql).expect("parse mysql table");
        let migration = Migration {
            id: "0001_users".to_string(),
            dependencies: Vec::new(),
            operations: vec![Operation::CreateTable {
                table: schema.tables["users"].clone(),
            }],
            atomic: false,
        };
        let sql = migration_to_sql(FamilyFlavor::Mysql, &migration, &Schema::default())
            .expect("render mysql table")
            .join("\n");
        assert!(sql.contains("AUTO_INCREMENT"));
        assert!(sql.contains("ON UPDATE CURRENT_TIMESTAMP"));
    }

    /// Verifies MariaDB renders stored generated columns with its persistent spelling.
    #[test]
    fn mariadb_generated_storage_renders_persistent() {
        let schema = crate::parsers::parse_sql(
            "CREATE TABLE metrics (base_value INT, doubled INT AS (base_value * 2) STORED)",
            crate::dialects::Dialect::Mariadb,
        )
        .expect("parse mariadb table");
        let migration = Migration {
            id: "0001_metrics".to_string(),
            dependencies: Vec::new(),
            operations: vec![Operation::CreateTable {
                table: schema.tables["metrics"].clone(),
            }],
            atomic: false,
        };
        let sql = migration_to_sql(FamilyFlavor::Mariadb, &migration, &Schema::default())
            .expect("render mariadb table")
            .join("\n");
        assert!(sql.contains("PERSISTENT"));
    }

    /// Verifies modeled family DDL cannot claim multi-statement transaction atomicity.
    #[test]
    fn family_schema_migration_rejects_atomic_true() {
        let migration = Migration {
            id: "0001_users".to_string(),
            dependencies: Vec::new(),
            operations: vec![Operation::DropTable {
                table: Table {
                    name: "users".to_string(),
                    schema: None,
                    primary_key: None,
                    columns: Vec::new(),
                    foreign_keys: Vec::new(),
                    indexes: Vec::new(),
                    constraints: Vec::new(),
                    triggers: Vec::new(),
                    options: Default::default(),
                },
            }],
            atomic: true,
        };
        let error = validate_migration(&migration, FamilyFlavor::Mysql)
            .expect_err("atomic family DDL must fail");
        assert!(error.to_string().contains("implicitly commits"));
    }

    /// Verifies every unbounded text and binary family type is rejected in modeled indexes.
    #[test]
    fn family_rejects_modeled_indexes_that_require_prefixes() {
        let types = [
            "TINYTEXT",
            "TEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "TINYBLOB",
            "BLOB",
            "MEDIUMBLOB",
            "LONGBLOB",
        ];
        for dialect in [
            crate::dialects::Dialect::Mysql,
            crate::dialects::Dialect::Mariadb,
        ] {
            for col_type in types {
                let sql = format!("CREATE TABLE documents (body {col_type})");
                let mut schema =
                    crate::parsers::parse_sql(&sql, dialect).expect("parse unindexed table");
                let table = schema.tables.get_mut("documents").expect("documents table");
                table.indexes.push(Index {
                    name: "documents_body_idx".to_string(),
                    columns: vec!["body".to_string()],
                    unique: false,
                    predicate: None,
                    opaque: Default::default(),
                });
                let flavor = if dialect == crate::dialects::Dialect::Mysql {
                    FamilyFlavor::Mysql
                } else {
                    FamilyFlavor::Mariadb
                };
                let error = validate_schema(&schema, flavor)
                    .expect_err("unbounded modeled index must fail");
                assert!(error.to_string().contains("requires an index prefix"));
            }
        }
    }

    /// Verifies primary, unique, and foreign-key identities reject unbounded key columns.
    #[test]
    fn family_rejects_unbounded_constraint_keys() {
        let statements = [
            "CREATE TABLE documents (body TEXT PRIMARY KEY)",
            "CREATE TABLE documents (body TEXT, CONSTRAINT documents_body_key UNIQUE (body))",
            "CREATE TABLE parents (id VARCHAR(255) PRIMARY KEY); CREATE TABLE children (parent_id TEXT, CONSTRAINT children_parent_fk FOREIGN KEY (parent_id) REFERENCES parents(id))",
        ];
        for dialect in [
            crate::dialects::Dialect::Mysql,
            crate::dialects::Dialect::Mariadb,
        ] {
            for sql in statements {
                let error = crate::parsers::parse_sql(sql, dialect)
                    .expect_err("unbounded modeled constraint key must fail");
                assert!(error.to_string().contains("requires an index prefix"));
            }
        }
    }

    /// Verifies bounded modeled indexes remain valid for both family products.
    #[test]
    fn family_accepts_bounded_modeled_indexes() {
        let sql = "CREATE TABLE documents (body VARCHAR(255), INDEX documents_body_idx (body), CONSTRAINT documents_body_key UNIQUE (body))";
        for dialect in [
            crate::dialects::Dialect::Mysql,
            crate::dialects::Dialect::Mariadb,
        ] {
            crate::parsers::parse_sql(sql, dialect).expect("bounded modeled index must pass");
        }
    }

    /// Verifies direct SQL rendering cannot bypass target-schema key validation.
    #[test]
    fn family_rendering_validates_replayed_target_schema() {
        let mut schema = crate::parsers::parse_sql(
            "CREATE TABLE documents (body VARCHAR(255))",
            crate::dialects::Dialect::Mysql,
        )
        .expect("parse bounded table");
        let table = schema.tables.get_mut("documents").expect("documents table");
        table.columns[0].col_type = "TEXT".to_string();
        table.indexes.push(Index {
            name: "documents_body_idx".to_string(),
            columns: vec!["body".to_string()],
            unique: false,
            predicate: None,
            opaque: Default::default(),
        });
        let migration = Migration {
            id: "0001_documents".to_string(),
            dependencies: Vec::new(),
            operations: vec![Operation::CreateTable {
                table: schema.tables["documents"].clone(),
            }],
            atomic: false,
        };
        let error = migration_to_sql(FamilyFlavor::Mysql, &migration, &Schema::default())
            .expect_err("rendering must validate target schema");
        assert!(error.to_string().contains("requires an index prefix"));
    }

    /// Verifies raw prefix indexes remain available through the opaque lifecycle.
    #[test]
    fn family_accepts_opaque_prefix_indexes() {
        let schema = crate::parsers::parse_sql(
            "CREATE TABLE documents (body TEXT); CREATE INDEX documents_body_idx ON documents (body(32))",
            crate::dialects::Dialect::Mysql,
        )
        .expect("opaque prefix index must pass");
        let index = &schema.tables["documents"].indexes[0];
        assert!(index.is_opaque());
    }
}
