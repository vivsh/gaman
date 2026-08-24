use std::collections::BTreeMap;

use gaman_core::clarifier::{Answer, ClarificationKind, Decision};
use gaman_core::managed_rows::{ManagedRow, ManagedRows, ManagedValue};
use gaman_core::operations::Operation;
use gaman_core::runner::EntityFilter;
use gaman_core::states::{EntityKind, Schema};
use gaman_core::{Dialect, OfflineError, OfflinePlanner};

fn postgres_schema(sql: &str) -> Schema {
    Schema::from_sql_str(sql, Dialect::Postgres).expect("valid PostgreSQL schema")
}

fn filter(value: &str) -> EntityFilter {
    EntityFilter::parse(value).expect("valid entity filter")
}

/// Verifies one table filter excludes an independent changed table.
#[test]
fn selects_only_matching_root() {
    let desired = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE TABLE projects (id bigint PRIMARY KEY);",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("users")])
        .expect("filtered migration")
        .expect("migration");

    assert!(
        migration
            .get_entities()
            .contains(&(EntityKind::Table, "users".to_string()))
    );
    assert!(!migration.operations.iter().any(|operation| {
        operation
            .table_name()
            .is_some_and(|table| table == "projects")
    }));
}

/// Verifies automatic names are stable across filter order and duplicates.
#[test]
fn automatic_name_uses_filtered_operations_deterministically() {
    let desired = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE TABLE projects (id bigint PRIMARY KEY);",
    );
    let planner = OfflinePlanner::new(Dialect::Postgres);
    let first = planner
        .make_migration_filtered(desired.clone(), &[], &[filter("users"), filter("projects")])
        .expect("first migration")
        .expect("first migration");
    let second = planner
        .make_migration_filtered(
            desired,
            &[],
            &[filter("projects"), filter("users"), filter("users")],
        )
        .expect("second migration")
        .expect("second migration");

    assert_eq!(first.id, second.id);
    assert_eq!(first.operations, second.operations);
}

/// Verifies a later unfiltered migration contains changes excluded previously.
#[test]
fn remaining_changes_are_generated_later() {
    let desired = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE TABLE projects (id bigint PRIMARY KEY);",
    );
    let first = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired.clone(), &[], &[filter("users")])
        .expect("filtered migration")
        .expect("filtered migration");
    let second = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![first])
        .make_migration(desired, &[])
        .expect("remaining migration")
        .expect("remaining migration");

    assert!(second.operations.iter().any(|operation| {
        operation
            .table_name()
            .is_some_and(|table| table == "projects")
    }));
    assert!(
        !second
            .operations
            .iter()
            .any(|operation| { operation.table_name().is_some_and(|table| table == "users") })
    );
}

/// Verifies required changed enum dependencies are included automatically.
#[test]
fn includes_required_changed_dependencies() {
    let desired = postgres_schema(
        "CREATE TYPE task_status AS ENUM ('open', 'closed');\n\
         CREATE TABLE tasks (id bigint PRIMARY KEY, status task_status NOT NULL);",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("tasks")])
        .expect("filtered migration")
        .expect("migration");

    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateEnum { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateTable { .. }))
    );
}

/// Verifies filtering a table retains its managed-row operations.
#[test]
fn table_filter_includes_managed_rows() {
    let mut desired = postgres_schema(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL);",
    );
    desired.managed_rows.insert(
        "task_lanes".to_string(),
        ManagedRows {
            rows: vec![ManagedRow {
                values: BTreeMap::from([
                    ("id".to_string(), ManagedValue("approval".into())),
                    ("name".to_string(), ManagedValue("review".into())),
                ]),
            }],
        },
    );
    let desired = desired.prepare(Dialect::Postgres).expect("managed schema");
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("task_lanes")])
        .expect("filtered migration")
        .expect("migration");

    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::InsertRow { .. }))
    );
}

