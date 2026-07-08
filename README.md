# Gaman

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Gaman is an offline-first schema migration engine that generates deterministic
migrations without connecting to a live database.

The crate is intentionally just a Rust library plus a CLI.

It starts from a simple idea: your committed migration history is enough to know
where the schema is, and your desired schema is enough to plan where it should
go next.

```text
desired schema ─┐
                ├─► Schema IR ─► deterministic replay + diff ─► migration YAML
migration log ──┘                                              │
                                                               ▼
                                                        dialect SQL
```

> **Project status:** Early-stage. Core behavior is tested and usable, but
> public API and file format may still change before 1.0.

PostgreSQL is the default and broadest supported engine. SQLite is supported
behind the `sqlite` Cargo feature as its own engine, not as a PostgreSQL
compatibility mode.

## Why Use It

- Migration generation is deterministic and offline.
- `sql_migrate` renders the SQL plan without opening a database connection.
- YAML, JSON, SQL DDL, Rust builders, and live inspection all feed one schema model.
- Ambiguous or risky changes are surfaced before files are written.
- Rust applications can use the same `MigrationEngine` API as the CLI.
- Storage is pluggable: directories, embedded structs, in-memory stores, and future browser buffers are adapters.

## Quick Start

```bash
cargo install gaman
export DATABASE_URL=postgres://localhost/myapp
export MIGRATIONS_DIR=migrations
export SCHEMA=schema.yaml

gaman make_migration initial
gaman sql_migrate
gaman migrate
gaman verify_db
```

The installed CLI includes all currently supported live dialects: PostgreSQL and
SQLite. MySQL/MariaDB remains planned and is not included in the supported CLI
profile yet.

The loop is intentionally small:

```text
schema.yaml -> make_migration -> migration.yaml -> sql_migrate -> SQL -> migrate
```

Offline commands can run without `DATABASE_URL` when the dialect is explicit:

```bash
gaman make_migration add_posts --dialect postgres
gaman sql_migrate --dialect sqlite
```

For smaller custom builds, select dialect features explicitly:

```bash
cargo install gaman --no-default-features --features cli,postgres
cargo install gaman --no-default-features --features cli,sqlite
```

Use `--non-interactive` in CI when prompts should fail the run instead of
waiting for input.

Gaman does not load `.env` automatically. Use `--env .env` when you want local
dotenv-style configuration.

## CLI Reference

Global flags come before the subcommand:

- `-m <dir>` overrides `MIGRATIONS_DIR`.
- `-s <file-or-dir>` / `--schema <file-or-dir>` overrides `SCHEMA`.
- `-d <url>` overrides `DATABASE_URL`.
- `--env <file>` loads environment variables from a dotenv file before config resolution.
- `--dialect postgres|sqlite` selects the renderer for offline commands.

Everyday commands:

```bash
gaman make_migration [name]       # diff schema and write the next migration
gaman make_migration --check      # CI check; never prompts or writes
gaman make_migration --dry-run    # print the migration that would be written
gaman make_migration --empty name # write an empty migration shell
gaman make_migration --merge name # merge multiple graph heads

gaman sql_migrate [id]            # print offline operation SQL
gaman sql_migrate --backwards id  # print rollback SQL

gaman migrate                     # apply pending migrations
gaman migrate --target id         # migrate forward or backward to id
gaman migrate --fake              # record as applied without running DDL
gaman migrate --plan              # print the live migration plan
gaman migrate --check             # fail if anything is pending

gaman inspect_db                  # export live schema
gaman inspect_db --table users    # export one table
gaman verify_db                   # compare live DB against replayed history
gaman show_migrations             # list applied/pending migrations
gaman config                      # print resolved config
```

Environment variables:

- `DATABASE_URL`: required for `migrate`, `inspect_db`, and `verify_db`.
- `MIGRATIONS_DIR`: defaults to `migrations`.
- `SCHEMA`: defaults to `schema.yaml`; may be YAML, JSON, SQL, or a directory.

