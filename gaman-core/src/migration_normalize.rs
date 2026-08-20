use std::collections::HashSet;

use crate::migrations::Migration;
use crate::operations::Operation;
use crate::parsers::parse_opaque_create_portable;
use crate::states::{
    Column, ColumnRef, Constraint, ForeignKey, ReplayError, Schema, Table, TriggerDef,
    canonical_foreign_key_action, names,
};
use crate::states::{EntityKind, schema_qualified_key};

impl Migration {
    /// Returns a structurally canonical migration suitable for replay and rendering.
    ///
    /// Authored-schema column shorthand is expanded into named foreign-key and check
    /// operations. Invalid identity collisions are rejected before lifecycle code can
    /// interpret the same migration in different ways.
    pub(crate) fn canonicalized(&self) -> Result<Self, ReplayError> {
        self.validate_opaque_declarations()?;
        let mut operations = Vec::new();
        for (index, operation) in self.operations.iter().enumerate() {
            let expanded = canonicalize_operation(operation)
                .map_err(|inner| self.operation_error(index, operation, inner))?;
            operations.extend(expanded);
        }
        Ok(Self {
            id: self.id.clone(),
            dependencies: self.dependencies.clone(),
            operations,
            atomic: self.atomic,
        })
    }

    /// Validates stored opaque definitions without changing modeled operation semantics.
    pub(crate) fn validate_opaque_declarations(&self) -> Result<(), ReplayError> {
        for (index, operation) in self.operations.iter().enumerate() {
            validate_opaque_sources(operation)
                .map_err(|inner| self.operation_error(index, operation, inner))?;
        }
        Ok(())
    }

    /// Adds stable migration and operation identity to a normalization failure.
    fn operation_error(
        &self,
        index: usize,
        operation: &Operation,
        inner: ReplayError,
    ) -> ReplayError {
        ReplayError::WithContext {
            migration: self.id.clone(),
            op_num: index + 1,
            operation: format!(
                "{} {}",
                operation.type_name().replace('_', " "),
                operation.entity_name()
            ),
            inner: Box::new(inner),
        }
    }
}

/// Rejects authored shorthand at the dialect operation-rendering boundary.
pub(crate) fn ensure_operation_is_canonical(operation: &Operation) -> Result<(), ReplayError> {
    validate_opaque_sources(operation)?;
    match operation {
        Operation::CreateTable { table } | Operation::DropTable { table } => {
            for column in &table.columns {
                ensure_column_is_canonical(column)?;
            }
        }
        Operation::AddColumn { column, .. } | Operation::DropColumn { column, .. } => {
            ensure_column_is_canonical(column)?;
        }
        Operation::AlterColumn { old, new, .. } => {
            ensure_column_is_canonical(old)?;
            ensure_column_is_canonical(new)?;
        }
        _ => {}
    }
    Ok(())
}

