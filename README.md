# Gaman

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Gaman is an offline-first schema migration engine that generates deterministic migrations without connecting to a live database.

Given the schema you want and the migrations you already committed, Gaman
deterministically replays history, diffs the result, and plans the next
migration.

```text
schema.yaml
    ↓
gaman make_migration
    ↓
migration.yaml
    ↓
gaman sql_migrate
    ↓
SQL
    ↓
gaman migrate
```

> **Project status:** Early-stage. Core engine is stable and tested in production use. Public API and file format may change before 1.0.

PostgreSQL is the default and broadest supported engine. SQLite support is
available behind the `sqlite` Cargo feature and is intentionally engine-specific:
SQLite migrations are not expected to look like PostgreSQL migrations, and
unsupported operations fail clearly instead of becoming no-op SQL.

## Why Developers Like Gaman

- Migration generation is deterministic and offline.
- Replay, diffing, and SQL planning use the same schema model.
- YAML, JSON, SQL DDL, and Rust structs can all feed the same engine.
- Ambiguous or risky changes are surfaced before migration files are written.
- `sql_migrate` previews the operation SQL without opening a database connection.
- Embedded migrations can ship inside Rust binaries and compose across crates.

## Quick Start

Install the CLI:

```bash
cargo install gaman
```

Point it at a schema file and a migrations directory:

```bash
export DATABASE_URL=postgres://localhost/myapp
export MIGRATIONS_DIR=migrations
export SCHEMA_FILE=schema.yaml
```

Then use the normal loop:

```bash
gaman make_migration initial
gaman sql_migrate
gaman migrate
gaman verify_db
```

For offline commands without a `DATABASE_URL`, pass the dialect explicitly:

```bash
gaman make_migration add_posts --dialect postgres
gaman sql_migrate --dialect sqlite
```

`make_migration` can run interactively, or in non-interactive mode when CI should
fail instead of prompting for risky or ambiguous decisions.

## What Is Supported Today

Engine-specific migration files are expected. Gaman does not try to make one
migration portable across PostgreSQL, SQLite, and future engines.

Unless a row explicitly says live introspection or `verify_db`, support means the
feature is modeled for deterministic offline diff/replay and SQL rendering.

Legend: ✅ implemented, 🚧 planned but not implemented, ❌ unsupported by design
or by the database engine.

| Feature | PostgreSQL | SQLite | MySQL / MariaDB |
| --- | --- | --- | --- |
| Offline replay, diff, and migration generation | ✅ | ✅ | 🚧 |
| Offline SQL rendering through `sql_migrate` | ✅ | ✅ | 🚧 |
| Live migration application | ✅ | ✅ | 🚧 |
| Live database introspection for supported metadata | ✅ | ✅ | 🚧 |
| Live `verify_db` for supported metadata | ✅ | ✅ | 🚧 |
| Migration tracking table | ✅ | ✅ | 🚧 |
| Dedicated migration lock | ✅ | ❌ | 🚧 |
| Tables: create, drop, rename | ✅ | ✅ | 🚧 |
| Columns: add, drop, rename | ✅ | ✅ | 🚧 |
| Columns: type, nullability, default changes | ✅ | ✅ | 🚧 |
| Generated columns | ✅ | ✅ | 🚧 |
| Single-column primary keys | ✅ | ✅ | 🚧 |
| Multi-column / composite primary keys | ✅ | ✅ | 🚧 |
| Automatic primary-key mutation generation | ❌ | ❌ | ❌ |
| Single-column foreign keys | ✅ | ✅ | 🚧 |
| Multi-column / composite foreign keys | ✅ | ✅ | 🚧 |
| Unique constraints | ✅ | ✅ | 🚧 |
| Check constraints | ✅ | ✅ | 🚧 |
| Indexes | ✅ | ✅ | 🚧 |
| Partial indexes | ✅ | ✅ | 🚧 |
| Concurrent indexes | ✅ | ❌ | 🚧 |
| Schemas / namespaces | ✅ | ❌ | ❌ |
| Extensions as opaque schema objects | ✅ | ❌ | ❌ |
| Enums | ✅ | ❌ | 🚧 |
| Functions as opaque schema objects | ✅ | ❌ | 🚧 |
| Trigger query schema objects | ✅ | ✅ | 🚧 |
| Views as opaque schema objects | ✅ | ✅ | 🚧 |
| Raw SQL statements | ✅ | ✅ | 🚧 |
| SQLite table-rebuild planner for ALTER TABLE | ❌ | ✅ | ❌ |
| Opaque source formatting fallback in offline diff | ✅ | ✅ | 🚧 |

