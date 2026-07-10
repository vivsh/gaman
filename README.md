# Gaman

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Gaman is an offline-first schema migration CLI that generates deterministic
migrations without connecting to a live database.

The crate also exposes the same engine as a Rust library for applications that
need embedded migrations or custom storage.

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
- `sql` renders the SQL plan without opening a database connection.
- `schema.sql` uses supported database `CREATE` DDL and direct database types.
- YAML, JSON, SQL DDL, and live inspection all feed one schema model.
- Ambiguous or risky changes are surfaced before files are written.
- Rust applications can embed the same engine when they need custom migration storage.

## Quick Start

```bash
cargo install --locked gaman

export DATABASE_URL=postgres://localhost/myapp
export MIGRATIONS_DIR=migrations
export SCHEMA=schema.sql

gaman check_schema
gaman make initial
gaman sql
gaman apply
gaman verify
```

The installed CLI includes all currently supported live dialects: PostgreSQL and
SQLite. MySQL/MariaDB remains planned and is not included in the supported CLI
profile yet.

The loop is intentionally small:

```text
schema.sql -> make -> migration.yaml -> sql -> SQL -> apply
```

`DATABASE_URL` is required for every CLI invocation only to select the dialect.
Offline commands such as `make`, `show`, and `sql` never open or otherwise use
the connection. `check_schema` is the deliberate validation exception: it
prepares schema SQL through the selected database without executing it. Other
live commands include `status`, `apply`, `inspect`, `verify`, and `repair`.

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

Everyday commands:

```bash
gaman make [name]       # diff schema and write the next migration
gaman make --check      # CI check; never prompts or writes
gaman make --dry-run    # print the migration that would be written
gaman make --empty name # write an empty migration shell
gaman make --merge name # merge multiple graph heads
gaman check_schema       # prepare each SQL schema statement without executing it

gaman sql [id]            # print offline operation SQL; id may be a unique prefix
gaman sql --backwards id  # print rollback SQL; id may be a unique prefix

gaman apply                     # apply pending migrations
gaman apply id                  # converge forward or backward to a unique id prefix
gaman apply --fake              # update tracking without running migration SQL
gaman apply --plan              # print the live migration plan
gaman apply --check             # fail if anything is pending

gaman inspect                              # export live schema
gaman inspect --schema billing --table users # export one unambiguous table
gaman verify --schema public --schema billing # verify multiple owned schemas
gaman repair --schema billing              # plan one-off drift repair SQL
gaman repair --apply           # apply one-off drift repair SQL
gaman status                   # list applied/pending migrations
gaman show [id]                # show canonical migration YAML; id may be a unique prefix
gaman config                    # print resolved config with a redacted URL
gaman config --show-database-url # print the full database URL explicitly
```

`gaman apply [id]` follows Django-style target semantics: it converges the
database on the selected migration, applying or reverting migrations as needed.
The target itself remains applied. Use `gaman sql --backwards id` to inspect the
inverse SQL before moving backward.

Environment variables:

- `DATABASE_URL`: required for all CLI commands; offline commands use it only
  to select the dialect. `check_schema` connects only to prepare SQL and never
  executes it.
- `MIGRATIONS_DIR`: defaults to `migrations`.
- `SCHEMA`: defaults to `schema.yaml`; may be YAML, JSON, SQL, or a directory.

Only commands that write migration files require a writable migrations
directory. Artifact inspection, SQL rendering, live application, inspection,
verification, and repair can read migrations from read-only deployments.

## Support

Migration files are engine-specific. Gaman does not try to make one migration
portable across PostgreSQL, SQLite, and future engines.

The table is generated from checked-in evidence snapshots plus explicit design
metadata for unsupported-by-design rows. Offline rows come from deterministic
fixture results; live rows require database-backed evidence.

Legend: ✅ accepted evidence, ◐ bounded support, 🚧 planned or not evidenced
yet, ❌ unsupported by design or by the database engine.

`inspect` is the high-fidelity reflection path for onboarding existing
projects into Gaman. `verify` is narrower by design: each dialect owns a
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
| Extensions as opaque schema objects | ✅ | ❌ | ❌ |
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

## Schema Input

All frontends normalize into the same internal `Schema` before replay, diffing,
clarification, and SQL rendering.

### Schema SQL First

`schema.sql` is the primary schema-authoring format. It contains supported
`CREATE` DDL declarations for the selected database dialect:

```sql
CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);
```

Use the database's own type names: `bigserial`, `text`, `timestamptz`, `jsonb`,
`integer`, and so on. Gaman has no separate type language and does not convert
application types into database types. It recognizes common aliases to validate
and canonicalize them, but the schema remains database DDL. See [Clarification](#clarification) for how unrecognized types are approved.

PostgreSQL recognition covers its stable user-declarable built-in types and
aliases from PostgreSQL 14 onward, including ranges, multiranges, `jsonpath`,
and native `uuid`. `pgcrypto` is an optional known extension for functions such
as `gen_random_uuid()`; it does not provide the `uuid` type. SQLite preserves
the declared type text and uses SQLite's documented affinity rules for semantic
comparison. These catalogs improve diagnostics only: custom, domain, composite,
and unlisted extension types still use trust on first use (TOFU).

`schema.sql` describes desired state; it is not a hand-written migration file.
When it changes, Gaman compares its prepared schema with replayed migration
history, generates a migration, and then applies that migration to update the
database. Schema SQL accepts supported `CREATE` definitions only; use generated
migrations or explicit raw migration SQL for structural `ALTER` and `DROP`
work. Data changes are outside schema input and remain application-owned SQL.

### YAML And Other Structured Input

YAML is available when a structured schema is a better fit:

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

YAML and JSON use the same direct database type strings as `schema.sql`; they
do not introduce an alternate type vocabulary. Rust builders and live inspection
also feed this same schema model.

Rust applications can also use builders, `EmbeddedMigrations`, `MigrationSource`,
and `MigrationEngine` directly. See [Embedding Gaman In Rust](docs/rust-embedding.md).

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
- `inspect` preserves useful reflected catalog state for onboarding existing
  projects, even when that state is not part of drift verification.
- `verify` compares deterministic inspected properties only and reports the
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
- [Embedding Gaman In Rust](docs/rust-embedding.md)
