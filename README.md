# Gaman

A PostgreSQL-first, offline schema migration tool written in Rust. Inspired by Django migrations — declare your schema in YAML, let `gaman` figure out what changed, and apply it.

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

### 1. Install

```bash
cargo install gaman
```

### 2. Set up environment

```bash
# .env
DATABASE_URL=postgres://localhost/myapp
MIGRATIONS_DIR=migrations
SCHEMA_FILE=schema.yaml
```

### 3. Define your schema

```yaml
# schema.yaml
tables:
  users:
    name: users
    columns:
      - name: id
        type: serial
        nullable: false
        primary_key: true
      - name: email
        type: text
        nullable: false
      - name: created_at
        type: timestamptz
        nullable: false
        default: now()
    indexes:
      - name: users_email_idx
        columns: [email]
        unique: true

  posts:
    name: posts
    columns:
      - name: id
        type: serial
        nullable: false
        primary_key: true
      - name: user_id
        type: integer
        nullable: false
        references:
          table: users
          column: id
      - name: title
        type: text
        nullable: false
      - name: body
        type: text
        nullable: true
    indexes:
      - name: posts_user_id_idx
        columns: [user_id]
        unique: false
```

### 4. Generate and apply migrations

```bash
# Create your initial migration
gaman make_migration initial

# Review what SQL will run
gaman sql_migrate

# Apply to the database
gaman migrate
```

---

## CLI Reference

All commands accept `-m <dir>` to override the migrations directory and `-s <file>` to override the schema file.

### `make_migration [name]`

Generate a new migration by diffing `schema.yaml` against the replayed state of all existing migrations. Writes a new YAML file into the migrations directory.

| Flag        | Description                                                                       |
| ----------- | --------------------------------------------------------------------------------- |
| `--empty`   | Create an empty migration (no auto-detected operations)                           |
| `--merge`   | Create a merge migration to resolve multiple heads                                |
| `--check`   | Exit with a non-zero code if there are pending schema changes; do not write files |
| `--dry-run` | Print what would be generated without writing files                               |

```bash
gaman make_migration add_posts
gaman make_migration --check          # CI gate: fail if schema is out of sync
gaman make_migration --empty hotfix   # empty shell for a hand-written migration
```

### `migrate`

Apply pending migrations to the database in topological order. Each migration runs in its own transaction — a failure rolls back only that migration.

| Flag                   | Description                                                   |
| ---------------------- | ------------------------------------------------------------- |
| `--database-url <url>` | Override `DATABASE_URL`                                       |
| `--target <id>`        | Migrate forward or backward to a specific migration ID        |
| `--fake`               | Record migrations as applied without executing DDL            |
| `--plan`               | List which migrations would be applied, then exit             |
| `--check`              | Exit non-zero if there are unapplied migrations; do not apply |

```bash
gaman migrate
gaman migrate --target 0003_add_posts   # forward or backward to a specific point
gaman migrate --fake 0001_initial       # adopt an existing DB without re-running DDL
gaman migrate --check                   # CI gate: fail if migrations are pending
```

### `verify_db`

Compare the live database against the replayed migration state and report any structural drift. Exits non-zero if drift is found. Views and functions are excluded — their SQL representation in `pg_catalog` rarely round-trips cleanly against the YAML definition.

| Flag                   | Description                                          |
| ---------------------- | ---------------------------------------------------- |
| `--database-url <url>` | Override `DATABASE_URL`                              |
| `--schema <name>`      | Schema to verify (default: `public`)                 |

```bash
gaman verify_db
gaman verify_db --schema myschema
```

### `show_migrations`

List all known migrations with their applied status.

```
[X] 0001_initial
[X] 0002_add_email
[ ] 0003_add_posts
```

| Flag                   | Description             |
| ---------------------- | ----------------------- |
| `--database-url <url>` | Override `DATABASE_URL` |

### `sql_migrate [id]`

Print the SQL statements for one or all migrations. No database connection required.

| Flag          | Description                               |
| ------------- | ----------------------------------------- |
| `--backwards` | Print rollback SQL instead of forward SQL |

```bash
gaman sql_migrate                        # all migrations, forward
gaman sql_migrate 0003_add_posts         # single migration
gaman sql_migrate 0003_add_posts --backwards
```

### `inspect_db`

Introspect a live PostgreSQL database and emit a schema state as YAML. Useful for adopting an existing database or auditing drift.

| Flag                   | Description                                          |
| ---------------------- | ---------------------------------------------------- |
| `--database-url <url>` | Override `DATABASE_URL`                              |
| `--schema <name>`      | Schema to introspect (repeatable; default: `public`) |
| `--table <name>`       | Restrict output to a single table                    |
| `--output <file>`      | Write to a file instead of stdout                    |

```bash
gaman inspect_db > schema.yaml
gaman inspect_db --schema myschema --output schema.yaml
```

### `config`

Print the resolved configuration and exit. Useful for debugging env var and flag resolution.

---

## Schema YAML Format

### Column

```yaml
- name: price
  type: numeric(10,2)
  nullable: false
  default: "0.00"
  primary_key: false # shorthand — gaman emits a constraint, not a column modifier
  references: # inline FK sugar — normalized into foreign_keys by gaman
    table: products
    column: id
    name: fk_order_items_product # optional constraint name
  check: "price >= 0" # inline check sugar
```

**Supported shorthand:** `primary_key: true` on a column and inline `references` / `check` are normalized away by `schema.normalize()` before diffing. You never have to write the expanded form by hand.