## Author Schema Where It Belongs

Gaman has one schema model and multiple authoring formats:

- YAML when schema metadata should be explicit and reviewable.
- SQL DDL when schema is already maintained as SQL.
- Rust structs when application types are the natural schema source.

All of them normalize into the same deterministic schema representation before
diffing or SQL generation.

### YAML

YAML is the most explicit format and is a good fit when schema metadata is the
source of truth.

```yaml
tables:
  order_lines:
    primary_key:
      columns: [tenant_id, order_id]
    columns:
      - { name: tenant_id, type: bigint }
      - { name: order_id, type: bigint }
      - { name: product_id, type: bigint, nullable: false }
    foreign_keys:
      - columns: [tenant_id, product_id]
        to_table: products
        to_columns: [tenant_id, id]
    indexes:
      - columns: [product_id]
```

Names for primary keys, foreign keys, indexes, unique constraints, and check
constraints may be omitted when Gaman can derive deterministic names. Add
explicit names only when you need to preserve database-compatible object names.

The same schema model can also be loaded from JSON. For the exact schema model
and normalization rules, see [ARCHITECTURE.md](ARCHITECTURE.md).

### SQL DDL

SQL DDL is useful when you already maintain schema in SQL:

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);
```

SQL parsing is intentionally focused on the supported schema model. Raw
statements are available in migration files for dialect-specific work outside
that model.

### Rust Structs

Rust structs work well when schema lives beside application types:

```rust
use gaman::IntoTable;

#[derive(IntoTable)]
#[table(name = "users", schema = "app")]
struct User {
    #[column(primary_key)]
    id: i64,
    email: String,
    bio: Option<String>,
    #[column(type = "timestamptz")]
    created_at: chrono::DateTime<chrono::Utc>,
    #[column(references = "orgs.id")]
    org_id: i64,
}
```

Field-level attributes are ergonomic shorthands. Multi-column primary keys and
foreign keys are represented as table-level metadata so column order and
constraint names are preserved.

See [docs/rust-structs.md](docs/rust-structs.md) for the full derive reference.

## How Gaman Works

Gaman treats migration planning as a pure, deterministic replay problem.

```text
YAML / JSON / SQL / Rust structs
        |
        v
Schema IR
        |
        v
normalize -> dialect canonicalize -> validate
        |
        v
diff against replayed migrations
        |
        v
disambiguate -> migration YAML -> deterministic SQL plan
```

Migration replay is deterministic, offline, and side-effect free. It produces an
in-memory `Schema`, not database changes.

`sql_migrate` uses the same dialect renderer that live migration execution uses,
but it emits only migration operation SQL. It does not include tracking table
setup, locks, transactions, or record/unrecord statements.

Live database access is used deliberately:

- `migrate` applies already planned migrations.
- `inspect_db` bootstraps schema from an existing database.
- `verify_db` compares live metadata against replayed migration state.

## Deterministic Migration Planning

Live-database-first tools are useful, but they make the current database an
implicit participant in generation. Gaman keeps generation offline: the database
is where migrations are applied, not where migrations are discovered.

That matters when schema ownership is spread across generated Rust structs,
handwritten SQL, YAML files, embedded crates, local development databases, and CI
environments. Gaman reduces each source to the same schema model before replay,
diffing, disambiguation, and deterministic SQL generation.

## Disambiguation, Not Guesswork

Some schema changes cannot be inferred safely from a diff. A rename can look like
a drop plus an add. A type change may need an explicit cast. A new `NOT NULL`
column may need a backfill.

Gaman asks before generating those migrations. In non-interactive mode, it fails
instead of guessing.

Unknown data types use trust on first use (TOFU). Type catalogs are intentionally
incomplete: Gaman knows common built-in aliases and popular extension types, but
it does not pretend to know every domain, composite, extension, or user-defined
type your database may support. If a new unknown type appears in desired schema,
Gaman asks whether to map it to a known type or keep it exactly. Once committed
in a migration, that type becomes trusted project history.

## Why Not...

Gaman is not trying to replace every migration workflow. It makes a specific
architectural tradeoff: deterministic offline planning first, live database
execution second.

- Flyway-style tools are excellent at applying ordered SQL files. Gaman adds an
  offline schema model, diffing, disambiguation, and SQL planning before those
  files exist.
- Atlas-style tools model desired state and database inspection deeply. Gaman's
  core planning path is designed to work without connecting to a database.
- Diesel migrations keep schema evolution close to Rust applications. Gaman also
  supports Rust-facing schema input, but it treats Rust structs as one frontend
  into a database-agnostic schema model.
- Handwritten SQL remains valuable. Gaman keeps `Statement` as the escape hatch
  instead of pretending every backend-specific operation belongs in the model.

## Ship Migrations Inside Your Binary

Gaman can run as a CLI, but it is also designed to ship inside Rust binaries.

```toml
[dependencies]
gaman = "0.3"
```

```rust
use gaman::{Config, EmbeddedMigrations, MigrationEngine, embedded_migrations};

