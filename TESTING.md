# Testing

This repo uses normal Rust tests plus YAML fixture harnesses for end-to-end
cases.

## Layout

- `gaman-core`: offline schema, replay, diff, SQL planning, dialect, parser, and
  disambiguator tests.
- `tests/offline.rs`: offline YAML harness for parsing, replay, diff, and SQL
  rendering.
- `tests/postgres.rs`: live PostgreSQL YAML harness, gated by `postgres`.
- `tests/sqlite.rs`: live SQLite YAML harness, gated by `sqlite`.
- `tests/sqlite_dialect.rs`: focused SQLite renderer and table-rebuild tests.
- `tests/derive_into_table.rs`, `tests/embedded_migrations.rs`,
  `tests/yaml_adapter.rs`: public integration surfaces.

## Common Commands

```bash
cargo test -p gaman-core
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
```

WASM/offline boundaries:

```bash
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline-sqlite --target wasm32-unknown-unknown
```

Final hygiene:

```bash
cargo fmt
git diff --check
```

## Fixture Selection

The `offline`, `postgres`, and `sqlite` harnesses are custom binaries. Pass case
files or directories after `--`.

```bash
cargo test --test offline
cargo test --test offline -- tests/cases/offline/sql_to_schema_basic.yaml
cargo test --test offline -- tests/cases/offline/parser/postgres
cargo test -p gaman --features sqlite --test sqlite -- tests/cases/sqlite/rebuild_drop_column.yaml
```

Selection rules:

- no args: run all `*.yaml` files under the harness root;
- file arg: run that file;
- directory arg: recursively run YAML files under it;
- missing, non-YAML, outside-root, and flag-style args fail.

## Offline Fixtures

Offline cases live under `tests/cases/offline`.

Kinds:

- `sql_parse`: sqlparser parse classification, separate from Gaman lowering.
- `sql_to_schema`: PostgreSQL SQL to `Schema`.
- `schema_to_migration`: desired schema to generated operations.
- `migration_to_replay`: migrations to replayed `Schema`.
- `migration_to_sql`: migrations to offline SQL.

Parser fixtures use:

```yaml
kind: sql_parse
parser_dialect: postgres # postgres | sqlite | mysql
sql: |
  CREATE POLICY active_users ON users USING (active);
expect_parse: ok         # ok | error
expect_lowering: unsupported # ok | unsupported | error
expect_error: unsupported SQL statement
```

Use parser fixtures to document parser capability even for dialects Gaman does
not yet support.

## Live PostgreSQL Fixtures

PostgreSQL cases live under `tests/cases/postgres` and need `TEST_DATABASE_URL`.

```bash
TEST_DATABASE_URL=postgres://localhost/gaman_test cargo test --test postgres
TEST_DATABASE_URL=postgres://localhost/gaman_test cargo test --test postgres -- tests/cases/postgres/inspect_setup_sql.yaml
```

Without `TEST_DATABASE_URL`, the full harness skips. Explicit selected cases
without `TEST_DATABASE_URL` fail.

Kinds:

- `migrate`
- `inspect`
- `verify`

Each case runs in an isolated generated schema and cleans it up afterward.

## Live SQLite Fixtures

SQLite cases live under `tests/cases/sqlite` and require `--features sqlite`.
Each case uses a temporary file-backed SQLite database.

Kinds:

- `migrate`
- `inspect`
- `verify`

Use these for rebuild behavior, live introspection, verify drift, constraint
enforcement, rollback, and index preservation.

## Expectations

- Use `expect_schema`, `expect_operations`, or `expect_sql` for success cases.
- Use `expect_error` for expected failures; it is matched as a substring.
- Fixture schemas and operations use Gaman's normal YAML shapes.
- Prefer small fixtures that prove one behavior.
- Use Rust tests instead of fixtures for mock executors, lifecycle ordering, or
  precise internal helper behavior.
