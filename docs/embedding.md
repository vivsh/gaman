# Embedding Gaman in Rust

Gaman can run entirely inside your binary — no separate CLI process, no external migration runner. The entry point is `MigrationEngine`.

## Setup

```toml
[dependencies]
gaman = "0.3"
```

Embed migration files at compile time with the `embedded_migrations!` macro. It reads every `.yaml` file in the given directory in lexicographic order and produces an `EmbeddedMigrations` value that carries both the compiled-in files and the source directory path.

```rust
use gaman::{EmbeddedMigrations, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");
// The path is relative to the crate root (same as include_str!).
// MIGRATIONS.files — the embedded (id, yaml) pairs
// MIGRATIONS.dir  — the source directory string, used to validate config.migrations_dir
```

## MigrationEngine

`MigrationEngine` is the sole public API for embedding. It is constructed with a builder, then consumed by one action call.

```rust
use gaman::{Config, MigrationEngine};

let engine = MigrationEngine::new(Config::default(), &MIGRATIONS);
```

### Config

`Config` controls connection and path settings. All fields are optional — unset fields fall back to environment variables.

| Field            | Env var          | Type             |
| ---------------- | ---------------- | ---------------- |
| `database_url`   | `DATABASE_URL`   | `Option<String>` |
| `migrations_dir` | `MIGRATIONS_DIR` | `Option<String>` |
| `schema_file`    | `SCHEMA_FILE`    | `Option<String>` |

`Config::default()` leaves all fields as `None` and relies on environment variables.

### Builder methods

All builder methods take `self` and return `Self`. Call them in any order before the action.

```rust
MigrationEngine::new(config, &MIGRATIONS)
    .with_database_url("postgres://localhost/myapp")  // override DATABASE_URL
    .with_schema(|s| s.table::<User>().build())       // provide schema for make_migration
    .with_tls(TlsMode::NoTls)                         // TLS mode (only NoTls exists today)
```

`with_schema` receives a `SchemaBuilder` and must return a `Schema`. Use `SchemaBuilder::table::<T>()` for struct-based schemas, or `Schema::load(path)` for file-based ones (see below).

### Action methods

Each action consumes `self`. Construct a new `MigrationEngine` for each call.

```rust
// Apply all pending migrations. Returns how many were applied.
let n: usize = engine.migrate()?;

// Migrate to a specific id (forward or backward).
let n: usize = engine.migrate_to("0003_add_posts")?;

// Mark all pending as applied without running SQL.
// Useful when the database was set up outside gaman.
let n: usize = engine.fake_migrate()?;

// True if unapplied migrations exist.
let pending: bool = engine.check()?;

// Ordered list of pending migration ids.
let ids: Vec<String> = engine.plan()?;

// All migrations with applied status: (id, is_applied).
let list: Vec<(String, bool)> = engine.show_migrations()?;

// Detect drift between replayed state and live database.
// Empty vec means the DB matches the migration history.
// `schema` is the PostgreSQL schema name, e.g. "public".
let drift: Vec<Operation> = engine.verify("public")?;

// Introspect the live database and return a Schema.
// Pass the PostgreSQL schema names to scan, e.g. &["public"].
let schema: Schema = engine.inspect_db(&["public"])?;

// Diff with_schema() against replayed state, write a migration file if changed.
// Returns Some(migration) or None if already up to date.
// Reads and writes to config.migrations_dir on disk (always disk, never the embedded slice).
// After writing, re-compile so embedded_migrations! picks up the new file.
// Requires with_schema() — returns Err(EngineError::NoSchema) otherwise.
// Returns Err(EngineError::MigrationsDirMismatch) if config.migrations_dir != MIGRATIONS.dir.
// Disambiguations (renames, etc.) are resolved via interactive terminal prompts.
let migration: Option<Migration> = engine.make_migration("add_posts")?;;

// Parse std::env::args() and dispatch the full CLI.
// Supports make_migration, migrate, verify_db, show_migrations, inspect_db, sql_migrate.
engine.handle_args()?;
```

### EngineError

All action methods return `Result<T, EngineError>`. The variants:

- `Command(CommandError)` — CLI dispatch error (only from `handle_args`)
- `Migrator(MigratorError)` — migration execution error
- `Connect(String)` — database connection failed
- `Adapter(AdapterError)` — migration file parse/load error
- `Config(String)` — misconfiguration (e.g. missing `database_url`)
- `NoSchema` — `make_migration` called without `with_schema()`
- `MigrationsDirMismatch(String, &'static str)` — `config.migrations_dir` differs from the path baked into `EmbeddedMigrations`; fix by making sure the argument to `embedded_migrations!` matches `config.migrations_dir`

## Common patterns

### Auto-migrate on startup

```rust
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn main() {
    let n = MigrationEngine::new(Config::default(), &MIGRATIONS)
        .migrate()
        .expect("migrations failed");
    if n > 0 {
        eprintln!("{n} migration(s) applied");
    }
}
```

### Expose the full CLI with a struct-based schema

```rust
use gaman::{Config, EmbeddedMigrations, IntoTable, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

#[derive(IntoTable)]
#[table(name = "users")]
struct User {
    id: i64,
    email: String,
}

fn main() {
    MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|s| s.table::<User>().build())
        .handle_args()
        .expect("command failed");
}
```

### Expose the full CLI with a file-based schema

```rust
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};
use gaman::states::Schema;

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|_| Schema::load(std::path::Path::new("schema.sql")).unwrap())
        .handle_args()
        .expect("command failed");
}
```

### Programmatic control

```rust
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn run_migrations(db_url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config = Config { database_url: Some(db_url.to_string()), ..Config::default() };

    if MigrationEngine::new(config.clone(), &MIGRATIONS).check()? {
        let plan = MigrationEngine::new(config.clone(), &MIGRATIONS).plan()?;
        eprintln!("applying: {plan:?}");
        let n = MigrationEngine::new(config, &MIGRATIONS).migrate()?;
        eprintln!("{n} applied");
    }

    Ok(())
}
```

### Detect drift at runtime

```rust
let drift = MigrationEngine::new(config, &MIGRATIONS)
    .verify("public")?;

if !drift.is_empty() {
    eprintln!("database is out of sync: {drift:#?}");
    std::process::exit(1);
}
```

## SchemaBuilder

`with_schema` hands you a `SchemaBuilder`. The only method you need for struct-derived schemas is `table::<T>()`:

```rust
.with_schema(|s| {
    s.table::<User>()
     .table::<Post>()
     .build()
})
```

Each `table::<T>()` call requires `T: IntoTable`. For the full `#[derive(IntoTable)]` attribute reference, see [rust-structs.md](rust-structs.md).