static MIGRATIONS: EmbeddedMigrations = embedded_migrations!("migrations");

fn main() {
    let applied = MigrationEngine::new(Config::default(), &MIGRATIONS)
        .migrate()
        .expect("migrations failed");

    if applied > 0 {
        eprintln!("{applied} migration(s) applied");
    }
}
```

Embedded migrations are resolved at compile time. Applications split across
multiple crates can compose each crate's migration tree into one ordered graph;
Gaman namespaces child migrations so IDs do not collide.

See [docs/embedding.md](docs/embedding.md) for the full embedding API.

## Design Boundaries

These are intentional modeling choices or dialect-specific behaviors:

- Column-level `references` is single-column shorthand. Use table-level
  `foreign_keys` for multi-column references.
- Multi-column primary keys and foreign keys are canonical table-level metadata.
  Column-level flags and references are input shorthands only.
- Primary-key mutation generation is intentionally unsupported for now. Use raw
  `Statement` operations for backend-specific PK surgery.
- Opaque source text is preserved exactly. Lexical canonicalization is used only
  as a fallback to suppress formatting-only diff churn.
- `verify_db` compares deterministic opaque metadata where available, but it does
  not prove function, trigger, or view body equivalence from live catalog text.
- PostgreSQL trigger `query` source is wrapped in generated trigger functions
  with default return behavior. Use an explicit modeled function and
  `function_name` for custom returns or `TG_OP` branching.
- SQLite renders query triggers directly, but function-backed triggers,
  statement-level triggers, `TRUNCATE` triggers, and trigger `language` are
  unsupported.
- SQLite table rebuilds require `atomic: true`; primary-key changes, tables with
  modeled triggers, and dependent views are rejected until Gaman can preserve
  them safely.
- SQLite nullable-to-not-null rebuilds require a default or explicit cast
  expression so existing rows can be copied deterministically.
- SQLite live introspection currently covers tables, columns, primary keys,
  foreign keys, and user-created indexes. Check constraints, views,
  generated-column expressions, and trigger bodies should be treated as authored
  schema/migration metadata rather than guaranteed live-drift detection.

## Docs

- [ARCHITECTURE.md](ARCHITECTURE.md): project goals, lifecycle, schema model,
  offline/WASM boundary, and planned work.
- [docs/cli.md](docs/cli.md): commands, flags, environment variables, and
  dialect selection.
- [docs/rust-structs.md](docs/rust-structs.md): derive macro and Rust schema
  metadata.
- [docs/embedding.md](docs/embedding.md): embedding migrations in Rust binaries.
- [TESTING.md](TESTING.md): fixture layout and test commands.

## Development

```bash
cargo test
cargo test --test offline
cargo test --features sqlite
```

PostgreSQL integration tests require a running database:

```bash
export TEST_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test postgres -- --include-ignored
```

Fixture cases live under `tests/cases/`. The harnesses support focused runs by
passing one or more case files or directories.
