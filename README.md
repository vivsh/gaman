# Gaman

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

Gaman is a standalone, schema-first migration engine and CLI for teams that
want a Django-like migration workflow without coupling database evolution to a
web framework.

Describe the database you want, including the small set of application-owned
rows that should evolve with its schema. Gaman replays committed migration
history, calculates the change offline, writes a reviewable migration, renders
dialect SQL, and can verify the deployed database for drift.

Schema tracking is deliberately **DDL `CREATE`-only**, with one bounded
exception: top-level managed rows. Gaman tracks desired definitions for tables,
columns, keys, constraints, indexes, enums, extensions, PostgreSQL sequences, functions, triggers,
and views. Managed rows let applications own a small, explicitly identified
set of data records. Arbitrary DML, transformations, `ALTER`, and `DROP` remain
migration operations rather than desired schema input.

> **Project status:** Early-stage and usable, but public APIs and file formats
> may still change before 1.0.

## Demo

This 40-second PostgreSQL demo creates an initial migration, applies it,
clarifies a risky schema change, and verifies the result.

![Gaman schema migration demo](docs/assets/gaman-demo.gif)

## Safe Changes Are Automatic. Risky Changes Ask.

Like Django's migration workflow, Gaman does not force every schema change into
a flag-heavy command. When intent is ambiguous, it pauses and asks a focused
question before writing the migration:

```text
[suggest] Column 'email' was removed from 'users'. Was it renamed?
  1 - email_address
  2 - No, it was dropped
```

For focused changes, `gaman make task-lanes --filter table::vyuh_task_lanes`
generates only the selected root and required changed dependencies. Filters are
temporary selectors for one invocation, not persistent staging; the next
unfiltered `make` still sees every remaining schema change.

Clarification covers possible table, column, and enum-value renames, new
non-null data requirements, type casts, unfamiliar database types, and coarse
opaque-object changes. The answer becomes part of committed migration history,
so replay remains deterministic and the question is not asked again.

Interactive prompts are for local development. In CI, `--non-interactive`
turns an unresolved clarification into a clear failure instead of guessing or
waiting for input.

## Why Gaman

- **Plan offline.** `make`, `show`, and `sql` use desired schema plus committed
  migrations; they do not inspect or modify a database.
- **Review the result.** Migrations are deterministic YAML artifacts, and the
  exact dialect SQL can be printed before application.
- **Clarify risk.** Renames, casts, new non-null columns, and unfamiliar types
  are surfaced instead of guessed through.
- **Keep production honest.** `inspect` reflects a live database, `verify`
  reports expected and observed properties, and `repair` plans bounded fixes.
- **Track more than tables.** Keys, constraints, indexes, enums, extensions,
  PostgreSQL sequences, functions, triggers, and views participate in migration ownership.
- **Keep sequence ownership bounded.** PostgreSQL sequence definitions and
  presence are migration-owned opaque roots. Gaman never inspects or repairs
  counter state, and rejects temporary sequences and `OWNED BY` declarations.
- **Order functions explicitly.** Function defaults and declared dependencies
  are modeled. In YAML and Rust, use exact `kind::target` selectors. In SQL,
  attach repeatable leading `-- @depends-on function::name(...)` comments to
  `CREATE FUNCTION`. `function::name` is valid only for a unique overload;
  `function::name()` and typed signatures select exact overloads. Leading `@`
  directives are reserved: unknown directives fail rather than being ignored.
- **Version application-owned rows.** Declare stable records by a primary or
  non-null unique key without claiming ownership of unrelated table data.
- **Choose the input that fits.** SQL DDL, YAML, JSON, and live inspection
  converge on the same schema lifecycle.
- **Keep an escape hatch.** Advanced objects can use preserved raw SQL when a
  granular model would be misleading.

## Typical Use Cases