/// Validates every standalone opaque definition carried by one operation.
fn validate_opaque_sources(operation: &Operation) -> Result<(), ReplayError> {
    match operation {
        Operation::CreateTable { table } | Operation::DropTable { table } => {
            validate_table_opaque_sources(table)
        }
        Operation::AddIndex {
            table_name, index, ..
        }
        | Operation::DropIndex {
            table_name, index, ..
        } => validate_opaque_source(
            EntityKind::Index,
            &index.name,
            Some(table_name),
            index.raw_sql(),
        ),
        Operation::CreateFunction { function } | Operation::DropFunction { function } => {
            validate_top_level_source(
                EntityKind::Function,
                &function.name,
                function.schema.as_deref(),
                function.raw_sql(),
            )
        }
        Operation::CreateSequence { sequence } | Operation::DropSequence { sequence } => {
            validate_top_level_source(
                EntityKind::Sequence,
                &sequence.name,
                sequence.schema.as_deref(),
                sequence.raw_sql(),
            )
        }
        Operation::AlterFunction { old, new } => {
            validate_top_level_source(
                EntityKind::Function,
                &old.name,
                old.schema.as_deref(),
                old.raw_sql(),
            )?;
            validate_top_level_source(
                EntityKind::Function,
                &new.name,
                new.schema.as_deref(),
                new.raw_sql(),
            )
        }
        Operation::CreateTrigger {
            table_name,
            trigger,
        }
        | Operation::DropTrigger {
            table_name,
            trigger,
        } => validate_trigger_source(table_name, trigger),
        Operation::AlterTrigger {
            table_name,
            old,
            new,
            ..
        } => {
            validate_trigger_source(table_name, old)?;
            validate_trigger_source(table_name, new)
        }
        Operation::CreateView { view } | Operation::DropView { view } => validate_top_level_source(
            EntityKind::View,
            &view.name,
            view.schema.as_deref(),
            view.raw_sql(),
        ),
        Operation::ReplaceView { old, new } => {
            validate_top_level_source(
                EntityKind::View,
                &old.name,
                old.schema.as_deref(),
                old.raw_sql(),
            )?;
            validate_top_level_source(
                EntityKind::View,
                &new.name,
                new.schema.as_deref(),
                new.raw_sql(),
            )
        }
        Operation::CreateExtension { extension } | Operation::DropExtension { extension } => {
            validate_top_level_source(
                EntityKind::Extension,
                &extension.name,
                extension.schema.as_deref(),
                extension.raw_sql(),
            )
        }
        Operation::CreateEnum { enum_def } | Operation::DropEnum { enum_def } => {
            validate_top_level_source(
                EntityKind::Enum,
                &enum_def.name,
                enum_def.schema.as_deref(),
                enum_def.opaque.raw.as_deref(),
            )
        }
        Operation::AlterEnum { old, new } => {
            validate_top_level_source(
                EntityKind::Enum,
                &old.name,
                old.schema.as_deref(),
                old.opaque.raw.as_deref(),
            )?;
            validate_top_level_source(
                EntityKind::Enum,
                &new.name,
                new.schema.as_deref(),
                new.opaque.raw.as_deref(),
            )
        }
        _ => Ok(()),
    }
}

fn validate_table_opaque_sources(table: &Table) -> Result<(), ReplayError> {
    let owner = table.qualified_name();
    for index in &table.indexes {
        validate_opaque_source(
            EntityKind::Index,
            &index.name,
            Some(&owner),
            index.raw_sql(),
        )?;
    }
    for trigger in &table.triggers {
        validate_trigger_source(&owner, trigger)?;
    }
    Ok(())
}

fn validate_trigger_source(table: &str, trigger: &TriggerDef) -> Result<(), ReplayError> {
    validate_opaque_source(
        EntityKind::Trigger,
        trigger.name.as_deref().unwrap_or_default(),
        Some(table),
        trigger.raw_sql(),
    )
}

fn validate_top_level_source(
    kind: EntityKind,
    name: &str,
    schema: Option<&str>,
    raw: Option<&str>,
) -> Result<(), ReplayError> {
    let identity = if kind == EntityKind::Extension {
        name.to_string()
    } else {
        schema_qualified_key(name, schema)
    };
    validate_opaque_source(kind, &identity, None, raw)
}

fn validate_opaque_source(
    kind: EntityKind,
    identity: &str,
    owner: Option<&str>,
    raw: Option<&str>,
) -> Result<(), ReplayError> {
    let Some(raw) = raw else {
        return Ok(());
    };
    let declaration =
        parse_opaque_create_portable(raw).map_err(|reason| ReplayError::InvalidOpaqueCreate {
            entity: identity.to_string(),
            reason,
        })?;
    if declaration.kind() != kind
        || declaration.identity() != identity
        || declaration.owner() != owner
    {
        return Err(ReplayError::InvalidOpaqueCreate {
            entity: identity.to_string(),
            reason: "stored source kind, identity, or owner does not match the operation"
                .to_string(),
        });
    }
    Ok(())
}

/// Ensures a rendered operation column contains canonical properties only.
fn ensure_column_is_canonical(column: &Column) -> Result<(), ReplayError> {
    if column.check.is_some() || column.references.is_some() {
        return Err(ReplayError::InvalidMigration(format!(
            "column '{}' contains inline check or reference shorthand",
            column.name
        )));
    }
    Ok(())
}

