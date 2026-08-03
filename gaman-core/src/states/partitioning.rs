use std::collections::BTreeSet;

use super::{
    PostgresPartitionMeta, PostgresRangePartition, PostgresRangePartitioning, Schema,
    SchemaValidationError, Table, TableOptionsMeta, schema_qualified_key,
};

impl Schema {
    /// Extends a modeled table with PostgreSQL range partition metadata.
    ///
    /// The operation is atomic: invalid metadata or a child-name collision
    /// leaves the schema unchanged. Child names are unqualified and inherit the
    /// parent's schema.
    pub fn with_postgres_range_partitioning(
        mut self,
        table: &str,
        definition: PostgresRangePartitioning,
    ) -> Result<Self, SchemaValidationError> {
        self.add_postgres_range_partitioning(table, definition)?;
        Ok(self)
    }

    /// Registers PostgreSQL range partitions on an existing modeled table.
    pub fn add_postgres_range_partitioning(
        &mut self,
        table: &str,
        definition: PostgresRangePartitioning,
    ) -> Result<(), SchemaValidationError> {
        let parent_key = resolve_parent_key(self, table)?;
        validate_registration(self, &parent_key, &definition)?;
        register_partition_tables(self, &parent_key, definition)
    }
}

/// Rejects PostgreSQL-only partition metadata for another dialect.
pub(crate) fn reject_postgres_range_partitioning(
    schema: &Schema,
    dialect: &str,
) -> Result<(), SchemaValidationError> {
    let Some(table) = schema
        .tables
        .values()
        .find(|table| table.options.postgres_partition.is_some())
    else {
        return Ok(());
    };
    Err(SchemaValidationError::UnsupportedDialectFeature {
        dialect: dialect.to_string(),
        feature: "PostgreSQL range partitioning".to_string(),
        table: table.qualified_name(),
    })
}

/// Validates the complete PostgreSQL parent/child partition graph.
pub(crate) fn validate_postgres_range_partitioning(
    schema: &Schema,
) -> Result<(), SchemaValidationError> {
    for table in schema.tables.values() {
        match &table.options.postgres_partition {
            Some(PostgresPartitionMeta::Parent { column }) => {
                validate_parent(schema, table, column)?;
            }
            Some(PostgresPartitionMeta::Child { parent, start, end }) => {
                validate_child(schema, table, parent, start, end)?;
            }
            None => {}
        }
    }
    validate_unique_partition_ranges(schema)
}

fn resolve_parent_key(schema: &Schema, reference: &str) -> Result<String, SchemaValidationError> {
    if schema.tables.contains_key(reference) {
        return Ok(reference.to_string());
    }
    let matches = schema
        .tables
        .iter()
        .filter(|(_, table)| table.name == reference)
        .map(|(key, _)| key.clone())
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [key] => Ok(key.clone()),
        [] => Err(invalid(format!(
            "partition parent table '{reference}' not found"
        ))),
        _ => Err(invalid(format!(
            "partition parent table '{reference}' is ambiguous; use its qualified name"
        ))),
    }
}

fn validate_registration(
    schema: &Schema,
    parent_key: &str,
    definition: &PostgresRangePartitioning,
) -> Result<(), SchemaValidationError> {
    let parent = &schema.tables[parent_key];
    if parent.options.postgres_partition.is_some() {
        return Err(invalid(format!(
            "table '{parent_key}' already has PostgreSQL partition metadata"
        )));
    }
    if !parent
        .columns
        .iter()
        .any(|column| column.name == definition.column)
    {
        return Err(invalid(format!(
            "partition key column '{}.{}' not found",
            parent_key, definition.column
        )));
    }
    validate_partition_unique_keys(parent, &definition.column)?;
    validate_partition_definitions(schema, parent, definition)
}

fn validate_partition_definitions(
    schema: &Schema,
    parent: &Table,
    definition: &PostgresRangePartitioning,
) -> Result<(), SchemaValidationError> {
    let mut names = BTreeSet::new();
    let mut bounds = BTreeSet::new();
    for partition in &definition.partitions {
        validate_partition_identity(partition.name(), partition.start(), partition.end())?;
        let key = schema_qualified_key(partition.name(), parent.schema.as_deref());
        if !names.insert(key.clone()) || schema.tables.contains_key(&key) {
            return Err(invalid(format!("partition table '{key}' already exists")));
        }
        if !bounds.insert((partition.start(), partition.end())) {
            return Err(invalid(format!(
                "partition '{}' duplicates an existing range",
                partition.name()
            )));
        }
    }
    Ok(())
}

fn validate_partition_identity(
    name: &str,
    start: &str,
    end: &str,
) -> Result<(), SchemaValidationError> {
    if name.trim().is_empty() || name.contains('.') {
        return Err(invalid(
            "partition names must be non-empty, unqualified table names",
        ));
    }
    if start.trim().is_empty() || end.trim().is_empty() || start == end {
        return Err(invalid(format!(
            "partition '{name}' must have distinct non-empty start and end bounds"
        )));
    }
    Ok(())
}