- Add schema-to-migration generation to a Rust service or any non-Django stack.
- Keep a database-native `schema.sql` as the desired state of a project.
- Generate and review migration operations and SQL in CI.
- Onboard an existing project by exporting its database with `inspect`.
- Detect deployment drift with property-level expected/observed diagnostics.
- Use the same migration model through integrations such as
  [Mool](https://github.com/vivsh/mool).

## Quick Start

Install the CLI and point it at a PostgreSQL project:

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

`make` and `sql` are offline. `check_schema` asks the database to prepare SQL
without executing it. `apply`, `status`, `inspect`, `verify`, and `repair` are
live commands.

The default build includes PostgreSQL and SQLite. Smaller builds can select one
dialect explicitly:

```bash
cargo install gaman --no-default-features --features cli,postgres
cargo install gaman --no-default-features --features cli,sqlite
```

Gaman does not load `.env` automatically. Use `gaman --env .env <command>` when
you want dotenv-style configuration.

## Managed Rows

Managed rows version a bounded set of application data alongside the schema.
They are declared separately from tables, so a table in SQL can be paired with
rows in YAML or JSON:

```yaml
managed_rows:
  vyuh_task_lanes:
    rows:
      - id: approval
        name: manager_review
        properties:
          requires_manager: true
```

The table must exist in the composed schema. Each row must include its primary
key or an eligible non-null unique key, which Gaman infers from the table. A row
may manage only a subset of columns; other rows and columns remain outside
Gaman ownership.

Treat declared values like schema: change them through schema files and
generated migrations, not application code or manual SQL. External changes are
reported as drift, and checked writes refuse to overwrite unexpected values.

## How It Works

```text
desired schema ─┐
                ├─► normalize ─► replay + diff ─► migration YAML ─► SQL
migration log ──┘                                      │
                                                       ▼
                                              apply / verify / repair
```

The desired schema is compared with the schema reconstructed from committed
migrations. That produces a deterministic migration without treating the live
database as planning state. Live inspection is a separate lifecycle used for
onboarding, drift verification, and repair.

When a change has more than one plausible interpretation or carries material
risk, Gaman returns a clarification request before writing the migration. In
CI, `--non-interactive` turns unresolved clarification into a failure.

## CLI

| Command | Purpose |
| --- | --- |
| `check_schema` | Prepare schema SQL without executing it |
| `make [name]` | Generate the next migration from desired state |
| `show [id]` | Show migration YAML |
| `sql [id]` | Render forward or backward SQL offline |
| `apply [id]` | Apply pending migrations or converge on a target |
| `status` | Show applied and pending migrations |
| `inspect` | Export reflected live database state |
| `verify` | Compare replayed history with the live database |
| `repair` | Plan or apply one-off verified drift repair |
| `config` | Show resolved, redacted configuration |

Global options come before the command:

```bash
gaman --env .env -s schema.sql -m migrations -d postgres://localhost/myapp make
```

Configuration can also come from environment variables:

| Variable | Meaning | Default |
| --- | --- | --- |
| `DATABASE_URL` | Database target and dialect selection | required |
| `MIGRATIONS_DIR` | Migration artifact directory | `migrations` |
| `SCHEMA` | SQL, YAML, JSON, or schema directory | `schema.yaml` |

Run `gaman --help` or `gaman <command> --help` for the complete option set.

## Database Support

Migration files are dialect-specific. Gaman does not pretend that one migration
is portable across engines.

Legend: ✅ accepted evidence, ◐ bounded support, 🚧 planned or not evidenced
yet, ❌ unsupported by design or by the database engine.

<!-- gaman:support-matrix:start -->
<!-- evidence-generation: 20260819T075134Z-95479 -->
| Feature | PostgreSQL | SQLite | MySQL | MariaDB |
| --- | --- | --- | --- | --- |
| Offline replay, diff, and migration generation | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Live migration application | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [◐](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Live database introspection | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Live `verify_db` | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Tables: create, drop, rename | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [◐](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Columns: add, drop, rename | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Columns: type, nullability, default changes | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [◐](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Multi-column / composite primary keys | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Single-column foreign keys | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Multi-column / composite foreign keys | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Unique constraints | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Indexes | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Extensions as opaque schema objects | [✅](docs/support-evidence.md#lifecycle-compatibility) | [❌](docs/support-evidence.md#lifecycle-compatibility) | [❌](docs/support-evidence.md#lifecycle-compatibility) | [❌](docs/support-evidence.md#lifecycle-compatibility) |
| Enums | [✅](docs/support-evidence.md#lifecycle-compatibility) | [❌](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Functions as opaque schema objects | [✅](docs/support-evidence.md#lifecycle-compatibility) | [❌](docs/support-evidence.md#lifecycle-compatibility) | [◐](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
| Trigger query schema objects | [✅](docs/support-evidence.md#lifecycle-compatibility) | [✅](docs/support-evidence.md#lifecycle-compatibility) | [◐](docs/support-evidence.md#lifecycle-compatibility) | [🚧](docs/support-evidence.md#lifecycle-compatibility) |
<!-- gaman:support-matrix:end -->

PostgreSQL has the broadest coverage. SQLite uses engine-specific table rebuilds
for changes its native `ALTER TABLE` cannot express. MySQL support is useful but
still bounded in parts of the live lifecycle. MariaDB has parser and offline
coverage but does not yet have accepted live-server evidence.

The [detailed support evidence](docs/support-evidence.md) is authoritative. It
contains the complete feature matrix, limitations, parser boundaries, live
fixtures, and the exact properties used by drift verification.

## Schema Input

SQL DDL is the primary authoring format and uses the database's own type names:

```sql
CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);
```

Structured YAML is available when it is a better fit:

```yaml
tables:
  users:
    columns:
      - { name: id, type: bigserial, primary_key: true }
      - { name: email, type: text, nullable: false }
    indexes:
      - { name: users_email_idx, columns: [email], unique: true }
```

YAML and JSON use the same database-native type strings and prepare into the
same schema model. SQL schema input describes desired `CREATE` state only;
`ALTER`, `DROP`, DML, data migrations, and unsupported structural surgery remain
explicit migration SQL rather than tracked desired state.

PostgreSQL sequences may be declared with `CREATE SEQUENCE`, through
`SchemaBuilder::opaque`, or under structured `sequences` entries containing
`sql`. Gaman owns the normalized definition and presence only; runtime counter
values are intentionally outside desired state.

## Where Gaman Fits

| Approach | Primary model | Gaman's distinction |
| --- | --- | --- |
| Django migrations | Framework-owned models and migration state | Standalone and database-oriented |
| Flyway/Liquibase-style tools | Apply authored migration files | Generates reviewable migrations from desired schema |
| Atlas-style tools | Inspection-led schema planning | Plans from committed history; inspection verifies separately |
| Handwritten SQL | Complete manual control | Generates common changes while retaining raw SQL escape hatches |

These are different workflow choices, not claims that one tool replaces every
other. Gaman is most useful when desired schema, committed migration history,
reviewable SQL, and deployment verification should remain distinct.

## Honest Boundaries

- Tables and columns are modeled for granular migration generation.
- Advanced non-table objects may be preserved as opaque SQL and changed
  through coarse create/drop/replace operations.
- Primary-key mutation is intentionally manual.
- Opaque body/source changes are not live drift inputs; opaque objects are
  generally verified by owned presence and stable modeled metadata.
- `inspect` is broader than `verify`: reflection helps onboarding, while drift
  comparison includes only properties a dialect can recover deterministically.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the complete lifecycle contract.

## Ecosystem

[Mool](https://github.com/vivsh/mool) integrates Gaman migration generation into
a Rust ORM workflow and provides a concrete example of Gaman used beyond the
standalone CLI.

## Project Documentation

- [Architecture](ARCHITECTURE.md)
- [Testing](TESTING.md)
- [Detailed support evidence](docs/support-evidence.md)
- [Command protocol](docs/command-protocol.md)
