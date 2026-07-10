# Online Fixtures

Online fixtures prove behavior that requires a real database. They cover live
migration application, rollback, migration tracking, lock behavior, inspection,
verification, data preservation, and expected database errors.

Use online fixtures only when the database is part of the behavior. Deterministic
rules belong in `gaman-core` unit tests or the offline fixture harness.

## Location

Online cases live under `tests/cases/online/`. Each file is a live scenario with
shared metadata and per-dialect sections.

## Fixture shape

Typical structure:

```yaml
description: representative live behavior
features:
- some.feature.id
migrations:
- id: 0001_initial
  operations: []
dialects:
  postgres:
    checks: [migrate, verify]
    expect_verify: []
  sqlite:
    checks: [error]
    expect_error: unsupported feature
```

Common top-level fields:

- `description`: globally unique human-readable behavior.
- `features`: feature ids from `tests/cases/features.yaml`.
- `migrations`: default migrations for dialect sections.
- `setup_sql`: optional SQL run before checks.
- `mutate_sql`: optional SQL run after migration and before verification.
- `dialects`: per-dialect expectations.

Dialect sections may override `migrations`, `setup_sql`, and `mutate_sql`.

## Checks

Supported online checks:

- `migrate`: apply migrations once.
- `migrate_twice`: prove migration application is idempotent.
- `migrate_to`: apply through a target migration.
- `rollback`: apply then rollback to a target.
- `migration_records`: assert recorded migration ids.
- `lock_behavior`: assert live migration locking behavior.
- `inspect`: compare high-fidelity reflected schema output.
- `verify`: run semantic drift detection against the inspected schema.
- `data`: run SQL assertions against live data.
- `error`: assert an expected live error.

Important expectation fields:

- `expect_schema`: expected reflected schema for `inspect` checks.
- `expect_verify`: expected drift operations for `verify` checks.
- `expect_error`: expected error substring for `error` checks.
- `target`: target migration id for target migrate or rollback checks.
- `expect_records`: expected migration tracking records.
- `data`: SQL data assertions.

## Inspect and verify contracts

`inspect` and `verify` intentionally test different contracts.

Inspection is onboarding-oriented. It asserts the high-fidelity schema Gaman can
reflect from a live database catalog.

Verification is drift-oriented. It compares replayed and inspected schemas using
Gaman core's dialect-specific drift registry. Only registered properties are
drift inputs. Opaque body/source fields are ignored unless they become explicit
verified properties.

## Environment

Database URL environment variables:

- `POSTGRES_DATABASE_URL`: PostgreSQL test database. Cases run in generated
  temporary schemas.
- `SQLITE_DATABASE_URL`: optional SQLite URL. When omitted, the harness uses a
  temporary file-backed database.
- `MYSQL_DATABASE_URL`: reserved for future MySQL online cases.

## Running online fixtures

```bash
cargo test -p gaman --features sqlite --test online -- --dialect sqlite
set -a; source .env; set +a; cargo test -p gaman --features sqlite --test online -- --dialect postgres
cargo test -p gaman --features sqlite --test online -- tests/cases/online/sqlite_rebuild_drop_column.yaml
```

When no explicit `--record` path is provided, the online harness writes local
support evidence to `results/online-support-results.yaml`.

Record accepted online evidence deliberately:

```bash
set -a; source .env; set +a; cargo test -p gaman --features sqlite --test online -- --record results/online-results.yaml
```

## Negative cases

Expected unsupported live behavior should list the shared
`unsupported_feature_errors` feature rather than the unsupported feature itself.
This prevents an expected SQLite function error from counting as SQLite function
support.