/// Expands one operation while preserving its modeled database effect.
fn canonicalize_operation(operation: &Operation) -> Result<Vec<Operation>, ReplayError> {
    match operation {
        Operation::CreateTable { table } => Ok(vec![Operation::CreateTable {
            table: canonicalize_table(table)?,
        }]),
        Operation::DropTable { table } => Ok(vec![Operation::DropTable {
            table: canonicalize_table(table)?,
        }]),
        Operation::AddColumn { table_name, column } => {
            Ok(canonicalize_add_column(table_name, column))
        }
        Operation::DropColumn {
            table_name,
            column,
            cascade,
        } => Ok(canonicalize_drop_column(table_name, column, *cascade)),
        Operation::AlterColumn {
            table_name,
            old,
            new,
            cast_expr,
        } => Ok(canonicalize_alter_column(
            table_name,
            old,
            new,
            cast_expr.clone(),
        )),
        _ => Ok(vec![operation.clone()]),
    }
}

/// Canonicalizes a table without requiring referenced tables to be present yet.
fn canonicalize_table(table: &Table) -> Result<Table, ReplayError> {
    let key = table.qualified_name();
    let mut schema = Schema::default();
    schema.tables.insert(key.clone(), table.clone());
    schema.normalize();
    let table = schema
        .tables
        .remove(&key)
        .ok_or_else(|| ReplayError::InvalidMigration("normalized table disappeared".to_string()))?;
    validate_table_identities(&table)?;
    Ok(table)
}

/// Rejects shorthand expansion that collides with an explicit table child identity.
fn validate_table_identities(table: &Table) -> Result<(), ReplayError> {
    let mut constraints = HashSet::new();
    for constraint in &table.constraints {
        if !constraints.insert(constraint.name()) {
            return Err(ReplayError::InvalidMigration(format!(
                "table '{}' contains duplicate constraint '{}' after normalization",
                table.name,
                constraint.name()
            )));
        }
    }
    let mut foreign_keys = HashSet::new();
    for foreign_key in &table.foreign_keys {
        if !foreign_keys.insert(foreign_key.name.as_str()) {
            return Err(ReplayError::InvalidMigration(format!(
                "table '{}' contains duplicate foreign key '{}' after normalization",
                table.name, foreign_key.name
            )));
        }
    }
    Ok(())
}

/// Expands an added column into its canonical column and table-child operations.
fn canonicalize_add_column(table_name: &str, column: &Column) -> Vec<Operation> {
    let (column, reference, check) = split_column_shorthand(column);
    let mut operations = vec![Operation::AddColumn {
        table_name: table_name.to_string(),
        column: column.clone(),
    }];
    if let Some(reference) = reference {
        operations.push(Operation::AddForeignKey {
            table_name: table_name.to_string(),
            foreign_key: foreign_key_for(table_name, &column.name, reference),
        });
    }
    if let Some(expression) = check {
        operations.push(Operation::AddConstraint {
            table_name: table_name.to_string(),
            constraint: check_for(table_name, &column.name, expression),
        });
    }
    operations
}

/// Expands a removed column so owned shorthand children are removed first.
fn canonicalize_drop_column(table_name: &str, column: &Column, cascade: bool) -> Vec<Operation> {
    let (column, reference, check) = split_column_shorthand(column);
    let mut operations = Vec::new();
    if let Some(expression) = check {
        operations.push(Operation::DropConstraint {
            table_name: table_name.to_string(),
            constraint: check_for(table_name, &column.name, expression),
        });
    }
    if let Some(reference) = reference {
        operations.push(Operation::DropForeignKey {
            table_name: table_name.to_string(),
            foreign_key: foreign_key_for(table_name, &column.name, reference),
            cascade: false,
        });
    }
    operations.push(Operation::DropColumn {
        table_name: table_name.to_string(),
        column,
        cascade,
    });
    operations
}

