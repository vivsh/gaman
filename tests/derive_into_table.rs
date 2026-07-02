use gaman::IntoTable;
use gaman::core::Dialect;
use gaman::schema::{Constraint, Table, TableBuilder};

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(name = "users")]
struct User {
    #[column(primary_key)]
    id: i64,
    #[column(unique)]
    email: String,
    #[column(index)]
    username: String,
    #[column(index_name = "users_handle_lookup_idx")]
    handle: String,
    #[column(unique_name = "users_external_id_key")]
    external_id: String,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(name = "order_lines")]
struct OrderLine {
    #[column(primary_key)]
    order_id: i64,
    #[column(primary_key)]
    tenant_id: i64,
    note: String,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(
    name = "explicit_order_lines",
    primary_key(name = "order_lines_identity", columns("tenant_id", "order_id"))
)]
struct ExplicitOrderLine {
    #[column(primary_key)]
    order_id: i64,
    #[column(primary_key)]
    tenant_id: i64,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(
    name = "unnamed_order_lines",
    primary_key(columns("tenant_id", "order_id"))
)]
struct UnnamedPkOrderLine {
    tenant_id: i64,
    order_id: i64,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(
    name = "orders",
    foreign_key(
        name = "orders_user_fkey",
        columns("tenant_id", "user_id"),
        references(table = "users", columns("tenant_id", "id"))
    )
)]
struct Order {
    tenant_id: i64,
    user_id: i64,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(
    name = "unnamed_orders",
    foreign_key(
        columns("tenant_id", "user_id"),
        references(table = "users", columns("tenant_id", "id"))
    )
)]
struct UnnamedFkOrder {
    tenant_id: i64,
    user_id: i64,
}

#[allow(dead_code)]
#[derive(IntoTable)]
#[table(
    name = "builder_users",
    schema = "app",
    primary_key(columns("tenant_id", "id")),
    foreign_key(
        columns("tenant_id", "org_id"),
        references(table = "orgs", columns("tenant_id", "id"))
    )
)]
struct BuilderParityUser {
    tenant_id: i64,
    id: i64,
    #[column(name = "email_address", index, unique)]
    email: String,
    #[column(r#type = "citext", nullable = true, default = "'anon'")]
    handle: String,
    #[column(nullable = false)]
    bio: Option<String>,
    org_id: i64,
    #[column(references = "roles.id", references_name = "builder_users_role_fkey")]
    role_id: i64,
    #[column(check = "age >= 0")]
    age: Option<i32>,
}

fn table<T: gaman::schema::IntoTable>() -> Table {
    T::into_table(&Dialect::Postgres)
}

#[test]
fn derive_into_table_emits_column_indexes_and_unique_constraints() {
    let users = table::<User>();

    assert_eq!(users.indexes.len(), 2);
    assert!(
        users
            .indexes
            .iter()
            .any(|i| { i.name == "users_username_idx" && i.columns == ["username"] && !i.unique })
    );
    assert!(
        users.indexes.iter().any(|i| {
            i.name == "users_handle_lookup_idx" && i.columns == ["handle"] && !i.unique
        })
    );

    assert_eq!(users.constraints.len(), 2);
    assert!(
        users
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::Unique { name, columns } if name == "users_email_key" && columns == &["email"]))
    );
    assert!(
        users
            .constraints
            .iter()
            .any(|c| matches!(c, Constraint::Unique { name, columns } if name == "users_external_id_key" && columns == &["external_id"]))
    );
}

#[test]
fn derive_into_table_allows_composite_primary_key_fields() {
    let table = table::<OrderLine>();
    let pk = table.primary_key.expect("primary key");

    assert_eq!(pk.name, "order_lines_pkey");
    assert_eq!(pk.columns, ["order_id", "tenant_id"]);
    assert!(
        table
            .columns
            .iter()
            .find(|c| c.name == "order_id")
            .unwrap()
            .primary_key
    );
    assert!(
        table
            .columns
            .iter()
            .find(|c| c.name == "tenant_id")
            .unwrap()
            .primary_key
    );
}

#[test]
fn derive_into_table_uses_explicit_primary_key_order_and_name() {
    let table = table::<ExplicitOrderLine>();
    let pk = table.primary_key.expect("primary key");

    assert_eq!(pk.name, "order_lines_identity");
    assert_eq!(pk.columns, ["tenant_id", "order_id"]);
}

#[test]
fn derive_into_table_generates_name_for_table_primary_key() {
    let table = table::<UnnamedPkOrderLine>();
    let pk = table.primary_key.expect("primary key");

    assert_eq!(pk.name, "unnamed_order_lines_pkey");
    assert_eq!(pk.columns, ["tenant_id", "order_id"]);
}

#[test]
fn derive_into_table_emits_composite_foreign_key() {
    let table = table::<Order>();
    let fk = &table.foreign_keys[0];

    assert_eq!(fk.name, "orders_user_fkey");
    assert_eq!(fk.columns, ["tenant_id", "user_id"]);
    assert_eq!(fk.to_table, "users");
    assert_eq!(fk.to_columns, ["tenant_id", "id"]);
}

#[test]
fn derive_into_table_generates_name_for_composite_foreign_key() {
    let table = table::<UnnamedFkOrder>();
    let fk = &table.foreign_keys[0];

    assert_eq!(fk.name, "unnamed_orders_tenant_id_user_id_fkey");
    assert_eq!(fk.columns, ["tenant_id", "user_id"]);
    assert_eq!(fk.to_table, "users");
    assert_eq!(fk.to_columns, ["tenant_id", "id"]);
}

/// Verifies IntoTable derive emits the same table model as the public TableBuilder API.
#[test]
fn derive_into_table_matches_equivalent_table_builder() {
    let dialect = Dialect::Postgres;
    let derived = table::<BuilderParityUser>();
    let built = TableBuilder::new("builder_users")
        .schema("app")
        .column_from_type::<i64>(&dialect, "tenant_id", |c| c)
        .column_from_type::<i64>(&dialect, "id", |c| c)
        .column_from_type::<String>(&dialect, "email_address", |c| c)
        .column("handle", "citext", |c| c.nullable().default("'anon'"))
        .column_from_type::<Option<String>>(&dialect, "bio", |c| c.not_null())
        .column_from_type::<i64>(&dialect, "org_id", |c| c)
        .column_from_type::<i64>(&dialect, "role_id", |c| {
            c.references_named("builder_users_role_fkey", "roles", "id")
        })
        .column_from_type::<Option<i32>>(&dialect, "age", |c| c.check("age >= 0"))
        .primary_key_columns(&["tenant_id", "id"])
        .foreign_key_columns(&["tenant_id", "org_id"], "orgs", &["tenant_id", "id"])
        .index_columns(&["email_address"])
        .unique_columns(&["email_address"])
        .build();

    assert_eq!(derived, built);
}