## Support

Migration files are engine-specific. Gaman does not try to make one migration
portable across PostgreSQL, SQLite, and future engines.

The table is generated from checked-in evidence snapshots plus explicit design
metadata for unsupported-by-design rows. Offline rows come from deterministic
fixture results; live rows require database-backed evidence.

Legend: ✅ accepted evidence, ◐ bounded support, 🚧 planned or not evidenced
yet, ❌ unsupported by design or by the database engine.

`inspect_db` is the high-fidelity reflection path for onboarding existing
projects into Gaman. `verify_db` is narrower by design: each dialect owns a
static registry of entity properties that live inspection can recover accurately
and deterministically. Opaque objects are still tracked by presence and stable
metadata, but their bodies are not drift inputs unless a dialect-specific
verifier can inspect them deterministically.

<!-- gaman:support-matrix:start -->
| Feature | PostgreSQL | SQLite | MySQL / MariaDB |
| --- | --- | --- | --- |
| Offline replay, diff, and migration generation | ✅ | ✅ | 🚧 |
| Offline SQL rendering through `sql_migrate` | ✅ | ✅ | 🚧 |
| Live migration application | ✅ | ✅ | 🚧 |
| Live database introspection | ✅ | ✅ | 🚧 |
| Live `verify_db` | ✅ | ✅ | 🚧 |
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
| Partial indexes | ✅ | ◐ | 🚧 |
| Concurrent indexes | ✅ | ❌ | 🚧 |
| Schemas / namespaces | ✅ | ❌ | ❌ |
| Extensions as opaque schema objects | 🚧 | ❌ | ❌ |
| Enums | ✅ | ❌ | 🚧 |
| Functions as opaque schema objects | ✅ | ❌ | 🚧 |
| Trigger query schema objects | ✅ | ✅ | 🚧 |
| Views as opaque schema objects | ✅ | ✅ | 🚧 |
| Raw SQL statements | ✅ | ✅ | 🚧 |
| SQLite table-rebuild planner for ALTER TABLE | ❌ | ✅ | ❌ |
| Opaque source formatting fallback in offline diff | ✅ | ✅ | 🚧 |
| Ownership-scoped `verify_db` | ✅ | ✅ | 🚧 |

Notes:
- Dedicated migration lock (sqlite): SQLite has no dedicated advisory-lock primitive; migration atomicity relies on SQLite transactions and file locking.
- Automatic primary-key mutation generation (postgres/sqlite/mysql): Primary-key surgery is intentionally manual/raw SQL for every dialect.
- Partial indexes (sqlite): SQLite partial-index SQL rendering is proven offline; live predicate introspection/verify is not yet accepted evidence.
- Concurrent indexes (sqlite): SQLite has no CREATE INDEX CONCURRENTLY syntax.
- Schemas / namespaces (mysql): MySQL does not use PostgreSQL-style schemas/namespaces in Gaman.
- Schemas / namespaces (sqlite): SQLite does not support PostgreSQL-style schemas/namespaces in Gaman.
- Extensions as opaque schema objects (mysql): MySQL extensions are not modeled as migratable schema objects.
- Extensions as opaque schema objects (sqlite): SQLite extensions are not modeled as migratable schema objects.
- Enums (sqlite): SQLite has no native enum schema object in Gaman.
- Functions as opaque schema objects (sqlite): SQLite stored functions are not supported by Gaman.
- SQLite table-rebuild planner for ALTER TABLE (postgres): PostgreSQL uses native ALTER TABLE paths; SQLite rebuild planning does not apply.
- SQLite table-rebuild planner for ALTER TABLE (mysql): SQLite rebuild planning does not apply to MySQL.
<!-- gaman:support-matrix:end -->

Offline parser, replay, diff, clarification, rollback, and SQL-rendering
evidence is tracked separately from live product support. See `TESTING.md` for
the checked offline evidence matrix and result-recording commands.

