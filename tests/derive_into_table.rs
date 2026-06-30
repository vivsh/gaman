use gaman::core::Dialect;
use gaman::schema::{Constraint, Table};
use gaman::IntoTable;

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

fn table<T: gaman::schema::IntoTable>() -> Table {
    T::into_table(&Dialect::Postgres)
}

#[test]
fn derive_into_table_emits_column_indexes_and_unique_constraints() {
    let users = table::<User>();

    assert_eq!(users.indexes.len(), 2);
    assert!(users
        .indexes
        .iter()
        .any(|i| { i.name == "users_username_idx" && i.columns == ["username"] && !i.unique }));
    assert!(users
        .indexes
        .iter()
        .any(|i| { i.name == "users_handle_lookup_idx" && i.columns == ["handle"] && !i.unique }));

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