**Column types are passed directly to the database.** Gaman does not map, validate, or normalise type names — whatever you write in `type:` is emitted verbatim in the generated SQL. This means types are database-specific (`serial`, `timestamptz`, `jsonb`, `uuid`, etc. are all PostgreSQL types). Cross-database type portability is explicitly not a goal.

### Foreign Key (expanded form)

```yaml
foreign_keys:
  - name: fk_posts_user
    columns: [user_id]
    to_table: users
    to_column: id
    on_delete: cascade # restrict | set_null | set_default | no_action | cascade
```

### Index

```yaml
indexes:
  - name: users_email_idx
    columns: [email]
    unique: true
  - name: orders_status_created_idx
    columns: [status, created_at]
    unique: false
    predicate: "status != 'archived'" # partial index
```

### Constraint

```yaml
constraints:
  - kind: unique
    name: users_email_unique
    columns: [email]
  - kind: check
    name: positive_price
    expression: "price > 0"
```

### View

```yaml
views:
  active_users:
    name: active_users
    definition: "SELECT id, email FROM users WHERE active = true"
```

### Function

```yaml
functions:
  notify_on_insert:
    name: notify_on_insert
    arguments: ""
    returns: trigger
    language: plpgsql
    body: |
      BEGIN
        PERFORM pg_notify('inserts', row_to_json(NEW)::text);
        RETURN NEW;
      END;
    volatility: volatile # volatile (default) | stable | immutable
    security_definer: false
```

### Trigger

```yaml
# Triggers live inside the table definition.
triggers:
  - name: notify_users_insert
    timing: after # before | after | instead_of
    events: [insert] # insert | update | delete | truncate
    scope: row # row | statement
    function_name: notify_on_insert

  # Inline sugar — body + language are converted to a synthetic function by normalize()
  - name: audit_update
    timing: after
    events: [update]
    scope: row
    language: plpgsql
    body: |
      BEGIN
        INSERT INTO audit_log(table_name, changed_at) VALUES (TG_TABLE_NAME, now());
        RETURN NEW;
      END;
```

---

## Migration File Format

Generated migrations are YAML files in the migrations directory. They are human-readable and can be hand-edited.

```yaml
# migrations/0003_add_posts.yaml
id: 0003_add_posts
dependencies:
  - 0002_add_email
operations:
  - type: create_table
    table:
      name: posts
      columns:
        - name: id
          type: serial
          nullable: false
        - name: user_id
          type: integer
          nullable: false
        - name: title
          type: text
          nullable: false
      foreign_keys:
        - name: fk_posts_user
          columns: [user_id]
          to_table: users
          to_column: id
      indexes: []
      constraints: []
```

### Arbitrary SQL and subprocess invocations

Two escape hatches are available for operations gaman can't express as structured operations:

```yaml
operations:
  - type: statement
    up: "UPDATE users SET role = 'member' WHERE role IS NULL"
    down: "UPDATE users SET role = NULL WHERE role = 'member'"

  - type: invoke
    up: ./scripts/backfill.py
    down: ./scripts/backfill_undo.py
```

`invoke` runs the given command as a subprocess. The command must exit 0 to succeed.

---

## Migration Graph

Migrations form a **directed acyclic graph**. Each migration's `dependencies` list points to its parent(s). Gaman applies migrations in topological order, guaranteeing that all dependencies are applied before their dependents.

**Linear history** (most projects):

```
0001_initial → 0002_add_email → 0003_add_posts
```

**Branched history** (parallel feature work):

```
0001_initial → 0002_feature_a ─┐
             → 0003_feature_b ─┴→ 0004_merge
```

If multiple heads exist without a merge migration, `make_migration` reports a conflict and requires `--merge` to resolve it.

---

## How Replay Works

`make_migration` never connects to a database. The previous schema state is reconstructed by replaying operations from all migrations in topological order:

```
[] → apply(0001) → apply(0002) → apply(0003) → PreviousState
```

The diff is then `CurrentState (schema.yaml) − PreviousState`. This makes migration generation deterministic, reproducible, and CI-friendly with no external dependencies.

---

## Environment Variables

| Variable         | Default       | Description                           |
| ---------------- | ------------- | ------------------------------------- |
| `DATABASE_URL`   | —             | PostgreSQL connection string          |
| `MIGRATIONS_DIR` | `migrations`  | Directory containing migration files  |
| `SCHEMA_FILE`    | `schema.yaml` | Path to the desired schema definition |

All three can be overridden per-command via CLI flags.

---

## Development

```bash
# Run all unit tests
cargo test

# Run integration tests (requires a running PostgreSQL instance)
export TEST_DATABASE_URL=postgres://localhost/gaman_test
cargo test --test postgres -- --include-ignored
```

Integration tests create and destroy isolated schemas (`gaman_test_N`) automatically — they do not touch your main database.

---

## Status

Early development. The core engine is stable and well-tested. PostgreSQL is the only supported database. The public API may change before 1.0.

Implemented:

- Schema state model (tables, columns, indexes, FKs, constraints, views, functions, triggers)
- Offline diff engine with deterministic operation ordering
- Migration DAG with topological replay, conflict detection, and merge migrations
- `make_migration`, `migrate`, `show_migrations`, `sql_migrate`, `inspect_db`, `config` commands
- Per-migration transactions with partial-failure isolation
- `--fake`, `--check`, `--dry-run`, `--plan` flags
- Live database introspection (`inspect_db`) with replay verification
- Schema drift detection (`verify_db`)

Not yet implemented:

- Rename detection (currently emits `drop + create`)
- Migration locking (`pg_try_advisory_lock`)
- `squashmigrations`