/// Verifies unchanged known roots are a no-op while unknown filters fail.
#[test]
fn distinguishes_unchanged_and_unknown_filters() {
    let desired = postgres_schema("CREATE TABLE users (id bigint PRIMARY KEY);");
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(desired.clone(), &[])
        .expect("initial migration")
        .expect("initial migration");
    let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![initial]);

    assert!(
        planner
            .make_migration_filtered(desired.clone(), &[], &[filter("users")])
            .expect("unchanged filter")
            .is_none()
    );
    let error = planner
        .make_migration_filtered(desired, &[], &[filter("missing")])
        .expect_err("unknown filter must fail");
    assert!(error.to_string().contains("matched no known root entity"));
}

/// Verifies selecting the new side of a rename includes and resolves the old side.
#[test]
fn selected_rename_pair_remains_atomic() {
    let previous = postgres_schema("CREATE TABLE users (id bigint PRIMARY KEY);");
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = postgres_schema("CREATE TABLE accounts (id bigint PRIMARY KEY);");
    let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![initial]);
    let error = planner
        .make_migration_filtered(desired.clone(), &[], &[filter("accounts")])
        .expect_err("rename clarification");
    let OfflineError::NeedsInput(clarifications) = error else {
        panic!("expected rename clarification");
    };
    let clarification = clarifications
        .iter()
        .find(|clarification| matches!(clarification.kind, ClarificationKind::RenameTable { .. }))
        .expect("rename clarification");
    let migration = planner
        .make_migration_filtered(
            desired,
            &[Decision {
                clarification_id: clarification.id.clone(),
                answer: Answer::RenameTo("accounts".to_string()),
            }],
            &[filter("accounts")],
        )
        .expect("resolved migration")
        .expect("migration");

    assert_eq!(migration.operations.len(), 1);
    assert!(matches!(
        &migration.operations[0],
        Operation::RenameTable { old_name, new_name }
            if old_name == "users" && new_name == "accounts"
    ));
}

/// Verifies selecting the old side of a rename retains the same atomic pair.
#[test]
fn selects_rename_by_old_identity() {
    let previous = postgres_schema("CREATE TABLE users (id bigint PRIMARY KEY);");
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = postgres_schema("CREATE TABLE accounts (id bigint PRIMARY KEY);");
    let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![initial]);
    let error = planner
        .make_migration_filtered(desired, &[], &[filter("users")])
        .expect_err("rename clarification");
    let OfflineError::NeedsInput(clarifications) = error else {
        panic!("expected rename clarification");
    };

    assert!(clarifications.iter().any(|clarification| matches!(
        &clarification.kind,
        ClarificationKind::RenameTable { old, candidates }
            if old == "users" && candidates.iter().any(|candidate| candidate == "accounts")
    )));
}

/// Verifies filtering one rename candidate does not surface clarification for
/// another independently changed table.
#[test]
fn excludes_unrelated_rename_clarifications() {
    let previous = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE TABLE audit_logs (id bigint PRIMARY KEY, message text NOT NULL);",
    );
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = postgres_schema(
        "CREATE TABLE accounts (id bigint PRIMARY KEY);\n\
         CREATE TABLE audit_events (id bigint PRIMARY KEY, message text NOT NULL, created_at bigint);",
    );
    let error = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(desired, &[], &[filter("accounts")])
        .expect_err("selected rename clarification");
    let OfflineError::NeedsInput(clarifications) = error else {
        panic!("expected rename clarification");
    };

    assert!(clarifications.iter().any(|clarification| matches!(
        &clarification.kind,
        ClarificationKind::RenameTable { old, .. } if old == "users"
    )));
    assert!(!clarifications.iter().any(|clarification| matches!(
        &clarification.kind,
        ClarificationKind::RenameTable { old, .. } if old == "audit_logs"
    )));
}

/// Proves the additive filtered API is byte-for-byte equivalent when no
/// filters are supplied.
#[test]
fn empty_filters_preserve_unfiltered_generation() {
    let desired = postgres_schema(
        "CREATE TYPE task_status AS ENUM ('open', 'closed');\n\
         CREATE TABLE tasks (id bigint PRIMARY KEY, status task_status NOT NULL);",
    );
    let planner = OfflinePlanner::new(Dialect::Postgres);
    let unfiltered = planner
        .make_migration(desired.clone(), &[])
        .expect("unfiltered migration");
    let filtered = planner
        .make_migration_filtered(desired, &[], &[])
        .expect("empty-filter migration");

    assert_eq!(
        serde_yaml::to_string(&unfiltered).expect("serialize unfiltered migration"),
        serde_yaml::to_string(&filtered).expect("serialize empty-filter migration")
    );
}