fn register_partition_tables(
    schema: &mut Schema,
    parent_key: &str,
    definition: PostgresRangePartitioning,
) -> Result<(), SchemaValidationError> {
    let Some(parent) = schema.tables.get_mut(parent_key) else {
        return Err(invalid(format!(
            "partition parent table '{parent_key}' not found"
        )));
    };
    let parent_schema = parent.schema.clone();
    let parent_name = parent.qualified_name();
    parent.options.postgres_partition = Some(PostgresPartitionMeta::Parent {
        column: definition.column,
    });
    for partition in definition.partitions {
        let key = schema_qualified_key(&partition.name, parent_schema.as_deref());
        schema.tables.insert(
            key,
            partition_table(parent_schema.clone(), parent_name.clone(), partition),
        );
    }
    Ok(())
}

fn validate_unique_partition_ranges(schema: &Schema) -> Result<(), SchemaValidationError> {
    let mut ranges = BTreeSet::new();
    for table in schema.tables.values() {
        let Some((parent, start, end)) = table.postgres_range_partition_child() else {
            continue;
        };
        if !ranges.insert((parent, start, end)) {
            return Err(invalid(format!(
                "partition '{}' duplicates range FROM ({start}) TO ({end}) on '{parent}'",
                table.qualified_name()
            )));
        }
    }
    Ok(())
}

fn partition_table(
    schema: Option<String>,
    parent: String,
    partition: PostgresRangePartition,
) -> Table {
    let options = TableOptionsMeta {
        postgres_partition: Some(PostgresPartitionMeta::Child {
            parent,
            start: partition.start,
            end: partition.end,
        }),
        ..Default::default()
    };
    Table {
        name: partition.name,
        schema,
        primary_key: None,
        columns: Vec::new(),
        foreign_keys: Vec::new(),
        indexes: Vec::new(),
        constraints: Vec::new(),
        triggers: Vec::new(),
        options,
    }
}

fn validate_parent(
    schema: &Schema,
    table: &Table,
    column: &str,
) -> Result<(), SchemaValidationError> {
    if !table.columns.iter().any(|item| item.name == column) {
        return Err(invalid(format!(
            "partition key column '{}.{column}' not found",
            table.qualified_name()
        )));
    }
    validate_partition_unique_keys(table, column)?;
    if schema.tables.values().any(|candidate| {
        candidate
            .postgres_range_partition_child()
            .is_some_and(|(parent, _, _)| {
                parent == table.qualified_name() && candidate.schema != table.schema
            })
    }) {
        return Err(invalid(format!(
            "partitions of '{}' must use the parent schema",
            table.qualified_name()
        )));
    }
    Ok(())
}

fn validate_partition_unique_keys(
    table: &Table,
    column: &str,
) -> Result<(), SchemaValidationError> {
    let missing_from_primary = table
        .primary_key
        .as_ref()
        .is_some_and(|key| !key.columns.iter().any(|item| item == column));
    let missing_from_unique = table.constraints.iter().any(|constraint| {
        matches!(constraint, super::Constraint::Unique { columns, .. } if !columns.iter().any(|item| item == column))
    }) || table.indexes.iter().any(|index| {
        index.unique && !index.columns.iter().any(|item| item == column)
    });
    if missing_from_primary || missing_from_unique {
        return Err(invalid(format!(
            "every unique key on partitioned table '{}' must include partition column '{column}'",
            table.qualified_name()
        )));
    }
    Ok(())
}

fn validate_child(
    schema: &Schema,
    table: &Table,
    parent: &str,
    start: &str,
    end: &str,
) -> Result<(), SchemaValidationError> {
    validate_partition_identity(&table.name, start, end)?;
    let Some(parent_table) = schema.tables.get(parent) else {
        return Err(invalid(format!(
            "partition '{}' references missing parent '{parent}'",
            table.qualified_name()
        )));
    };
    if parent_table.postgres_range_partition_column().is_none() {
        return Err(invalid(format!(
            "partition '{}' references non-partitioned table '{parent}'",
            table.qualified_name()
        )));
    }
    if table.schema != parent_table.schema || !partition_is_empty(table) {
        return Err(invalid(format!(
            "partition '{}' must inherit its parent schema and cannot define an independent table body",
            table.qualified_name()
        )));
    }
    Ok(())
}

fn partition_is_empty(table: &Table) -> bool {
    table.columns.is_empty()
        && table.primary_key.is_none()
        && table.foreign_keys.is_empty()
        && table.indexes.is_empty()
        && table.constraints.is_empty()
        && table.triggers.is_empty()
        && table.options.header_raw.is_empty()
        && table.options.tail_raw.is_empty()
}

fn invalid(message: impl Into<String>) -> SchemaValidationError {
    SchemaValidationError::Invalid(message.into())
}
