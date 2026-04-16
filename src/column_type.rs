use crate::dialects::Dialect;

/// The SQL type and nullability for a column, resolved per-dialect.
#[derive(Clone, Copy)]
pub struct ColumnDesc {
    pub sql_type: &'static str,
    pub nullable: bool,
}

/// Maps a Rust type to its SQL column description for a given dialect.
///
/// Implement this on your own types to support custom column types in
/// `#[derive(IntoTable)]`. Use `#[column(type = "...")]` as a per-field
/// escape hatch for third-party types where you cannot provide an impl.
pub trait ColumnType {
    fn column_desc(dialect: &Dialect) -> ColumnDesc;
}

impl<T: ColumnType> ColumnType for Option<T> {
    fn column_desc(dialect: &Dialect) -> ColumnDesc {
        ColumnDesc { nullable: true, ..T::column_desc(dialect) }
    }
}

macro_rules! impl_column_type {
    ($ty:ty, $( $variant:path => $sql:literal ),+ $(,)?) => {
        impl ColumnType for $ty {
            fn column_desc(dialect: &Dialect) -> ColumnDesc {
                let sql_type = match dialect { $( $variant => $sql, )+ };
                ColumnDesc { sql_type, nullable: false }
            }
        }
    };
}

impl_column_type!(i16,     Dialect::Postgres => "smallint");
impl_column_type!(i32,     Dialect::Postgres => "integer");
impl_column_type!(i64,     Dialect::Postgres => "bigint");
impl_column_type!(f32,     Dialect::Postgres => "real");
impl_column_type!(f64,     Dialect::Postgres => "double precision");
impl_column_type!(bool,    Dialect::Postgres => "boolean");
impl_column_type!(String,  Dialect::Postgres => "text");
impl_column_type!(Vec<u8>, Dialect::Postgres => "bytea");