/// Verifies glob matching honors qualified root identities.
#[test]
fn supports_globs_and_qualified_identities() {
    let desired = postgres_schema(
        "CREATE TABLE audit.users (id bigint PRIMARY KEY);\n\
         CREATE TABLE public.projects (id bigint PRIMARY KEY);",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("table:audit.*")])
        .expect("qualified filtered migration")
        .expect("migration");

    let entities = migration.get_entities();
    assert!(entities.contains(&(EntityKind::Table, "audit.users".to_string())));
    assert!(!entities.contains(&(EntityKind::Table, "projects".to_string())));
}

/// Verifies removals can be selected because filters resolve against replayed
/// as well as desired roots.
#[test]
fn selects_removed_roots() {
    let previous = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE TABLE projects (id bigint PRIMARY KEY);",
    );
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = postgres_schema("CREATE TABLE users (id bigint PRIMARY KEY);");
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(desired, &[], &[filter("projects")])
        .expect("filtered drop migration")
        .expect("migration");

    assert!(matches!(
        migration.operations.as_slice(),
        [Operation::DropTable { table }] if table.name == "projects"
    ));
}

/// Verifies selecting a child table includes a changed parent required by its
/// modeled foreign key.
#[test]
fn includes_foreign_key_dependencies() {
    let desired = postgres_schema(
        "CREATE TABLE parents (id bigint PRIMARY KEY);\n\
         CREATE TABLE children (id bigint PRIMARY KEY, parent_id bigint NOT NULL,\n\
           CONSTRAINT children_parent_fk FOREIGN KEY (parent_id) REFERENCES parents (id));",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("children")])
        .expect("filtered migration")
        .expect("migration");
    let tables: Vec<&str> = migration
        .operations
        .iter()
        .filter_map(Operation::table_name)
        .collect();

    assert!(tables.contains(&"parents"));
    assert!(tables.contains(&"children"));
}

/// Verifies unknown-type clarification is scoped away from unrelated roots.
#[test]
fn ignores_unrelated_unknown_type_clarifications() {
    let desired = postgres_schema(
        "CREATE TABLE selected (id bigint PRIMARY KEY);\n\
         CREATE TABLE ignored (id bigint PRIMARY KEY, payload application_payload);",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("selected")])
        .expect("unrelated unknown type should not block")
        .expect("migration");

    assert!(migration.operations.iter().all(|operation| {
        operation
            .table_name()
            .is_none_or(|table| table == "selected")
    }));
}

/// Verifies selecting a changed view includes its changed table dependency.
#[test]
fn includes_view_dependencies() {
    let desired = postgres_schema(
        "CREATE TABLE users (id bigint PRIMARY KEY);\n\
         CREATE VIEW active_users AS SELECT id FROM users;",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired, &[], &[filter("view:active_users")])
        .expect("filtered view migration")
        .expect("migration");

    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateView { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateTable { .. }))
    );
}

/// Verifies filtered migrations remain renderable offline for every modeled
/// SQL dialect.
#[test]
fn renders_filtered_migrations_for_all_dialects() {
    for dialect in [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ] {
        let desired = Schema::from_sql_str("CREATE TABLE users (id integer PRIMARY KEY);", dialect)
            .expect("valid schema");
        let planner = OfflinePlanner::new(dialect);
        let migration = planner
            .make_migration_filtered(desired, &[], &[filter("users")])
            .expect("filtered migration")
            .expect("migration");
        let statements = planner
            .sql_migrate(&[migration])
            .expect("render filtered migration");

        assert!(!statements.is_empty(), "{dialect:?} rendered no SQL");
    }
}

