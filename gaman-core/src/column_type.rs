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
        ColumnDesc {
            nullable: true,
            ..T::column_desc(dialect)
        }
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

impl_column_type!(i16,     Dialect::Postgres => "smallint", Dialect::Sqlite => "integer", Dialect::Mysql => "smallint");
impl_column_type!(i32,     Dialect::Postgres => "integer", Dialect::Sqlite => "integer", Dialect::Mysql => "int");
impl_column_type!(i64,     Dialect::Postgres => "bigint", Dialect::Sqlite => "integer", Dialect::Mysql => "bigint");
impl_column_type!(f32,     Dialect::Postgres => "real", Dialect::Sqlite => "real", Dialect::Mysql => "float");
impl_column_type!(f64,     Dialect::Postgres => "double precision", Dialect::Sqlite => "real", Dialect::Mysql => "double");
impl_column_type!(bool,    Dialect::Postgres => "boolean", Dialect::Sqlite => "integer", Dialect::Mysql => "boolean");
impl_column_type!(String,  Dialect::Postgres => "text", Dialect::Sqlite => "text", Dialect::Mysql => "text");
impl_column_type!(Vec<u8>, Dialect::Postgres => "bytea", Dialect::Sqlite => "blob", Dialect::Mysql => "blob");
