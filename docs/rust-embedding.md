# Embedding Gaman In Rust

Gaman is primarily exposed as a CLI, but Rust applications can use the same
planning and execution engine directly. This is useful when migrations need to
ship inside an application binary, compose across crates, or run through custom
storage.

The embedding API is intentionally small:

- `MigrationEngine` orchestrates planning, SQL rendering, apply, inspect, and verify.
- `EmbeddedMigrations` stores migration trees as plain Rust data.
- `MigrationSource` supports custom migration storage.
- `SchemaBuilder`, `TableBuilder`, and `IntoTable` provide structured Rust schema input.

## Feature Flags

The default crate build includes the CLI and the currently supported live
dialects. Applications that want smaller builds can select features explicitly:

```toml
[dependencies]
gaman = { version = "0.3", default-features = false, features = ["postgres"] }
```

Use `sqlite` for SQLite support. Offline-only integrations can use the core
types without enabling a live database executor.

## Structured Schema Builders

Rust schema input uses builders and traits, not a Gaman-owned model derive:

```rust
use gaman::core::Dialect;
use gaman::schema::TableBuilder;

let dialect = Dialect::Postgres;
let users = TableBuilder::new("users")
    .column_from_type::<i64>(&dialect, "id", |c| c.primary_key())
    .column_from_type::<String>(&dialect, "email", |c| c.not_null())
    .column("created_at", "timestamptz", |c| c.default("now()"))
    .unique_columns(&["email"])
    .build();
```

`IntoTable` remains a plain trait. Model/query crates can derive or implement it
without Gaman owning Rust model macros.

```rust
use gaman::core::Dialect;
use gaman::schema::{IntoTable, Table, TableBuilder};

struct User;

impl IntoTable for User {
    fn into_table(dialect: &Dialect) -> Table {
        TableBuilder::new("users")
            .column_from_type::<i64>(dialect, "id", |c| c.primary_key())
            .column_from_type::<String>(dialect, "email", |c| c.not_null())
            .build()
    }
}
```

## Embedded Migration Sources

Gaman keeps `EmbeddedMigrations` as a plain data structure and migration source
adapter. It does not provide an embedding macro; external framework/model crates
can construct or return this Gaman-compatible shape.

Multiple crates can compose migration trees:

```rust
use gaman::EmbeddedMigrations;

static MIGRATIONS: EmbeddedMigrations = EmbeddedMigrations {
    files: &[],
    dir: "migrations",
    children: &[("auth", &auth::MIGRATIONS)],
};
```

Child IDs are namespaced, for example `auth/0001_init`.

## MigrationEngine

`MigrationEngine` is the public orchestration API. The CLI delegates to it.
Use `new` when you already have an `EmbeddedMigrations` value from another crate
or a manual static definition, `from_directory` for a filesystem migration
directory, and `from_source` for custom storage.

```rust
use gaman::{Config, MigrationEngine};
use gaman::core::Dialect;

let config = Config::new(
    "postgres://localhost/app".to_string(),
    "migrations".into(),
    "schema.yaml".into(),
    Dialect::Postgres,
);
let engine = MigrationEngine::new(config, &MIGRATIONS);
```

Common methods:

```rust
engine.sql()?;                              // offline operation SQL
engine.sql_id("0002_add_posts")?;           // one migration
engine.sql_rollback(&["0002_add_posts"])?;  // offline rollback SQL
engine.make_non_interactive(None)?;         // CI-safe generation
engine.make_check()?;                       // fail if schema changed
engine.show()?;                              // canonical migration artifacts, offline
engine.inspect_table(&["public"], "users").await?;
engine.verify("public").await?;
engine.verify_report_schemas(&["public", "billing"]).await?;
```

Live actions require a database connection. Offline SQL planning does not.
`apply`, `apply_to`, and rollback methods return `MigrationMovement` with
separate `applied` and `reverted` counts.

## Custom Storage

Custom storage implements `MigrationSource`. It can be file-backed, embedded,
in-memory, or application-owned.

Use this when migrations live outside the normal `migrations/` directory or when
an application wants to present migrations from a virtual filesystem, compiled
assets, or framework-owned storage.
