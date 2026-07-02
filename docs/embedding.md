# Embedding Gaman in Rust

Gaman can run entirely inside your binary — no separate CLI process, no external migration runner. The entry point is `MigrationEngine`.

## Setup

```toml
[dependencies]
gaman = "0.3"
```

Embed migration files at compile time with the `embedded_migrations!` macro. It reads every `.yaml` file in the given directory in lexicographic order and produces an `EmbeddedMigrations` value that carries both the compiled-in files and the absolute source directory path.

```rust
use gaman::{EmbeddedMigrations, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");
// macro input is relative to the crate root; the stored dir is absolute
```

## MigrationEngine

`MigrationEngine` is the primary public API for embedding. Construct it with a `Config` and a migration source, call builder methods, then run an action.

```rust
use gaman::{Config, MigrationEngine};

MigrationEngine::new(Config::default(), &MIGRATIONS).migrate()?;
```

`MigrationEngine::new` uses embedded migrations, but the engine itself is
storage-neutral. Use `from_source` or `from_shared_source` when migrations live
somewhere else, such as an in-memory buffer, an application-owned store, or a
future browser-backed string store.

```rust
use gaman::{Config, MigrationEngine};
use gaman::core::{AdapterError, MigrationSource};
use gaman::Migration;

struct MemoryMigrations {
    migrations: Vec<Migration>,
}

impl MigrationSource for MemoryMigrations {
    fn load_all(&self) -> Result<Vec<Migration>, AdapterError> {
        Ok(self.migrations.clone())
    }

    fn save(&self, migration: &Migration) -> Result<(), AdapterError> {
        // Store this wherever your application owns migration state.
        Ok(())
    }
}

let engine = MigrationEngine::from_source(
    Config::default(),
    MemoryMigrations { migrations: vec![] },
);
```

### Config

`Config` controls connection, path, and TLS settings. Set fields directly or rely on environment variables via `Config::default()`.

| Field            | Env var          | Type             | Default         |
| ---------------- | ---------------- | ---------------- | --------------- |
| `database_url`   | `DATABASE_URL`   | `Option<String>` | `None`          |
| `migrations_dir` | `MIGRATIONS_DIR` | `PathBuf`        | `"migrations"`  |
| `schema_file`    | `SCHEMA_FILE`    | `PathBuf`        | `"schema.yaml"` |
| `tls`            | —                | `TlsMode`        | `NoTls`         |

To override specific fields while keeping the rest from env vars:

```rust
let config = Config { database_url: Some(url.to_string()), ..Config::default() };
```

### Builder methods

`with_schema` and `with_dialect` are the builder methods. They take `self` and return the updated engine; `with_schema` returns `Result<Self, EngineError>` so schema load errors surface before any action runs. Calling `with_schema` more than once replaces the previous schema — last call wins.

Use `with_dialect(Dialect::Sqlite)` for offline SQLite planning when there is no `DATABASE_URL` to infer from. PostgreSQL remains the default when neither `DATABASE_URL` nor `with_dialect` selects an engine. SQLite support requires the `sqlite` Cargo feature.

```rust
use gaman::core::Dialect;

let engine = MigrationEngine::new(Config::default(), &MIGRATIONS)
    .with_dialect(Dialect::Postgres);
```

The closure receives a `SchemaBuilder` and can return either a `Schema` (infallible) or a `Result<Schema, SchemaLoadError>` (fallible). Both are accepted via the `IntoSchema` trait.

```rust
// struct-based
engine.with_schema(|s| s.table::<User>().table::<Post>().build())?

// file-based — no unwrap needed
engine.with_schema(|s| s.load_file("schema.yaml"))?

// directory of yaml/sql files
engine.with_schema(|s| s.load_dir("schema/"))?

// combine sources
engine.with_schema(|s| {
    let base = s.load_file("base.yaml")?;
    let extra = Schema::from_file(Path::new("extra.yaml"))?;
    base.merge(extra)
})?
```

`SchemaBuilder` also exposes `.table::<T>()`, `.view()`, `.function()`, `.extension()`, `.enum_type()`, and `.build()` for programmatic construction. See [rust-structs.md](rust-structs.md) for the full `IntoTable` derive reference.

`Schema::merge(other)` merges two schemas into one. Duplicate table names are an error; views, functions, extensions, and enums use last-writer-wins.

### Action methods

Most live actions consume `self`. Offline SQL rendering borrows `&self`, so
library callers can render several SQL views from the same engine value.

