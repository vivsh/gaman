//! Logic for name derivation throughout the crate.

pub fn primary_key(table: &str) -> String {
    format!("{table}_pkey")
}

pub fn foreign_key(table: &str, columns: &[impl AsRef<str>]) -> String {
    format!("{}_{}_fkey", table, join_columns(columns))
}

pub fn index(table: &str, columns: &[impl AsRef<str>]) -> String {
    format!("{}_{}_idx", table, join_columns(columns))
}

pub fn unique(table: &str, columns: &[impl AsRef<str>]) -> String {
    format!("{}_{}_key", table, join_columns(columns))
}

pub fn column_check(table: &str, column: &str) -> String {
    format!("{table}_{column}_check")
}

pub fn table_check(table: &str) -> String {
    format!("{table}_check")
}

fn join_columns(columns: &[impl AsRef<str>]) -> String {
    columns
        .iter()
        .map(AsRef::as_ref)
        .collect::<Vec<_>>()
        .join("_")
}
