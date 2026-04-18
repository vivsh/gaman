# Rust Struct Schema Reference

If your application already describes the data model in Rust, `#[derive(IntoTable)]` lets you keep that as the source of truth for migration generation.

The common case is small:

```rust
use gaman::IntoTable;

#[derive(IntoTable)]
#[table(name = "users")]
struct User {
    id: i64,
    email: String,
    #[column(nullable)]
    bio: Option<String>,
    #[column(default = "now()")]
    created_at: Option<String>,
}
```

## Table attributes

Use table attributes when the default mapping from struct name to table name is not enough.

- `#[table(name = "...")]` overrides the default snake_case table name.
- `#[table(schema = "...")]` sets a non-public PostgreSQL schema.

## Column attributes

Column attributes fall into a few groups.

### Naming and inclusion

- `#[column(name = "...")]` overrides the column name.
- `#[column(skip)]` leaves the field out of the schema entirely.

### Type and nullability

- `#[column(type = "...")]` sets the SQL type explicitly. This is the usual escape hatch for third-party types like `uuid` or `timestamptz`.
- `#[column(nullable)]` forces the column to be nullable.
- `#[column(nullable = false)]` forces the column to be non-nullable.

If you do not specify `type = "..."`, Gaman uses the `ColumnType` trait for the Rust field type. `Option<T>` becomes nullable through that path.

### Constraints and defaults

- `#[column(primary_key)]` marks the column as the primary key.
- `#[column(default = "expr")]` sets a SQL default expression.
- `#[column(references = "table.column")]` adds an inline foreign key.
- `#[column(references_name = "fk_name")]` names that foreign key explicitly.
- `#[column(check = "expr")]` adds an inline check constraint.

## Custom column types

When the field type is yours, implement `gaman::schema::ColumnType` instead of spelling the SQL type on every field.

```rust
use gaman::core::Dialect;
use gaman::schema::{ColumnDesc, ColumnType};

struct MyId(i64);

impl ColumnType for MyId {
    fn column_desc(_dialect: &Dialect) -> ColumnDesc {
        ColumnDesc { sql_type: "bigint", nullable: false }
    }
}
```

Use `#[column(type = "...")]` when you cannot add that trait impl, or when only one field needs a special SQL type.
