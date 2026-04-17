# Gaman

> **Not production ready — still in active development. APIs and file formats may change.**

A deterministic, offline-first migration engine for PostgreSQL, written in Rust. Declare your schema — as YAML, SQL DDL, or Rust structs — and gaman computes the diff and generates migrations with no database connection required at plan time.

Pronounced _guh-MUN_ (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward".

Gaman works in two modes:

- **CLI** — declare your schema in `schema.yaml` or `schema.sql`, run `gaman make_migration`, and apply with `gaman migrate`. No code changes required.
- **Embedded** — add `gaman` as a Rust dependency, declare your schema as structs with `#[derive(IntoTable)]`, and run migrations at application startup.

---

## Mental Model

```
schema.yaml / schema.sql (current) ──┐
                                      ├─► diff ──► new migration file
migrations/ (replayed) ───────────────┘
```

- `schema.yaml` (or `schema.sql`) is the **desired** schema state.
- The **previous** state is reconstructed by replaying all migrations in topological order — no database access required.
- The **diff** between the two states produces an ordered list of operations, emitted as a new migration file.
- `migrate` applies pending migrations to the database and records them in `gaman_migrations`.

Migrations are stored as a **directed acyclic graph (DAG)**. Each migration declares its `dependencies`, enabling parallel feature branches and explicit merge migrations when branches need to be unified.

```
0001_initial → 0002_feature_a ─┐
             → 0003_feature_b ─┴→ 0004_merge
```

---

## Usage

### CLI

```bash
cargo install gaman
```

Set environment variables (or use CLI flags `-d`, `-m`, `-s`):

```bash
DATABASE_URL=postgres://localhost/myapp
MIGRATIONS_DIR=migrations
SCHEMA_FILE=schema.yaml   # or schema.sql
```

Declare your schema in `schema.yaml` (or `schema.sql`), then:

```bash
gaman make_migration initial   # generate first migration
gaman sql_migrate               # preview SQL
gaman migrate                   # apply
```

See [Schema YAML Format](#schema-yaml-format) for the full schema syntax and [CLI Reference](#cli-reference) for all available commands.

### Embedded in Rust

Add to `Cargo.toml`:

```toml
[dependencies]
gaman = "0.3"
```

**Apply migrations at startup — no CLI needed**

```rust
use gaman::{Config, MigrationEngine, include_migrations};

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), MIGRATIONS)
        .migrate()
        .expect("migrations failed");
    // start server, run jobs, etc.
}
```

**Expose the full CLI from your own binary**

Struct-based schema — no YAML file needed:

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

YAML or SQL-based schema — load from a file at startup:

```rust
use gaman::{Config, MigrationEngine, include_migrations};
use gaman::schema::Schema;

static MIGRATIONS: &[(&str, &str)] = include_migrations!("migrations");

fn main() {
    MigrationEngine::new(Config::default(), MIGRATIONS)
        // accepts .yaml, .sql, or a directory containing both
        .with_schema(|_| Schema::load(std::path::Path::new("schema.sql"))
            .expect("failed to load schema"))
        .handle_args()
        .expect("command failed");
}
```

`handle_args()` parses `std::env::args()` and dispatches the full command set: `make_migration`, `migrate`, `verify_db`, `show_migrations`, etc. CLI flags (`-d`, `-m`, `-s`) override the `Config` passed to `new()`.

`include_migrations!("path")` embeds all `.yaml` files from the given path at compile time, sorted lexicographically. No files on disk required at runtime.

**`#[derive(IntoTable)]` — full attribute reference**

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

| Attribute        | Description                                           |
| ---------------- | ----------------------------------------------------- |
| `name = "..."`   | Override table name (default: snake_case struct name) |
| `schema = "..."` | Postgres schema (omit for `public`)                   |

**`#[column(...)]` attributes**

| Attribute                       | Description                                                                      |
| ------------------------------- | -------------------------------------------------------------------------------- |
| `skip`                          | Exclude this field from the table definition                                     |
| `name = "..."`                  | Override column name                                                             |
| `type = "..."`                  | Explicit SQL type — required for third-party types (`uuid`, `timestamptz`, etc.) |
| `nullable` / `nullable = false` | Override inferred nullability                                                    |
| `primary_key`                   | Mark as primary key                                                              |
| `default = "expr"`              | SQL default expression                                                           |
| `references = "table.col"`      | Inline foreign key                                                               |
| `references_name = "fk_name"`   | Explicit FK constraint name                                                      |
| `check = "expr"`                | Inline check constraint                                                          |

For your own custom types, implement `gaman::schema::ColumnType`:

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

## CLI Reference

Global flags (before subcommand): `-m <dir>`, `-s <file>`, `-d <url>`.

### `make_migration [name]`

Diff `schema.yaml` (or `schema.sql`) against replayed state and write a new migration file.

| Flag        | Description                                        |
| ----------- | -------------------------------------------------- |
| `--empty`   | Empty migration, no auto-detected ops              |
| `--merge`   | Create a merge migration to resolve multiple heads |
| `--check`   | Exit non-zero if changes exist; do not write       |
| `--dry-run` | Print what would be generated without writing      |

```bash
gaman make_migration add_posts
gaman make_migration --check
gaman make_migration --empty hotfix
```

### `migrate`

Apply pending migrations in topological order. Each runs in its own transaction.

| Flag            | Description                                           |
| --------------- | ----------------------------------------------------- |
| `--target <id>` | Migrate forward or backward to a specific ID          |
| `--fake`        | Record as applied without executing DDL               |
| `--plan`        | List what would be applied, then exit                 |
| `--check`       | Exit non-zero if migrations are pending; do not apply |

```bash
gaman migrate
gaman migrate --target 0003_add_posts
gaman migrate --fake 0001_initial
```

### `verify_db`

Compare the live database against replayed state and report drift. Views and functions are excluded.

```bash
gaman verify_db
gaman verify_db --schema myschema
```

### `show_migrations`

List all migrations with `[X]` / `[ ]` applied markers.

### `sql_migrate [id]`

Print SQL for one or all migrations. No database connection required. `--backwards` for rollback SQL.

### `inspect_db`

Introspect a live database and emit `schema.yaml`. Flags: `--schema`, `--table`, `--output`.

```bash
gaman inspect_db > schema.yaml
```

### `config`

Print resolved configuration and exit.

---

## Schema YAML Format

Column types are passed verbatim to PostgreSQL. Shorthand fields (`primary_key`, inline `references`, inline `check`) are normalised before diffing.

Extensions and enum types sit at the top level alongside `tables`:

```yaml
extensions:
  pgcrypto:
    name: pgcrypto

enums:
  order_status:
    name: order_status
    schema: public
    values: [pending, confirmed, shipped, delivered]

tables:
  orders:
    name: orders
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
views:
  recent_orders:
    name: recent_orders
    definition: "SELECT id, user_id FROM orders ORDER BY id DESC LIMIT 100"
functions:
  notify_order:
    name: notify_order
    arguments: ""
    returns: trigger
    language: plpgsql
    body: |
      BEGIN
        PERFORM pg_notify('orders', row_to_json(NEW)::text);
        RETURN NEW;
      END;
    volatility: volatile
    security_definer: false
```

Triggers live inside the table definition:

```yaml
triggers:
  - name: notify_order_insert
    timing: after
    events: [insert]
    scope: row
    function_name: notify_order
```

---

## Migration File Format

YAML files, human-readable and hand-editable. Operations are generated automatically by `make_migration` — you rarely need to write them by hand.

### `atomic`

Every migration runs inside a single transaction by default (`atomic: true`). Set `atomic: false` for operations PostgreSQL cannot run inside a transaction — most notably `CREATE INDEX CONCURRENTLY`.

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

Set `concurrent: true` on `add_index` / `drop_index` to emit `CREATE INDEX CONCURRENTLY` / `DROP INDEX CONCURRENTLY`. The migration **must** have `atomic: false`; gaman validates this.

---

## Escape Hatches

```yaml
operations:
  - type: statement
    up: "UPDATE users SET role = 'member' WHERE role IS NULL"
    down: "UPDATE users SET role = NULL WHERE role = 'member'"

  - type: invoke
    up: ./scripts/backfill.py
    down: ./scripts/backfill_undo.py
```

`invoke` runs a subprocess; must exit 0.

---

## Disambiguator

The diff engine is conservative: a renamed column looks identical to a drop + add. The disambiguator catches these cases and asks before committing.

| Severity     | Kind            | What it catches                                                |
| ------------ | --------------- | -------------------------------------------------------------- |
| `Fatal`      | `NotNullAdd`    | NOT NULL column with no default — fails on non-empty tables    |
| `Fatal`      | `NotNullChange` | Nullable → NOT NULL — requires backfilling existing NULLs      |
| `Warning`    | `TypeCast`      | Type change — requires explicit CAST or implicit coercion      |
| `Suggestion` | `RenameColumn`  | Drop + add of compatible types — likely a rename               |
| `Suggestion` | `RenameTable`   | Drop + create of structurally similar tables — likely a rename |

For `NotNullChange`, a backfill `UPDATE` is auto-injected before the `ALTER COLUMN`.

---

## Environment Variables

| Variable         | Default       | Description                                        |
| ---------------- | ------------- | -------------------------------------------------- |
| `DATABASE_URL`   | —             | PostgreSQL connection string                       |
| `MIGRATIONS_DIR` | `migrations`  | Directory containing migration files               |
| `SCHEMA_FILE`    | `schema.yaml` | Path to schema file (`.yaml`, `.sql`) or directory |

---

## Development

```bash
cargo test

# Integration tests (requires PostgreSQL)
export TEST_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test postgres -- --include-ignored
```

Integration tests create and destroy isolated schemas automatically.

---

## SQL DDL as Schema Source

In addition to YAML and Rust structs, gaman can parse a `schema.sql` file containing `CREATE` statements and feed it through the same diff pipeline:

```sql
-- schema.sql
CREATE EXTENSION IF NOT EXISTS "uuid-ossp";

CREATE TYPE order_status AS ENUM ('pending', 'confirmed', 'shipped');

CREATE TABLE users (
    id bigserial PRIMARY KEY,
    email text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE INDEX users_email_idx ON users (email);

CREATE VIEW active_users AS SELECT * FROM users WHERE deleted_at IS NULL;
```

Supported statements: `CREATE TABLE`, `CREATE INDEX`, `CREATE UNIQUE INDEX`, `CREATE VIEW`, `CREATE FUNCTION`, `CREATE EXTENSION`, `CREATE TYPE AS ENUM`. Everything else is skipped.

`SCHEMA_FILE` accepts `.yaml`, `.sql`, or a directory. When a directory is given, all `.yaml` and `.sql` files inside are merged in alphabetical order.

---

## Status

Early-stage. Core migration engine is stable and tested in real use. PostgreSQL only. Public API may change before 1.0.

### Not yet implemented

- `squashmigrations`
- C-FFI interface

### Known limitations

- Single-column primary and foreign keys only
- Column order is not tracked
- `verify_db` does not validate view, function, extension, or enum definitions
- `alter_enum` has no inverse — migrations containing it cannot be rolled back

---