/// Verifies SQLite retains a selected table's complete rebuild while excluding
/// an unrelated changed table from the candidate.
#[test]
fn preserves_sqlite_rebuild_groups() {
    let previous = Schema::from_sql_str(
        "CREATE TABLE users (id integer PRIMARY KEY, name text, obsolete text);\n\
         CREATE TABLE projects (id integer PRIMARY KEY, legacy text);",
        Dialect::Sqlite,
    )
    .expect("valid previous SQLite schema");
    let initial = OfflinePlanner::new(Dialect::Sqlite)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = Schema::from_sql_str(
        "CREATE TABLE users (id integer PRIMARY KEY, name text);\n\
         CREATE TABLE projects (id integer PRIMARY KEY);",
        Dialect::Sqlite,
    )
    .expect("valid desired SQLite schema");
    let planner = OfflinePlanner::new(Dialect::Sqlite).from_migrations(vec![initial]);
    let migration = planner
        .make_migration_filtered(desired, &[], &[filter("users")])
        .expect("filtered rebuild migration")
        .expect("migration");

    assert!(
        migration
            .operations
            .iter()
            .all(|operation| { operation.table_name().is_none_or(|table| table == "users") })
    );
    let statements = planner
        .sql_migrate(&[migration])
        .expect("render SQLite rebuild");
    assert!(
        statements
            .iter()
            .any(|statement| statement.contains("__gaman_rebuild_users"))
    );
    assert!(
        statements
            .iter()
            .all(|statement| !statement.contains("__gaman_rebuild_projects"))
    );
}

/// Verifies enums, extensions, and functions can each be selected directly as
/// root entities without pulling independent roots into the migration.
#[test]
fn selects_every_non_table_root_kind() {
    let desired = postgres_schema(
        "CREATE TYPE task_status AS ENUM ('open', 'closed');\n\
         CREATE EXTENSION pgcrypto;\n\
         CREATE FUNCTION task_count() RETURNS bigint LANGUAGE SQL AS $$ SELECT 1 $$;",
    );
    for (selector, expected) in [
        ("enum:task_status", EntityKind::Enum),
        ("extension:pgcrypto", EntityKind::Extension),
        ("function:task_count", EntityKind::Function),
    ] {
        let migration = OfflinePlanner::new(Dialect::Postgres)
            .make_migration_filtered(desired.clone(), &[], &[filter(selector)])
            .expect("filtered root migration")
            .expect("migration");
        let entities = migration.get_entities();
        assert!(entities.iter().any(|(kind, _)| *kind == expected));
        assert!(!migration.operations.iter().any(|operation| {
            matches!(
                operation,
                Operation::CreateTable { .. } | Operation::CreateView { .. }
            )
        }));
    }
}

/// Verifies one table filter owns its columns, indexes, constraints, foreign
/// keys, and trigger while including the trigger function dependency.
#[test]
fn table_filter_includes_structural_children() {
    let previous = postgres_schema(
        "CREATE TABLE parents (id bigint PRIMARY KEY);\n\
         CREATE TABLE items (id bigint PRIMARY KEY);",
    );
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let desired = postgres_schema(
        "CREATE TABLE parents (id bigint PRIMARY KEY);\n\
         CREATE TABLE items (id bigint PRIMARY KEY, parent_id bigint, note text,\n\
           CONSTRAINT items_parent_fk FOREIGN KEY (parent_id) REFERENCES parents(id),\n\
           CONSTRAINT items_id_check CHECK (id > 0));\n\
         CREATE INDEX items_note_idx ON items (note);\n\
         CREATE FUNCTION audit_item() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RETURN NEW; END $$;\n\
         CREATE TRIGGER items_audit BEFORE INSERT ON items FOR EACH ROW EXECUTE FUNCTION audit_item();",
    );
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(desired, &[], &[filter("items")])
        .expect("filtered table migration")
        .expect("migration");

    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AddColumn { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AddIndex { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AddConstraint { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::AddForeignKey { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateTrigger { .. }))
    );
    assert!(
        migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateFunction { .. }))
    );
}

fn managed_schema(name: &str) -> Schema {
    let mut schema = postgres_schema(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL);",
    );
    schema.managed_rows.insert(
        "task_lanes".to_string(),
        ManagedRows {
            rows: vec![ManagedRow {
                values: BTreeMap::from([
                    ("id".to_string(), ManagedValue("approval".into())),
                    ("name".to_string(), ManagedValue(name.into())),
                ]),
            }],
        },
    );
    schema.prepare(Dialect::Postgres).expect("managed schema")
}

