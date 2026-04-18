# Gaman — गमन

> **Early-stage. Core engine is stable and tested in production use. Public API and file format may change before 1.0.**

[![Crates.io](https://img.shields.io/crates/v/gaman)](https://crates.io/crates/gaman)
[![License: MIT](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

A deterministic, offline-first migration engine for PostgreSQL — written in Rust, inspired by Django migrations.

Declare your schema as **YAML**, **SQL DDL**, or **Rust structs**. Gaman diffs it against your migration history and generates the next migration — no database connection required at plan time.

_Pronounced guh-MUN (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward"._

---

## How It Works

No matter how you declare your schema, every input path is parsed into the same intermediate representation (`Schema`) before any comparison happens:

```
schema.yaml  ─┐
schema.sql   ─┼─► Schema (IR) ──┐
Rust structs ─┘                  ├─► diff ──► new migration file
migrations/ (replayed) ──────────┘
```

- **Desired state** — your schema file (YAML, SQL DDL, or Rust structs).
- **Previous state** — fully reconstructed by replaying all existing migrations in topological order. No database needed.
- **Diff** — the ordered set of operations between the two states, written to a new migration YAML file.
- **Apply** — `gaman migrate` executes pending migrations and records them in `gaman_migrations`.

Migrations form a **directed acyclic graph (DAG)**. Each file declares its `dependencies`, which enables parallel feature branches and explicit merge points:

```
0001_initial → 0002_add_auth ──┐
             → 0003_add_posts ─┴→ 0004_merge
```

---

## Quick Start

### CLI

```bash
cargo install gaman
```

Configure via environment variables or per-invocation flags (`-d`, `-m`, `-s`):

```bash
DATABASE_URL=postgres://localhost/myapp
MIGRATIONS_DIR=migrations
SCHEMA_FILE=schema.yaml   # or schema.sql
```

```bash
gaman make_migration initial   # diff schema → write migration
gaman sql_migrate               # preview SQL — no DB needed
gaman migrate                   # apply to database
```

### Embedded in Rust

```toml
[dependencies]
gaman = "0.3"
```

**Auto-apply at startup:**

```rust
use gaman::{Config, MigrationEngine, include_migrations};

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), MIGRATIONS)
        .migrate()
        .expect("migrations failed");
}
```

**Expose the full CLI from your binary — struct-based schema:**

```rust
use gaman::{Config, IntoTable, MigrationEngine, include_migrations};

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

#[derive(IntoTable)]
struct User { id: i64, email: String, bio: Option<String> }

fn main() {
    MigrationEngine::new(Config::default(), MIGRATIONS)
        .with_schema(|s| s.table::<User>().build())
        .handle_args()
        .expect("command failed");
}
```

**Expose the full CLI — file-based schema:**

```rust
use gaman::{Config, MigrationEngine, include_migrations};
use gaman::schema::Schema;

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), MIGRATIONS)
        .with_schema(|_| Schema::load(std::path::Path::new("schema.sql"))
            .expect("failed to load schema"))
        .handle_args()
        .expect("command failed");
}
```

`handle_args()` parses `std::env::args()` and dispatches `make_migration`, `migrate`, `verify_db`, `show_migrations`, and more. CLI flags override `Config`.

`include_migrations!("path")` embeds all `.yaml` files at compile time, sorted lexicographically. No files needed at runtime.

---

## Schema Formats

### YAML

Column types are passed verbatim to PostgreSQL. Inline shorthands (`primary_key`, `references`, `check` on columns) are normalised into table-level lists before diffing.

```yaml
extensions:
  pgcrypto: {}

enums:
  order_status:
    schema: public
    values: [pending, confirmed, shipped, delivered]

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

  orders:
    columns:
      - name: id
        type: serial
        nullable: false
        primary_key: true
      - name: user_id
        type: integer
        nullable: false
        references: { table: users, column: id }
      - name: total
        type: numeric(10,2)
        nullable: false
        default: "0.00"
        check: "total >= 0"
    indexes:
      - name: orders_user_id_idx
        columns: [user_id]
        unique: false
        predicate: "total > 0"
    foreign_keys:
      - name: fk_orders_user
        columns: [user_id]
        to_table: users
        to_column: id
        on_delete: cascade
    constraints:
      - kind: check
        name: positive_total
        expression: "total >= 0"
    triggers:
      - name: notify_order_insert
        timing: after
        events: [insert]
        scope: row
        function_name: notify_order

views:
  recent_orders:
    definition: "SELECT id, user_id FROM orders ORDER BY id DESC LIMIT 100"

functions:
  notify_order:
    arguments: ""
    returns: trigger
    language: plpgsql
    volatility: volatile
    security_definer: false
    body: |
      BEGIN
        PERFORM pg_notify('orders', row_to_json(NEW)::text);
        RETURN NEW;
      END;
```

`SCHEMA_FILE` accepts `.yaml`, `.sql`, or a **directory** — all files inside are merged in alphabetical order.

### SQL DDL

```sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TYPE order_status AS ENUM ('pending', 'confirmed', 'shipped');

CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE UNIQUE INDEX users_email_idx ON users (email);

CREATE VIEW active_users AS SELECT * FROM users WHERE deleted_at IS NULL;
```

Supported: `CREATE TABLE`, `CREATE [UNIQUE] INDEX`, `CREATE VIEW`, `CREATE FUNCTION`, `CREATE EXTENSION`, `CREATE TYPE AS ENUM`. Any other statement is an error.

### Rust Structs

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
    #[column(check = "score >= 0")]
    rank: i32,
    #[column(skip)]
    _cache: Vec<u8>,
}
```

**`#[table(...)]` attributes**

| Attribute        | Description                                           |
| ---------------- | ----------------------------------------------------- |
| `name = "..."`   | Override table name (default: snake_case struct name) |
| `schema = "..."` | PostgreSQL schema (omit for `public`)                 |

**`#[column(...)]` attributes**

| Attribute                       | Description                                                                      |
| ------------------------------- | -------------------------------------------------------------------------------- |
| `skip`                          | Exclude this field                                                               |
| `name = "..."`                  | Override column name                                                             |
| `type = "..."`                  | Explicit SQL type — required for third-party types (`uuid`, `timestamptz`, etc.) |
| `nullable` / `nullable = false` | Override inferred nullability                                                    |
| `primary_key`                   | Mark as primary key                                                              |
| `default = "expr"`              | SQL default expression                                                           |
| `references = "table.col"`      | Inline foreign key                                                               |
| `references_name = "fk_name"`   | Explicit FK constraint name                                                      |
| `check = "expr"`                | Inline check constraint                                                          |

**Custom types** — implement `gaman::schema::ColumnType`:

```rust
use gaman::schema::{ColumnType, ColumnDesc};
use gaman::core::Dialect;

struct MyId(i64);

impl ColumnType for MyId {
    fn column_desc(_dialect: &Dialect) -> ColumnDesc {
        ColumnDesc { sql_type: "bigint", nullable: false }
    }
}
```

---

## Migration File Format

Auto-generated by `make_migration`. Human-readable YAML — hand-edit when needed.

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

Every migration runs in a single transaction (`atomic: true`). Use `atomic: false` for operations PostgreSQL cannot run transactionally — primarily `CREATE INDEX CONCURRENTLY`:

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

Setting `concurrent: true` emits `CREATE INDEX CONCURRENTLY`. Gaman validates that `atomic: false` accompanies it.

---

## Escape Hatches

Raw SQL or subprocess calls can be mixed into any migration:

```yaml
operations:
  - type: statement
    up: "UPDATE users SET role = 'member' WHERE role IS NULL"
    down: "UPDATE users SET role = NULL WHERE role = 'member'"

  - type: invoke
    up: ./scripts/backfill.py
    down: ./scripts/backfill_undo.py
```

`invoke` runs the path as a subprocess; it must exit 0.

---

## Disambiguator

The diff engine is conservative — a renamed column is indistinguishable from a drop + add. Before writing a migration, gaman flags ambiguous changes and, in interactive mode, asks for confirmation.

| Severity     | Kind            | What it catches                                                  |
| ------------ | --------------- | ---------------------------------------------------------------- |
| `Fatal`      | `NotNullAdd`    | NOT NULL column with no default — will fail on non-empty tables  |
| `Fatal`      | `NotNullChange` | Nullable → NOT NULL — existing NULLs must be backfilled first    |
| `Warning`    | `TypeCast`      | Type change — requires an explicit CAST or implicit coercion     |
| `Suggestion` | `RenameColumn`  | Drop + add of compatible types — likely a rename                 |
| `Suggestion` | `RenameTable`   | Drop + recreate of structurally similar tables — likely a rename |

For `NotNullChange`, a backfill `UPDATE` is automatically injected before the `ALTER COLUMN`.

---

## CLI Reference

Global flags (before subcommand): `-m <dir>`, `-s <file>`, `-d <url>`.

### `make_migration [name]`

Diff the schema against replayed state and write a new migration file.

| Flag        | Description                                           |
| ----------- | ----------------------------------------------------- |
| `--empty`   | Generate an empty migration with no auto-detected ops |
| `--merge`   | Create a merge migration to resolve multiple heads    |
| `--check`   | Exit non-zero if changes exist; do not write          |
| `--dry-run` | Print what would be generated without writing         |

```bash
gaman make_migration add_posts
gaman make_migration --check        # CI: fail if schema is out of sync
gaman make_migration --dry-run      # preview without writing
```

### `migrate`

Apply pending migrations in topological order. Each runs in its own transaction unless `atomic: false`.

| Flag            | Description                                            |
| --------------- | ------------------------------------------------------ |
| `--target <id>` | Migrate forward or backward to a specific migration ID |
| `--fake`        | Record as applied without executing DDL                |
| `--plan`        | List what would be applied, then exit                  |
| `--check`       | Exit non-zero if migrations are pending; do not apply  |

```bash
gaman migrate
gaman migrate --target 0003_add_posts
gaman migrate --fake 0001_initial
```

### `verify_db`

Compare the live database schema against replayed state and report drift. Tables and columns only; views and functions are excluded.

```bash
gaman verify_db
gaman verify_db --schema myschema
```

### `show_migrations`

List all migrations with `[X]` / `[ ]` applied markers.

### `sql_migrate [id]`

Print the SQL for one or all migrations. No database connection required. `--backwards` for rollback SQL.

### `inspect_db`

Introspect a live database and emit `schema.yaml`. Useful for bootstrapping an existing project.

```bash
gaman inspect_db > schema.yaml
gaman inspect_db --schema myschema --table users
```

### `config`

Print the resolved configuration and exit.

---

## Environment Variables

| Variable         | Default       | Description                                    |
| ---------------- | ------------- | ---------------------------------------------- |
| `DATABASE_URL`   | —             | PostgreSQL connection string                   |
| `MIGRATIONS_DIR` | `migrations`  | Directory containing migration YAML files      |
| `SCHEMA_FILE`    | `schema.yaml` | Path to schema (`.yaml`, `.sql`, or directory) |

All three can be overridden per-invocation with CLI flags `-d`, `-m`, `-s`.

---

## Development

```bash
cargo test

# Integration tests — require a running PostgreSQL instance
export TEST_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test postgres -- --include-ignored
```

Integration tests create and destroy isolated schemas automatically; they leave no lasting state.

---

## Status

PostgreSQL only. Core migration engine is stable and used in production. Public API may change before 1.0.

### Not yet implemented

- `squashmigrations`
- C-FFI interface

### Known limitations

- Single-column primary and foreign keys only
- Column order is not tracked
- `verify_db` does not validate view, function, extension, or enum definitions
- `alter_enum` has no inverse — migrations containing it cannot be rolled back