```rust
// Apply all pending migrations. Returns how many were applied.
let n: usize = engine.migrate()?;

// Migrate forward or backward to a specific id.
let n: usize = engine.migrate_to("0003_add_posts")?;

// Mark all pending as applied without running SQL.
let n: usize = engine.fake_migrate()?;

// True if unapplied migrations exist.
let pending: bool = engine.check()?;

// Ordered list of pending migration ids.
let ids: Vec<String> = engine.plan()?;

// All migrations with applied status: (id, is_applied).
let list: Vec<(String, bool)> = engine.show_migrations()?;

// Detect drift between replayed state and the live database.
// Empty vec means the DB matches migration history.
// The argument is the PostgreSQL schema name, e.g. "public".
let drift: Vec<Operation> = engine.verify("public")?;

// Introspect the live database and return its schema.
let schema: Schema = engine.inspect_db(&["public"])?;

// Introspect one table from the live database.
let users: Schema = engine.inspect_table(&["public"], "users")?;

// Render operation SQL offline. This does not connect to a database and does
// not include locks, transaction wrappers, or tracking-table writes.
let sql: Vec<String> = engine.sql_migrate()?;
let one: Vec<String> = engine.sql_migrate_id("0002_add_posts")?;
let rollback: Vec<String> = engine.sql_rollback(&["0002_add_posts"])?;

// Diff with_schema() against replayed state, write a migration file if changed.
// Returns Some(migration) or None if already up to date.
// Reads and writes to the embedded source directory on disk — never the embedded slice.
// Re-compile after writing so embedded_migrations! picks up the new file.
// Requires with_schema() — returns Err(EngineError::NoSchema) otherwise.
// Returns Err(EngineError::MigrationsDirMismatch) if config.migrations_dir does not resolve to MIGRATIONS.dir.
// Disambiguations (renames etc.) are resolved via interactive terminal prompts.
let migration: Option<Migration> = engine.make_migration("add_posts")?;

// CI-safe generation paths fail instead of prompting when clarification is needed.
engine.make_migration_check()?;
let migration = engine.make_migration_non_interactive(Some("add_posts"))?;
let preview = engine.make_migration_dry_run_non_interactive(Some("add_posts"))?;

// Write an empty migration shell (no operations) for hand-editing.
let migration: Migration = engine.make_empty_migration("add_posts")?;

// Write a merge migration for multiple graph heads.
let migration: Migration = engine.make_merge_migration("merge_heads")?;

// Parse std::env::args() and dispatch the full CLI surface.
engine.handle_args()?;
```

### EngineError variants

All action methods return `Result<T, EngineError>`.

| Variant                       | When                                                                              |
| ----------------------------- | --------------------------------------------------------------------------------- |
| `Command(CommandError)`       | CLI dispatch error — only from `handle_args`                                      |
| `Migrator(MigratorError)`     | Migration execution error                                                         |
| `Connect(String)`             | Database connection failed                                                        |
| `Adapter(AdapterError)`       | Migration file parse or load error                                                |
| `Config(String)`              | Misconfiguration, e.g. missing `database_url`                                     |
| `NoSchema`                    | `make_migration` called without `with_schema()`                                   |
| `SchemaLoad(SchemaLoadError)` | `with_schema()` closure returned an error loading a schema file                   |
| `NeedsInput(Vec<Clarification>)` | Caller-provided decisions did not answer every required clarification          |
| `MigrationsDirMismatch(…)`    | `config.migrations_dir` does not resolve to the path baked into `embedded_migrations!()` |

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

### Full CLI with struct-based schema

```rust
use gaman::{Config, EmbeddedMigrations, IntoTable, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

#[derive(IntoTable)]
#[table(name = "users")]
struct User { id: i64, email: String }

fn main() {
    MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|s| s.table::<User>().build())
        .and_then(|e| e.handle_args())
        .expect("command failed");
}
```

### Full CLI with a file-based schema

```rust
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), &MIGRATIONS)
        .with_schema(|s| s.load_file("schema.yaml"))
        .and_then(|e| e.handle_args())
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

### Detect drift

```rust
let drift = MigrationEngine::new(config, &MIGRATIONS).verify("public")?;
if !drift.is_empty() {
    eprintln!("database is out of sync: {drift:#?}");
    std::process::exit(1);
}
```

## Multi-crate migration composition

Each library crate can own and embed its own migrations. The application assembles them into a single static tree — no runtime file discovery, no naming coordination between crates.

```rust
// auth crate
pub static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

// app crate
static MIGRATIONS: EmbeddedMigrations = EmbeddedMigrations {
    children: &[("auth", &auth::MIGRATIONS)],
    ..embedded_migrations!("migrations")
};

MigrationEngine::new(Config::default(), &MIGRATIONS).migrate()?;
```

Each child's IDs and dependencies are automatically namespaced (`auth/0001_init`). Children can themselves have children — the tree can be arbitrarily deep. Everything is resolved at compile time.