/// Verifies table selection includes managed-row updates and confirmed deletes,
/// while a table drop absorbs its rows without redundant row operations.
#[test]
fn table_filter_preserves_managed_row_lifecycle_groups() {
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(managed_schema("review"), &[])
        .expect("initial migration")
        .expect("initial migration");
    let planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![initial.clone()]);
    let update = planner
        .make_migration_filtered(
            managed_schema("senior_review"),
            &[],
            &[filter("task_lanes")],
        )
        .expect("managed update")
        .expect("migration");
    assert!(matches!(
        update.operations.as_slice(),
        [Operation::UpdateRow { .. }]
    ));

    let mut without_rows = postgres_schema(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL);",
    );
    without_rows = without_rows
        .prepare(Dialect::Postgres)
        .expect("table schema");
    let error = planner
        .make_migration_filtered(without_rows, &[], &[filter("task_lanes")])
        .expect_err("managed delete clarification");
    let OfflineError::NeedsInput(clarifications) = error else {
        panic!("expected managed delete clarification");
    };
    let delete = clarifications
        .iter()
        .find(|clarification| {
            matches!(
                clarification.kind,
                ClarificationKind::DeleteManagedRow { .. }
            )
        })
        .expect("managed delete clarification");
    let table_only = postgres_schema(
        "CREATE TABLE task_lanes (id text PRIMARY KEY NOT NULL, name text NOT NULL);",
    );
    let deletion = planner
        .make_migration_filtered(
            table_only,
            &[Decision {
                clarification_id: delete.id.clone(),
                answer: Answer::AcceptRisk,
            }],
            &[filter("task_lanes")],
        )
        .expect("confirmed managed delete")
        .expect("migration");
    assert!(matches!(
        deletion.operations.as_slice(),
        [Operation::DeleteRow { .. }]
    ));

    let drop = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(Schema::default(), &[], &[filter("task_lanes")])
        .expect("filtered table drop")
        .expect("migration");
    assert!(
        drop.operations
            .iter()
            .any(|operation| matches!(operation, Operation::DropTable { .. }))
    );
    assert!(
        !drop
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::DeleteRow { .. }))
    );
}

/// Verifies changed dependencies are minimal and already-committed dependencies
/// are not emitted again.
#[test]
fn dependency_closure_is_minimal_and_changed_only() {
    let desired = postgres_schema(
        "CREATE TYPE selected_status AS ENUM ('open');\n\
         CREATE TYPE unrelated_status AS ENUM ('ready');\n\
         CREATE TABLE selected_tasks (id bigint PRIMARY KEY, status selected_status);\n\
         CREATE TABLE unrelated_tasks (id bigint PRIMARY KEY, status unrelated_status);",
    );
    let first = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(desired.clone(), &[], &[filter("selected_tasks")])
        .expect("selected branch")
        .expect("migration");
    assert!(
        first
            .get_entities()
            .contains(&(EntityKind::Enum, "selected_status".to_string()))
    );
    assert!(
        !first
            .get_entities()
            .contains(&(EntityKind::Enum, "unrelated_status".to_string()))
    );

    let committed_enum = postgres_schema("CREATE TYPE selected_status AS ENUM ('open');");
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(committed_enum, &[])
        .expect("enum migration")
        .expect("enum migration");
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(
            postgres_schema(
                "CREATE TYPE selected_status AS ENUM ('open');\n\
                 CREATE TABLE selected_tasks (id bigint PRIMARY KEY, status selected_status);",
            ),
            &[],
            &[filter("selected_tasks")],
        )
        .expect("table migration")
        .expect("migration");
    assert!(
        !migration
            .operations
            .iter()
            .any(|operation| matches!(operation, Operation::CreateEnum { .. }))
    );
}

