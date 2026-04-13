# Gaman

> **Not production ready — still in active development. APIs and file formats may change.**

A deterministic, offline-first migration engine for PostgreSQL, written in Rust. Declare your schema in YAML, and let gaman compute and apply the diff - no database access required at plan time. Inspired by Django migrations.

Pronounced _guh-MUN_ (गमन, /ɡəˈmən/) — Sanskrit for "movement" or "going forward".

---

## Mental Model

```
schema.yaml (current) ──┐
                         ├─► diff ──► new migration file
migrations/ (replayed) ──┘
```

- `schema.yaml` is the **desired** schema state.
- The **previous** state is reconstructed by replaying all migrations in topological order — no database access required.
- The **diff** between the two states produces an ordered list of operations, emitted as a new migration file.
- `migrate` applies pending migrations to the database and records them in `gaman_migrations`.

Migrations are stored as a **directed acyclic graph (DAG)**. Each migration declares its `dependencies`, enabling parallel feature branches and explicit merge migrations when branches need to be unified.

---

## Quick Start

```bash
cargo install gaman
```

Set env vars (or use CLI flags `-d`, `-m`, `-s`):

```bash
DATABASE_URL=postgres://localhost/myapp
MIGRATIONS_DIR=migrations
SCHEMA_FILE=schema.yaml
```

Declare your schema in `schema.yaml`, then:

```bash
gaman make_migration initial   # generate first migration
gaman sql_migrate               # preview SQL
gaman migrate                   # apply
```

---

## CLI Reference

Global flags (before subcommand): `-m <dir>`, `-s <file>`, `-d <url>`.

### `make_migration [name]`

Diff `schema.yaml` against replayed state and write a new migration file.

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

YAML files, human-readable and hand-editable.

### `atomic`

By default every migration runs inside a single transaction (`atomic: true`). Set `atomic: false` when the migration contains operations that PostgreSQL cannot run inside a transaction — most notably `CREATE INDEX CONCURRENTLY`.

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

### Concurrent indexes

`concurrent: true` on `add_index` / `drop_index` emits `CREATE INDEX CONCURRENTLY` / `DROP INDEX CONCURRENTLY`. The migration **must** have `atomic: false`; gaman validates this and rejects the plan otherwise.

### Extensions

```yaml
- type: create_extension
  extension:
    name: pgcrypto

- type: create_extension
  extension:
    name: postgis
    schema: public
    version: "3.4"

- type: drop_extension
  extension:
    name: pgcrypto
```

### Enum types

```yaml
- type: create_enum
  enum_def:
    name: order_status
    schema: public
    values: [pending, confirmed, shipped, delivered]

- type: alter_enum # append-only: adds new values in-place
  old:
    name: order_status
    values: [pending, confirmed, shipped, delivered]
  new:
    name: order_status
    values: [pending, confirmed, shipped, delivered, cancelled]

- type: drop_enum
  enum_def:
    name: order_status
```

`alter_enum` is subject to PostgreSQL's append-only rule: existing values cannot be removed or reordered. If the diff detects a removal or reordering it emits `drop_enum` + `create_enum` instead. `alter_enum` has no inverse — it cannot appear in a migration that is rolled back.

---

## Escape hatches

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

Runs as a post-diff pass inside `make_migration`. Accepts prior `decisions`; returns more questions or the final op list:

```
ops = diff(current, previous)
decisions = []
loop:
    process(ops, decisions) → NeedsInput(clars) → prompt → decisions.extend(answers)
                            → Resolved(final_ops) → write migration
```

| Severity     | Kind            | What it catches                                                |
| ------------ | --------------- | -------------------------------------------------------------- |
| `Fatal`      | `NotNullAdd`    | NOT NULL column with no default — fails on non-empty tables    |
| `Fatal`      | `NotNullChange` | Nullable → NOT NULL — requires backfilling existing NULLs      |
| `Warning`    | `TypeCast`      | Type change — requires explicit CAST or implicit coercion      |
| `Suggestion` | `RenameColumn`  | Drop + add of compatible types — likely a rename               |
| `Suggestion` | `RenameTable`   | Drop + create of structurally similar tables — likely a rename |

For `NotNullChange`, a backfill `UPDATE` is auto-injected before the `ALTER COLUMN`.

Transport-agnostic — `CliPromptEngine` uses stdin/stdout; `PromptEngine` is a trait any caller can implement.

---

## Migration Graph & Replay

Migrations form a DAG. Each declares `dependencies`, enabling parallel branches and explicit merges.

```
0001_initial → 0002_feature_a ─┐
             → 0003_feature_b ─┴→ 0004_merge
```

`make_migration` never touches the database — it replays existing migrations to reconstruct previous state, then diffs:

```
[] → apply(0001) → apply(0002) → apply(0003) → PreviousState
CurrentState − PreviousState → new migration
```

Multiple heads without a merge migration → conflict error, requires `--merge`.

---

## Environment Variables

| Variable         | Default       | Description                          |
| ---------------- | ------------- | ------------------------------------ |
| `DATABASE_URL`   | —             | PostgreSQL connection string         |
| `MIGRATIONS_DIR` | `migrations`  | Directory containing migration files |
| `SCHEMA_FILE`    | `schema.yaml` | Path to the schema definition        |

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

## Status

Early-stage. Core migration engine is stable and tested in real use. PostgreSQL only. Public API may change before 1.0.

### Not yet implemented

- `squashmigrations`
- Embedded Rust library API and C-FFI interface
- `ALTER EXTENSION … UPDATE` (use a `statement` operation for now)

### Known limitations

- Single-column primary and foreign keys only
- Column order is not tracked
- `verify_db` does not validate view, function, extension, or enum definitions
- `alter_enum` has no inverse — migrations containing it cannot be rolled back