## Author Schema

All frontends normalize into the same internal `Schema` before replay, diffing,
clarification, and SQL rendering.

YAML is explicit and reviewable:

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

SQL DDL works when schema already lives as SQL:

```sql
CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);
```

Rust uses builders and traits, not a Gaman-owned model derive:

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

`IntoTable` remains a plain trait, so model/query crates such as Mool can derive
or implement it without Gaman owning Rust model macros.

## Embedded Migration Sources

Gaman keeps `EmbeddedMigrations` as a plain data structure and migration source
adapter. It does not provide an embedding macro; external framework/model crates
can construct or return this Gaman-compatible shape.

Multiple crates can still compose migration trees:

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
Use `new` when you already have an `EmbeddedMigrations` value from Mool or a
manual static definition; use `from_source` for custom storage.

```rust
use gaman::{Config, MigrationEngine};
use gaman::core::Dialect;

let schema = gaman::schema_file::load_schema_path("schema.yaml", Dialect::Postgres)?;
let engine = MigrationEngine::new(Config::default(), &MIGRATIONS)
    .with_dialect(Dialect::Postgres)
    .with_schema(|_| schema)?;
```

Common methods:

```rust
engine.sql_migrate()?;                         // offline operation SQL
engine.sql_migrate_id("0002_add_posts")?;      // one migration
engine.sql_rollback(&["0002_add_posts"])?;     // offline rollback SQL
engine.make_migration_non_interactive(None)?;  // CI-safe generation
engine.make_migration_check()?;                // fail if schema changed
engine.inspect_table(&["public"], "users").await?;
engine.verify("public").await?;
```

Live actions require a database connection. Offline SQL planning does not.

Custom storage implements `MigrationSource`; it can be file-backed, embedded,
in-memory, or application-owned.

## Clarification

Gaman does not guess through risky changes. Renames, new `NOT NULL` columns,
type casts, and newly introduced unknown data types can require decisions before
a migration is written.

Unknown data types use trust on first use. Catalogs know common aliases and
popular extension types, but committed migrations are the project-local approval
log for custom domains, composites, extension types, and user-defined types.

## Why Not...

Gaman is not a universal DDL modeler or a live-database-first planner.

- Flyway-style tools apply ordered SQL files; Gaman helps create them from a schema model.
- Atlas-style tools inspect databases deeply; Gaman’s generation path is offline.
- Diesel keeps migrations close to Rust; Gaman treats Rust as one frontend into a shared schema IR.
- Handwritten SQL remains valuable; `Statement` is the escape hatch.

## Design Boundaries

- Column-level `references` is single-column shorthand. Use table-level
  `foreign_keys` for composite references.
- Composite primary keys and foreign keys are canonical table-level metadata.
- Primary-key mutation generation is intentionally unsupported; use raw
  `Statement` operations for backend-specific PK surgery.
- Opaque source text is preserved exactly. Lexical canonicalization is used only
  to suppress formatting-only diff churn.
- `inspect_db` preserves useful reflected catalog state for onboarding existing
  projects, even when that state is not part of drift verification.
- `verify_db` compares deterministic inspected properties only and reports the
  entity property that drifted. It does not prove function, trigger, or view
  body equivalence from live catalog text.
- PostgreSQL trigger `query` source is wrapped in generated trigger functions
  with default return behavior. Use explicit functions for custom returns.
- SQLite table rebuilds require `atomic: true`; unsafe rebuilds fail early.
- SQLite live introspection is intentionally narrower than authored schema metadata.

## Development

```bash
cargo test
cargo test --test offline
cargo test --features sqlite --test online
```

Online PostgreSQL tests need a database:

```bash
export POSTGRES_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test online -- --dialect postgres
```

More detail lives in:

- [ARCHITECTURE.md](ARCHITECTURE.md)
- [TESTING.md](TESTING.md)