/// Verifies selecting a parent drop includes the changed child dependency and
/// orders the child removal first.
#[test]
fn parent_drop_includes_child_dependency() {
    let previous = postgres_schema(
        "CREATE TABLE parents (id bigint PRIMARY KEY);\n\
         CREATE TABLE children (id bigint PRIMARY KEY, parent_id bigint,\n\
           CONSTRAINT children_parent_fk FOREIGN KEY (parent_id) REFERENCES parents(id));",
    );
    let initial = OfflinePlanner::new(Dialect::Postgres)
        .make_migration(previous, &[])
        .expect("initial migration")
        .expect("initial migration");
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .from_migrations(vec![initial])
        .make_migration_filtered(Schema::default(), &[], &[filter("parents")])
        .expect("dependent drops")
        .expect("migration");
    let dropped = migration
        .operations
        .iter()
        .filter_map(|operation| match operation {
            Operation::DropTable { table } => Some(table.name.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();

    assert_eq!(dropped, vec!["children", "parents"]);
}

/// Verifies public-schema aliases and overlapping filters select one canonical
/// root without duplicating its operations.
#[test]
fn public_alias_and_overlapping_filters_are_canonical() {
    let desired = postgres_schema("CREATE TABLE public.users (id bigint PRIMARY KEY);");
    let migration = OfflinePlanner::new(Dialect::Postgres)
        .make_migration_filtered(
            desired,
            &[],
            &[
                filter("users"),
                filter("table:public.*"),
                filter("table:users"),
            ],
        )
        .expect("overlapping filters")
        .expect("migration");

    assert_eq!(migration.operations.len(), 1);
    assert!(matches!(
        migration.operations[0],
        Operation::CreateTable { .. }
    ));
}

/// Verifies sequence filters select qualified opaque roots and leave independent roots pending.
#[test]
fn sequence_filter_selects_only_the_requested_root() {
    let desired =
        postgres_schema("CREATE SEQUENCE audit.event_ids;\nCREATE SEQUENCE billing.invoice_ids;");
    let planner = OfflinePlanner::new(Dialect::Postgres);
    let pending = planner
        .make_migration_filtered(desired.clone(), &[], &[filter("sequence::audit.*")])
        .expect_err("opaque sequence should require explicit acceptance");
    let clarification_id = match pending {
        OfflineError::NeedsInput(clarifications) => {
            clarifications
                .into_iter()
                .find(|clarification| {
                    matches!(
                        clarification.kind,
                        ClarificationKind::OpaqueEntity {
                            kind: EntityKind::Sequence,
                            ..
                        }
                    )
                })
                .expect("sequence clarification should be present")
                .id
        }
        other => panic!("expected sequence clarification, got {other:?}"),
    };
    let first = planner
        .make_migration_filtered(
            desired.clone(),
            &[Decision {
                clarification_id,
                answer: Answer::AcceptRisk,
            }],
            &[filter("sequence::audit.*")],
        )
        .expect("accepted sequence filter should plan")
        .expect("selected sequence should produce a migration");
    assert!(matches!(
        first.operations.as_slice(),
        [Operation::CreateSequence { sequence }] if sequence.qualified_name() == "audit.event_ids"
    ));

    let remaining_planner = OfflinePlanner::new(Dialect::Postgres).from_migrations(vec![first]);
    let pending = remaining_planner
        .make_migration(desired.clone(), &[])
        .expect_err("remaining opaque sequence should require explicit acceptance");
    let clarification_id = match pending {
        OfflineError::NeedsInput(clarifications) => {
            clarifications
                .into_iter()
                .find(|clarification| {
                    matches!(
                        clarification.kind,
                        ClarificationKind::OpaqueEntity {
                            kind: EntityKind::Sequence,
                            ..
                        }
                    )
                })
                .expect("remaining sequence clarification should be present")
                .id
        }
        other => panic!("expected sequence clarification, got {other:?}"),
    };
    let remaining = remaining_planner
        .make_migration(
            desired,
            &[Decision {
                clarification_id,
                answer: Answer::AcceptRisk,
            }],
        )
        .expect("accepted remaining sequence should plan")
        .expect("remaining sequence should produce a migration");
    assert!(matches!(
        remaining.operations.as_slice(),
        [Operation::CreateSequence { sequence }] if sequence.qualified_name() == "billing.invoice_ids"
    ));
}
