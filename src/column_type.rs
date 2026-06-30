use crate::dialects::Dialect;

/// Resolved SQL type and nullability for one column.
#[derive(Clone, Copy)]
pub struct ColumnDesc {
    pub sql_type: &'static str,
    pub nullable: bool,
}

/// Map a Rust type to a SQL column description.
/// Implement this for your own types, or use `#[column(type = "...")]` per field.
pub trait ColumnType {
    fn column_desc(dialect: &Dialect) -> ColumnDesc;
}

impl<T: ColumnType> ColumnType for Option<T> {
    fn column_desc(dialect: &Dialect) -> ColumnDesc {
        ColumnDesc { nullable: true, ..T::column_desc(dialect) }
    }
}

macro_rules! impl_column_type {
    ($ty:ty, $( $(#[$meta:meta])* $variant:path => $sql:literal ),+ $(,)?) => {
        impl ColumnType for $ty {
            fn column_desc(dialect: &Dialect) -> ColumnDesc {
                let sql_type = match dialect { $( $(#[$meta])* $variant => $sql, )+ };
                ColumnDesc { sql_type, nullable: false }
            }
        }
    };
}

impl_column_type!(i16,     Dialect::Postgres => "smallint", #[cfg(feature = "sqlite")] Dialect::Sqlite => "integer");
impl_column_type!(i32,     Dialect::Postgres => "integer", #[cfg(feature = "sqlite")] Dialect::Sqlite => "integer");
impl_column_type!(i64,     Dialect::Postgres => "bigint", #[cfg(feature = "sqlite")] Dialect::Sqlite => "integer");
impl_column_type!(f32,     Dialect::Postgres => "real", #[cfg(feature = "sqlite")] Dialect::Sqlite => "real");
impl_column_type!(f64,     Dialect::Postgres => "double precision", #[cfg(feature = "sqlite")] Dialect::Sqlite => "real");
impl_column_type!(bool,    Dialect::Postgres => "boolean", #[cfg(feature = "sqlite")] Dialect::Sqlite => "integer");
impl_column_type!(String,  Dialect::Postgres => "text", #[cfg(feature = "sqlite")] Dialect::Sqlite => "text");
impl_column_type!(Vec<u8>, Dialect::Postgres => "bytea", #[cfg(feature = "sqlite")] Dialect::Sqlite => "blob");
