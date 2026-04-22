# Gaman

> **Early-stage. Core engine is stable and tested in production use. Public API and file format may change before 1.0.**

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A deterministic, offline-first migration engine for PostgreSQL, written in Rust and inspired by Django migrations.

Declare your schema as **YAML**, **SQL DDL**, or **Rust structs**. Gaman diffs that desired state against replayed migration history and writes the next migration without touching a database at plan time.

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

## How It Works

Gaman treats migration planning as a pure replay problem.

It takes the schema you want now, reconstructs the previous schema by replaying every existing migration, and diffs those two states. Because both sides are reduced to the same internal `Schema`, there is no hidden database state involved in generation.

```
schema.yaml  ─┐
schema.sql   ─┼─► Schema (IR) ──┐
Rust structs ─┘                  ├─► diff ──► new migration file
migrations/ (replayed) ──────────┘
```

That is what makes the engine deterministic: the same schema input and the same migration history always produce the same next migration.

Migrations form a DAG. Each file declares its `dependencies`, so parallel feature branches can meet in an explicit merge migration:

```
0001_initial → 0002_add_auth ──┐
             → 0003_add_posts ─┴→ 0004_merge
```

## Quick Start

### CLI

```bash
cargo install gaman
```

Point Gaman at a database, a migrations directory, and a schema file:

```bash
DATABASE_URL=postgres://localhost/myapp
MIGRATIONS_DIR=migrations
SCHEMA_FILE=schema.yaml   # or schema.sql
```

```bash
gaman make_migration initial   # diff schema -> write migration
gaman sql_migrate              # preview SQL without a database
gaman migrate                  # apply pending migrations
```

Typical loop:

1. Change the schema.
2. Run `gaman make_migration add_whatever_changed`.
3. Review the generated YAML or `gaman sql_migrate` output.
4. Run `gaman migrate`.

### Embedded in Rust

```toml
[dependencies]
gaman = "0.3"
```

Use the library when you want migrations to ship with your binary. `include_migrations!("path")` embeds every `.yaml` file at compile time; `MigrationEngine` runs them.

```rust
use gaman::{Config, MigrationEngine, include_migrations};

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    let n = MigrationEngine::new(Config::default(), MIGRATIONS)
        .migrate()
        .expect("migrations failed");
    if n > 0 {
        eprintln!("{n} migration(s) applied");
    }
}
```

`MigrationEngine` also exposes `check()`, `plan()`, `show_migrations()`, `verify()`, `inspect_db()`, `make_migration()`, and `handle_args()` for the full CLI surface. See [docs/embedding.md](docs/embedding.md) for the complete API reference.

## Declaring Schema

All schema inputs become the same internal `Schema`, so the format choice is mostly about how you want to author it.

### YAML

YAML is the most explicit format. It works well when you want schema, indexes, views, functions, and extensions in one place.

```yaml
extensions:
  pgcrypto: {}

tables:
  users:
    columns:
      - name: id
        type: bigserial
        nullable: false
        primary_key: true
      - name: email
        type: text
        nullable: false
      - name: created_at
        type: timestamptz
        nullable: false
        default: "now()"
    indexes:
      - name: users_email_idx
        columns: [email]
        unique: true
```

Inline shorthands such as `primary_key`, `references`, and `check` are normalized before diffing. `SCHEMA_FILE` can point to a `.yaml`, a `.sql`, or a directory; when you pass a directory, Gaman merges files in alphabetical order.

### SQL DDL

SQL is useful when you already maintain schema DDL by hand and want Gaman to diff from that source of truth.

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);
```

Supported statements today are `CREATE TABLE`, `CREATE [UNIQUE] INDEX`, `CREATE VIEW`, `CREATE FUNCTION`, `CREATE EXTENSION`, and `CREATE TYPE AS ENUM`.

### Rust Structs

Rust structs fit best when the application already owns the schema in code.

```rust
use gaman::IntoTable;

