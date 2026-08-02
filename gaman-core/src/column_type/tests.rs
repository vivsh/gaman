use super::{ColumnType, Dialect};

/// Checks one inferred type against every supported schema dialect.
#[cfg(any(
    feature = "chrono-types",
    feature = "json-types",
    feature = "time-types",
    feature = "uuid-types"
))]
fn assert_mapping<T: ColumnType>(expected: [&str; 4]) {
    let dialects = [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ];
    for (dialect, expected) in dialects.iter().zip(expected) {
        let desc = T::column_desc(dialect);
        assert_eq!(desc.sql_type, expected);
        assert!(!desc.nullable);
    }
}

/// Checks that a nullable inferred type preserves its SQL mapping.
#[cfg(any(
    feature = "chrono-types",
    feature = "json-types",
    feature = "time-types",
    feature = "uuid-types"
))]
fn assert_nullable_mapping<T: ColumnType>(expected: [&str; 4]) {
    let dialects = [
        Dialect::Postgres,
        Dialect::Sqlite,
        Dialect::Mysql,
        Dialect::Mariadb,
    ];
    for (dialect, expected) in dialects.iter().zip(expected) {
        let desc = <Option<T>>::column_desc(dialect);
        assert_eq!(desc.sql_type, expected);
        assert!(desc.nullable);
    }
}

/// Proves a mapped Rust value is supported by SQLx for one database.
#[cfg(any(
    feature = "chrono-types",
    feature = "json-types",
    feature = "time-types",
    feature = "uuid-types"
))]
fn assert_sqlx_type<T, DB>()
where
    DB: sqlx::Database,
    T: sqlx::Type<DB>,
    for<'q> T: sqlx::Encode<'q, DB>,
    for<'r> T: sqlx::Decode<'r, DB>,
{
}

/// Verifies JSON values use native JSON where available and text on SQLite.
#[cfg(feature = "json-types")]
#[test]
fn json_value_mapping_is_portable() {
    assert_mapping::<serde_json::Value>(["jsonb", "text", "json", "json"]);
    assert_sqlx_type::<serde_json::Value, sqlx::Postgres>();
    assert_sqlx_type::<serde_json::Value, sqlx::Sqlite>();
    assert_sqlx_type::<serde_json::Value, sqlx::MySql>();
}

/// Verifies nullable mappings preserve SQL type while enabling nullability.
#[cfg(feature = "json-types")]
#[test]
fn option_preserves_type_and_sets_nullability() {
    assert_nullable_mapping::<serde_json::Value>(["jsonb", "text", "json", "json"]);
    assert_nullable_mapping::<sqlx::types::Json<serde_json::Value>>([
        "jsonb", "text", "json", "json",
    ]);
}

/// Verifies Chrono aliases resolve through Rust traits instead of token spelling.
#[cfg(feature = "chrono-types")]
#[test]
fn chrono_mappings_are_alias_safe_and_sqlx_compatible() {
    type Timestamp = chrono::DateTime<chrono::Utc>;
    assert_mapping::<Timestamp>(["timestamptz", "text", "timestamp(6)", "timestamp(6)"]);
    assert_mapping::<chrono::NaiveDateTime>(["timestamp", "text", "datetime(6)", "datetime(6)"]);
    assert_mapping::<chrono::NaiveDate>(["date", "text", "date", "date"]);
    assert_nullable_mapping::<Timestamp>(["timestamptz", "text", "timestamp(6)", "timestamp(6)"]);
    assert_sqlx_type::<Timestamp, sqlx::Postgres>();
    assert_sqlx_type::<Timestamp, sqlx::Sqlite>();
    assert_sqlx_type::<Timestamp, sqlx::MySql>();
    assert_sqlx_type::<chrono::NaiveDateTime, sqlx::Postgres>();
    assert_sqlx_type::<chrono::NaiveDateTime, sqlx::Sqlite>();
    assert_sqlx_type::<chrono::NaiveDateTime, sqlx::MySql>();
    assert_sqlx_type::<chrono::NaiveDate, sqlx::Postgres>();
    assert_sqlx_type::<chrono::NaiveDate, sqlx::Sqlite>();
    assert_sqlx_type::<chrono::NaiveDate, sqlx::MySql>();
}

/// Verifies `time` aliases resolve through traits and retain temporal families.
#[cfg(feature = "time-types")]
#[test]
fn time_mappings_are_alias_safe_and_sqlx_compatible() {
    type Timestamp = time::OffsetDateTime;
    assert_mapping::<Timestamp>(["timestamptz", "text", "timestamp(6)", "timestamp(6)"]);
    assert_mapping::<time::PrimitiveDateTime>(["timestamp", "text", "datetime(6)", "datetime(6)"]);
    assert_mapping::<time::Date>(["date", "text", "date", "date"]);
    assert_nullable_mapping::<Timestamp>(["timestamptz", "text", "timestamp(6)", "timestamp(6)"]);
    assert_sqlx_type::<Timestamp, sqlx::Postgres>();
    assert_sqlx_type::<Timestamp, sqlx::Sqlite>();
    assert_sqlx_type::<Timestamp, sqlx::MySql>();
    assert_sqlx_type::<time::PrimitiveDateTime, sqlx::Postgres>();
    assert_sqlx_type::<time::PrimitiveDateTime, sqlx::Sqlite>();
    assert_sqlx_type::<time::PrimitiveDateTime, sqlx::MySql>();
    assert_sqlx_type::<time::Date, sqlx::Postgres>();
    assert_sqlx_type::<time::Date, sqlx::Sqlite>();
    assert_sqlx_type::<time::Date, sqlx::MySql>();
}

/// Verifies UUID storage follows SQLx's binary transport outside PostgreSQL.
#[cfg(feature = "uuid-types")]
#[test]
fn uuid_mapping_matches_sqlx_binary_transport() {
    assert_mapping::<uuid::Uuid>(["uuid", "blob", "binary(16)", "binary(16)"]);
    assert_nullable_mapping::<uuid::Uuid>(["uuid", "blob", "binary(16)", "binary(16)"]);
    assert_sqlx_type::<uuid::Uuid, sqlx::Postgres>();
    assert_sqlx_type::<uuid::Uuid, sqlx::Sqlite>();
    assert_sqlx_type::<uuid::Uuid, sqlx::MySql>();
}

/// Verifies typed SQLx JSON wrappers use the same portable schema policy.
#[cfg(feature = "json-types")]
#[test]
fn typed_json_mapping_is_sqlx_compatible() {
    type Metadata = sqlx::types::Json<serde_json::Value>;
    assert_mapping::<Metadata>(["jsonb", "text", "json", "json"]);
    assert_sqlx_type::<Metadata, sqlx::Postgres>();
    assert_sqlx_type::<Metadata, sqlx::Sqlite>();
    assert_sqlx_type::<Metadata, sqlx::MySql>();
}