/// Separates relation changes from an altered column's ordinary properties.
fn canonicalize_alter_column(
    table_name: &str,
    old: &Column,
    new: &Column,
    cast_expr: Option<String>,
) -> Vec<Operation> {
    let (old, old_reference, old_check) = split_column_shorthand(old);
    let (new, new_reference, new_check) = split_column_shorthand(new);
    let relation_changed = old_reference != new_reference || old_check != new_check;
    let mut operations = Vec::new();
    push_removed_relations(
        &mut operations,
        table_name,
        &old,
        &old_reference,
        &new_reference,
        &old_check,
        &new_check,
    );
    if old != new || cast_expr.is_some() || !relation_changed {
        operations.push(Operation::AlterColumn {
            table_name: table_name.to_string(),
            old: old.clone(),
            new: new.clone(),
            cast_expr,
        });
    }
    push_added_relations(
        &mut operations,
        table_name,
        &new,
        &old_reference,
        &new_reference,
        &old_check,
        &new_check,
    );
    operations
}

/// Adds relation-removal operations required before a column alteration.
fn push_removed_relations(
    operations: &mut Vec<Operation>,
    table_name: &str,
    column: &Column,
    old_reference: &Option<ColumnRef>,
    new_reference: &Option<ColumnRef>,
    old_check: &Option<String>,
    new_check: &Option<String>,
) {
    if old_check != new_check
        && let Some(expression) = old_check.clone()
    {
        operations.push(Operation::DropConstraint {
            table_name: table_name.to_string(),
            constraint: check_for(table_name, &column.name, expression),
        });
    }
    if old_reference != new_reference
        && let Some(reference) = old_reference.clone()
    {
        operations.push(Operation::DropForeignKey {
            table_name: table_name.to_string(),
            foreign_key: foreign_key_for(table_name, &column.name, reference),
            cascade: false,
        });
    }
}

/// Adds relation-creation operations required after a column alteration.
fn push_added_relations(
    operations: &mut Vec<Operation>,
    table_name: &str,
    column: &Column,
    old_reference: &Option<ColumnRef>,
    new_reference: &Option<ColumnRef>,
    old_check: &Option<String>,
    new_check: &Option<String>,
) {
    if old_reference != new_reference
        && let Some(reference) = new_reference.clone()
    {
        operations.push(Operation::AddForeignKey {
            table_name: table_name.to_string(),
            foreign_key: foreign_key_for(table_name, &column.name, reference),
        });
    }
    if old_check != new_check
        && let Some(expression) = new_check.clone()
    {
        operations.push(Operation::AddConstraint {
            table_name: table_name.to_string(),
            constraint: check_for(table_name, &column.name, expression),
        });
    }
}

fn split_column_shorthand(column: &Column) -> (Column, Option<ColumnRef>, Option<String>) {
    let mut column = column.clone();
    let reference = column.references.take();
    let check = column.check.take();
    (column, reference, check)
}

fn check_for(table_name: &str, column_name: &str, expression: String) -> Constraint {
    Constraint::Check {
        name: names::column_check(table_name, column_name),
        expression,
    }
}

fn foreign_key_for(table_name: &str, column_name: &str, reference: ColumnRef) -> ForeignKey {
    let name = reference
        .name
        .unwrap_or_else(|| names::foreign_key(table_name, &[column_name]));
    let mut foreign_key = ForeignKey::single(
        name,
        column_name.to_string(),
        reference.table,
        reference.column,
    );
    foreign_key.on_delete = normalize_action(reference.on_delete);
    foreign_key.on_update = normalize_action(reference.on_update);
    foreign_key
}