#[derive(IntoTable)]
#[table(name = "users", schema = "app")]
struct User {
    id: i64,
    email: String,
    bio: Option<String>,
    #[column(type = "timestamptz")]
    created_at: chrono::DateTime<chrono::Utc>,
    #[column(type = "uuid", nullable)]
    invite_code: Option<uuid::Uuid>,
    #[column(default = "0")]
    score: i32,
    #[column(references = "orgs.id")]
    org_id: i64,
}
```

The common derive controls are `#[table(name = "...")]`, `#[table(schema = "...")]`, `#[column(type = "...")]`, `#[column(nullable)]`, `#[column(default = "...")]`, `#[column(references = "table.col")]`, and `#[column(skip)]`.

For the full derive reference, see [docs/rust-structs.md](docs/rust-structs.md).

## Migration Files

`make_migration` writes readable YAML. Most migrations stay auto-generated; you only edit them by hand when the change needs something explicit.

```yaml
id: 0003_add_posts
dependencies: [0002_add_users]
atomic: true
operations:
  - type: create_table
    table:
      name: posts
      columns:
        - { name: id, type: bigserial, nullable: false, primary_key: true }
        - { name: title, type: text, nullable: false }
        - { name: author_id, type: bigint, nullable: false }
      foreign_keys:
        - name: fk_posts_author
          columns: [author_id]
          to_table: users
          to_column: id
```

Every migration runs in one transaction by default. Set `atomic: false` only for PostgreSQL operations that cannot run transactionally, most notably `CREATE INDEX CONCURRENTLY`.

```yaml
id: 0004_add_search_idx
dependencies: [0003_add_posts]
atomic: false
operations:
  - type: add_index
    table_name: posts
    index:
      name: posts_title_idx
      columns: [title]
      unique: false
    concurrent: true
```

When `concurrent: true` is present, Gaman validates that `atomic: false` is also set.

## Escape Hatches

Sometimes the generated migration is structurally correct but still needs a hand-written step. Use `statement` to embed raw SQL that runs inside the migration's transaction:

```yaml
operations:
  - type: statement
    up: "UPDATE users SET role = 'member' WHERE role IS NULL"
    down: "UPDATE users SET role = NULL WHERE role = 'member'"
```

## Disambiguator

The diff engine stays conservative on purpose. A renamed column looks exactly like a drop plus an add unless you tell the tool otherwise. Before writing a migration, Gaman flags ambiguous changes and, in interactive mode, asks for confirmation.

- `Fatal`: `NotNullAdd` and `NotNullChange`, where the migration would fail or needs a backfill first.
- `Warning`: `TypeCast`, where the change needs an explicit cast or relies on PostgreSQL coercion.
- `Suggestion`: `RenameColumn` and `RenameTable`, where a human likely meant rename rather than drop and recreate.

For `NotNullChange`, a backfill `UPDATE` is automatically injected before the `ALTER COLUMN`.

## CLI and Config Reference

The complete command reference, flag details, and environment variables now live in [docs/cli.md](docs/cli.md).

The short version:

- `gaman make_migration name` writes the next migration by diffing your schema against replayed history.
- `gaman migrate` applies pending migrations.
- `gaman sql_migrate` prints SQL without touching a database.
- `gaman verify_db` compares the live database against replayed state.
- `gaman inspect_db` bootstraps a `schema.yaml` from an existing database.

Global overrides stay the same: `-m <dir>`, `-s <file>`, `-d <url>`.

## Development

```bash
cargo test

# Offline transform cases.
cargo test --test offline

# Integration tests require a running PostgreSQL instance.
export TEST_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test postgres -- --include-ignored
```

Integration tests create and destroy isolated schemas automatically; they leave no lasting state.

Offline transform cases live under `tests/cases/offline/`, and PostgreSQL-backed cases live under `tests/cases/postgres/`. Each case is a single `case.yaml` with inline inputs and expected outputs.

## Status

PostgreSQL only. Core migration engine is stable and used in production. Public API may change before 1.0.

### Known limitations

- Single-column primary and foreign keys only
- Column order is not tracked
- `verify_db` does not validate view, function, extension, or enum definitions
- `alter_enum` has no inverse, so migrations containing it cannot be rolled back
