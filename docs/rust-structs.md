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
- `#[table(schema = "...")]` sets a non-public schema for dialects that support schemas. SQLite rejects schema-qualified objects.
- `#[table(primary_key(columns("a", "b")))]` sets an ordered primary-key column list and derives `{table}_pkey`.
- `#[table(primary_key(name = "...", columns("a", "b")))]` also preserves an explicit primary-key constraint name.

## Column attributes

Column attributes fall into a few groups.

### Naming and inclusion

- `#[column(name = "...")]` overrides the column name.
- `#[column(skip)]` leaves the field out of the schema entirely.

### Type and nullability

- `#[column(type = "...")]` sets the SQL type explicitly. This is the usual escape hatch for third-party types like `uuid` or `timestamptz`.
- `#[column(nullable)]` forces the column to be nullable.
- `#[column(nullable = false)]` forces the column to be non-nullable.

If you do not specify `type = "..."`, Gaman uses the `ColumnType` trait for the Rust field type and the active dialect. `Option<T>` becomes nullable through that path. Built-in mappings use PostgreSQL types by default and SQLite-native types when the `sqlite` feature and `Dialect::Sqlite` are active.

### Constraints and defaults

- `#[column(primary_key)]` marks the column as part of the primary key. Mark multiple fields to create a composite primary key in struct field order.
- `#[column(index)]` adds a single-column index named `{table}_{column}_idx`.
- `#[column(index_name = "idx_name")]` adds a single-column index with an explicit name.
- `#[column(unique)]` adds a single-column unique constraint named `{table}_{column}_key`.
- `#[column(unique_name = "constraint_name")]` adds a single-column unique constraint with an explicit name.
- `#[column(default = "expr")]` sets a SQL default expression.
- `#[column(references = "table.column")]` adds an inline single-column foreign key.
- `#[column(references_name = "fk_name")]` names that foreign key explicitly.
- `#[column(check = "expr")]` adds an inline check constraint.

For composite foreign keys, use table-level metadata so Gaman can preserve
ordered source/target columns. The name is optional and defaults to
`{table}_{source_columns_joined}_fkey`:

```rust
#[derive(gaman::IntoTable)]
#[table(
    name = "orders",
    foreign_key(
        columns("tenant_id", "user_id"),
        references(table = "users", columns("tenant_id", "id"))
    )
)]
struct Order {
    tenant_id: i64,
    user_id: i64,
}
```

## Builder API

Use `TableBuilder` when schema is generated from Rust code instead of derive
attributes. The common helpers cover columns, keys, indexes, and constraints.
For triggers, pass the schema model directly so the public builder surface stays
small:

```rust
use gaman::schema::{
    TableBuilder, TriggerDef, TriggerEvent, TriggerScope, TriggerTiming,
};

let table = TableBuilder::new("orders")
    .column("tenant_id", "bigint", |c| c.not_null())
    .column("id", "bigint", |c| c.not_null())
    .column("email", "text", |c| c.not_null())
    .primary_key_columns(&["tenant_id", "id"])
    .index_columns(&["email"])
    .unique_columns(&["tenant_id", "email"])
    .check_expr("tenant_id > 0")
    .trigger(TriggerDef {
        name: None,
        timing: TriggerTiming::After,
        events: vec![TriggerEvent::Insert],
        scope: TriggerScope::Row,
        function_name: None,
        when: None,
        query: Some("INSERT INTO audit_log(order_id) VALUES (NEW.id);".to_string()),
        language: None,
    })
    .build();
```

PostgreSQL wraps trigger queries in generated trigger functions and supplies the
normal return statement. Set `function_name` instead of `query` when you want to
reference an explicitly modeled trigger function.

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