fn normalize_action(action: Option<String>) -> Option<String> {
    let action = action?;
    let action = action.trim();
    if action.is_empty() {
        None
    } else {
        Some(
            canonical_foreign_key_action(action)
                .unwrap_or(action)
                .to_string(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dialects::Dialect;
    use crate::states::ViewDef;

    fn column(name: &str) -> Column {
        Column {
            name: name.to_string(),
            col_type: "integer".to_string(),
            ..Column::default()
        }
    }

    fn migration(operation: Operation) -> Migration {
        Migration {
            id: "0001_test".to_string(),
            dependencies: Vec::new(),
            operations: vec![operation],
            atomic: true,
        }
    }

    /// Canonicalization promotes a table's inline check and reference exactly once.
    #[test]
    fn create_table_shorthand_is_canonical_and_idempotent() {
        let mut parent_id = column("parent_id");
        parent_id.check = Some("parent_id > 0".to_string());
        parent_id.references = Some(ColumnRef {
            table: "parents".to_string(),
            column: "id".to_string(),
            name: None,
            on_delete: Some("CASCADE".to_string()),
            on_update: None,
        });
        let table = Table {
            name: "children".to_string(),
            schema: None,
            primary_key: None,
            columns: vec![parent_id],
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            constraints: Vec::new(),
            triggers: Vec::new(),
            options: Default::default(),
        };
        let canonical = migration(Operation::CreateTable { table })
            .canonicalized()
            .expect("legacy shorthand should normalize");
        let twice = canonical
            .canonicalized()
            .expect("canonicalization should be idempotent");
        assert_eq!(canonical.operations, twice.operations);
        let table = match &canonical.operations[0] {
            Operation::CreateTable { table } => table,
            operation => {
                assert_eq!(operation.type_name(), "create_table");
                return;
            }
        };
        assert!(table.columns[0].check.is_none());
        assert!(table.columns[0].references.is_none());
        assert_eq!(table.constraints[0].name(), "children_parent_id_check");
        assert_eq!(table.foreign_keys[0].name, "children_parent_id_fkey");
        assert_eq!(table.foreign_keys[0].on_delete.as_deref(), Some("cascade"));
    }

    /// Added-column shorthand expands into explicit column, FK, and check operations.
    #[test]
    fn add_column_shorthand_expands_to_table_children() {
        let mut child = column("parent_id");
        child.check = Some("parent_id > 0".to_string());
        child.references = Some(ColumnRef {
            table: "parents".to_string(),
            column: "id".to_string(),
            name: None,
            on_delete: None,
            on_update: None,
        });
        let canonical = migration(Operation::AddColumn {
            table_name: "children".to_string(),
            column: child,
        })
        .canonicalized()
        .expect("add column shorthand should normalize");
        assert!(matches!(
            canonical.operations[0],
            Operation::AddColumn { .. }
        ));
        assert!(matches!(
            canonical.operations[1],
            Operation::AddForeignKey { .. }
        ));
        assert!(matches!(
            canonical.operations[2],
            Operation::AddConstraint { .. }
        ));
    }

    /// Dropped-column shorthand removes its table children before the column.
    #[test]
    fn drop_column_shorthand_orders_dependent_removals_first() {
        let mut child = column("parent_id");
        child.check = Some("parent_id > 0".to_string());
        child.references = Some(ColumnRef {
            table: "parents".to_string(),
            column: "id".to_string(),
            name: None,
            on_delete: None,
            on_update: None,
        });
        let canonical = migration(Operation::DropColumn {
            table_name: "children".to_string(),
            column: child,
            cascade: false,
        })
        .canonicalized()
        .expect("drop column shorthand should normalize");
        assert!(matches!(
            canonical.operations[0],
            Operation::DropConstraint { .. }
        ));
        assert!(matches!(
            canonical.operations[1],
            Operation::DropForeignKey { .. }
        ));
        assert!(matches!(
            canonical.operations[2],
            Operation::DropColumn { .. }
        ));
    }

    /// Altered-column check changes become table-constraint operations.
    #[test]
    fn alter_column_shorthand_expands_constraint_change() {
        let mut old = column("score");
        old.check = Some("score >= 0".to_string());
        let mut new = column("score");
        new.check = Some("score > 0".to_string());
        let canonical = migration(Operation::AlterColumn {
            table_name: "results".to_string(),
            old,
            new,
            cast_expr: None,
        })
        .canonicalized()
        .expect("alter column shorthand should normalize");
        assert_eq!(canonical.operations.len(), 2);
        assert!(matches!(
            canonical.operations[0],
            Operation::DropConstraint { .. }
        ));
        assert!(matches!(
            canonical.operations[1],
            Operation::AddConstraint { .. }
        ));
    }

    /// Inline shorthand cannot collide with an explicitly declared generated identity.
    #[test]
    fn create_table_shorthand_rejects_constraint_collision() {
        let mut score = column("score");
        score.check = Some("score >= 0".to_string());
        let table = Table {
            name: "results".to_string(),
            schema: None,
            primary_key: None,
            columns: vec![score],
            foreign_keys: Vec::new(),
            indexes: Vec::new(),
            constraints: vec![Constraint::Check {
                name: "results_score_check".to_string(),
                expression: "score < 100".to_string(),
            }],
            triggers: Vec::new(),
            options: Default::default(),
        };
        let error = migration(Operation::CreateTable { table })
            .canonicalized()
            .expect_err("colliding constraints should fail");
        assert!(matches!(
            error,
            ReplayError::WithContext { inner, .. }
                if matches!(*inner, ReplayError::InvalidMigration(ref message) if message.contains("duplicate constraint"))
        ));
    }

    /// Committed opaque migration source cannot retain caller-owned lifecycle modifiers.
    #[test]
    fn migration_rejects_modified_opaque_create_source() {
        for source in [
            "CREATE OR REPLACE VIEW active_users AS SELECT 1",
            "CREATE VIEW IF NOT EXISTS active_users AS SELECT 1",
        ] {
            let error = migration(Operation::CreateView {
                view: ViewDef::from_raw("active_users", source),
            })
            .canonicalized()
            .expect_err("legacy modified source must fail");
            assert!(matches!(
                error,
                ReplayError::WithContext {
                    migration,
                    op_num: 1,
                    inner,
                    ..
                } if migration == "0001_test"
                    && matches!(*inner, ReplayError::InvalidOpaqueCreate { .. })
            ));
        }
    }

    /// Every dialect renders opaque replacement as DROP followed by stored plain CREATE.
    #[test]
    fn opaque_view_replacement_never_rewrites_create_source() {
        let create = "CREATE VIEW active_users AS SELECT 2";
        let old = ViewDef::from_raw("active_users", "CREATE VIEW active_users AS SELECT 1");
        let mut start = Schema::default();
        start.views.insert("active_users".to_string(), old.clone());
        let mut migration = migration(Operation::ReplaceView {
            old,
            new: ViewDef::from_raw("active_users", create),
        });
        migration.atomic = false;
        for dialect in [
            Dialect::Postgres,
            Dialect::Sqlite,
            Dialect::Mysql,
            Dialect::Mariadb,
        ] {
            let sql = dialect
                .migration_to_sql(&migration, &start)
                .expect("opaque replacement must render");
            assert_eq!(sql.len(), 2);
            assert!(sql[0].starts_with("DROP VIEW"));
            assert_eq!(sql[1], create);
        }
    }

    /// Direct rendering cannot bypass strict validation of opaque migration metadata.
    #[test]
    fn every_dialect_rejects_modified_opaque_render_source() {
        let mut migration = migration(Operation::CreateView {
            view: ViewDef::from_raw(
                "active_users",
                "CREATE OR REPLACE VIEW active_users AS SELECT 1",
            ),
        });
        migration.atomic = false;
        for dialect in [
            Dialect::Postgres,
            Dialect::Sqlite,
            Dialect::Mysql,
            Dialect::Mariadb,
        ] {
            let error = dialect
                .migration_to_sql(&migration, &Schema::default())
                .expect_err("modified opaque source must not render");
            assert!(
                matches!(
                    error,
                    crate::dialects::DialectError::Migration(ReplayError::WithContext {
                        ref inner,
                        ..
                    }) if matches!(**inner, ReplayError::InvalidOpaqueCreate { ref reason, .. }
                        if reason.contains("Gaman owns replacement"))
                ),
                "{dialect:?}: {error:?}"
            );
        }
    }
}
