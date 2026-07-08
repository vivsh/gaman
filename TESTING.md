# Testing

Gaman is tested through Rust unit tests and YAML fixture harnesses. The fixture
files are the primary product evidence: they show which migration features work,
which dialects they work for, and which unsupported features fail clearly.

Most migration planning is tested entirely offline without requiring a running
database. Live databases are only required for behavior that cannot be modeled
deterministically.

Testing hierarchy:

1. Rust unit tests verify implementation details and deterministic invariants.
2. Parser YAML fixtures verify SQL DDL lowering into Gaman schema entities.
3. Offline YAML fixtures verify deterministic migration behavior.
4. Online YAML fixtures verify real database behavior.
5. Accepted evidence generates the public support matrix.

## Policy

- All new migration behavior must first be covered by YAML fixtures.
- Add parser YAML fixtures for `CREATE`-only SQL DDL lowering into `Schema`.
- Add offline YAML fixtures for deterministic behavior: normalization, replay,
  diffing, clarification, rollback planning, and SQL rendering.
- Add online YAML fixtures for behavior that needs a real database: migration
  application, catalog introspection, `verify_db`, constraints, locks,
  transactions, data preservation, and dialect quirks.
- When a YAML fixture exposes a failure, reproduce the failure as the smallest
  Rust unit test around the responsible module, then fix the implementation.
- Use property-based tests only for deterministic `gaman-core` invariants. They
  complement YAML fixtures; they do not create support evidence.
- Keep message copy out of migration behavior tests. Clarifier fixtures
  assert structured `Clarification` and `Decision` values, not prompt wording.
- Do not hand-edit support claims. Accepted result files and generated README
  tables are the source of truth.

## Test Layers

- `gaman-core`: offline schema IR, normalization, canonicalization, validation,
  replay, diff, clarification, SQL segmentation/parsing, and dialect SQL planning.
- `tests/parser.rs`: YAML fixtures for parser support by dialect and entity
  kind. These track SQL statements that lower into Gaman schema structs.
- `tests/offline.rs`: YAML fixtures for offline parser smoke coverage, replay,
  diff, clarification, rollback, SQL rendering, and end-to-end planning.
- `tests/online.rs`: YAML fixtures for live PostgreSQL and SQLite migration,
  inspect, verify, data, and expected-error checks.
- `tests/offline_coverage.rs`: validates offline and online evidence matrices
  and checks that the README support table is current.
- `tests/sqlite_dialect.rs`: focused SQLite renderer and rebuild unit tests.
- `tests/yaml_adapter.rs`: filesystem-backed migration write behavior.
- Rust unit tests: precise module-level regressions, lifecycle ordering, exact
  error variants, and helpers that should not require full YAML scenarios.
- `proptest` tests in `gaman-core`: generated checks for deterministic
  normalization, replay, diff, SQL planning, rollback, and identifier handling.

## Property-Based Tests

Property tests live beside the modules they exercise, not in a separate harness.
They use the standard `proptest` crate and are intentionally limited to
deterministic offline code in `gaman-core`.

Current property coverage includes:

- normalization idempotency;
- prepared schemas keeping deterministic child names;
- column primary-key shorthand normalization;
- replay determinism and independent create-table ordering;
- empty diff and deterministic diff output;
- diff followed by replay reconstructing generated target schemas;
- PostgreSQL SQL planning/rendering stability;
- raw statement preservation;
- rollback round-trips for reversible generated migrations;
- non-reversible rollback failure before SQL output;
- PostgreSQL identifier quoting and schema-qualified rendering;
- SQLite rebuild-only operations requiring planner context when the `sqlite`
  feature is enabled.

Property-test rules:

- keep generators small, valid by construction, and private to test modules
  unless reuse is clear;
- do not generate random SQL for live databases;
- do not test CLI, filesystem, prompting, or online database behavior through
  property tests;
- keep case counts modest enough for normal `cargo test -p gaman-core`;
- checked-in `proptest-regressions/` files are allowed only for real shrunk
  failures, not as a generated corpus.

Run property tests through the normal core commands:

```bash
cargo test -p gaman-core
cargo test -p gaman-core --features sqlite
```

## Current Coverage Snapshot

Measured with `scripts/coverage.sh` on 2026-07-07:

- workspace line coverage: 78.68% (`10274/13058`);
- `gaman-core` line coverage: 87.31% (`8389/9608`);
- root `gaman` crate line coverage: 54.64% (`1885/3450`);
- HTML report: `results/coverage/html/index.html`;
- LCOV report: `results/coverage/lcov.info`.

The default coverage run includes SQLite but does not require live PostgreSQL.
PostgreSQL online evidence is recorded separately through the online harness.

Coverage policy:

- line coverage is the primary metric;
- function and region coverage are secondary signals;
- coverage must not drop below the current baseline without an explicit note;
- WASM checks are compile-boundary checks, not coverage inputs.

## Common Commands

```bash
cargo test -p gaman-core
cargo test -p gaman --test parser
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
```

Offline/WASM boundaries:

```bash
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
```

Local non-live gate:

```bash
scripts/check.sh
```

Coverage:

```bash
scripts/coverage.sh
```

## Results Directory

All generated evidence, reports, and benchmark outputs belong under `results/`.

Checked-in accepted evidence:

- `results/offline-results.yaml`
- `results/online-results.yaml`
- `results/parser-results.yaml`

Local/ad-hoc outputs should use ignored paths such as:

- `results/offline-support-results.yaml`
- `results/online-support-results.yaml`
- `results/coverage/`

## Fixture Selection

The offline and online harnesses are custom binaries. Pass case files,
directories, or quoted globs after `--`.

```bash
cargo test --test offline
cargo test --test offline -- tests/cases/offline/parser/postgres
cargo test --test offline -- tests/cases/offline/diff/add_nullable_email.yaml
cargo test --test offline -- 'tests/cases/offline/parser/postgres/*.yaml'

cargo test --test parser
cargo test --test parser -- tests/cases/parser/postgres
cargo test --test parser -- tests/cases/parser/sqlite/sqlite_trigger_body.yaml
cargo test --test parser -- 'tests/cases/parser/postgres/*.yaml'

cargo test --features sqlite --test online -- --dialect sqlite
cargo test --features sqlite --test online -- tests/cases/online/sqlite_rebuild_drop_column.yaml
```

Selection rules:

- no args: run all YAML files under the harness root;
- file arg: run exactly that file;
- directory arg: recursively run YAML files under that directory;
- quoted glob: expanded by the harness inside the harness root;
- missing, non-YAML, metadata-only, and outside-root paths fail early.

## Parser Fixtures

Parser cases live under `tests/cases/parser/` and are grouped by SQL dialect:

- `postgres/`
- `sqlite/`

The parser harness is independent from offline migration planning. It records
which `CREATE` SQL DDL statements successfully lower into Gaman `Schema`
entities through the public parser API:

```rust
gaman::parsers::parse_sql_for_dialect(sql, dialect)
```

Parser fixtures assert two levels of evidence:

- `expect_entities`: compact `EntityKind` coverage such as table, column, index,
  trigger, function, enum, or extension.
- `expect_schema`: exact normalized Gaman `Schema` expected after parsing.

Unsupported parser cases use `expect_error` instead of entity/schema assertions.
`ALTER`, `DROP`, `DELETE`, and other non-`CREATE` statements must be error
fixtures because the parser is a schema loader, not a migration parser.

Statement segmentation happens before AST parsing and is tested in `gaman-core`.
It is dialect-aware and tag-free: semicolon boundaries, final statements without
trailing semicolons, PostgreSQL dollar-quoted bodies, SQLite trigger bodies, and
MySQL delimiter-driven routines are covered by unit tests. MySQL segmentation is
available through parser utilities, but MySQL schema lowering remains explicitly
unsupported.

Example:

```yaml
description: PostgreSQL CREATE TYPE enum lowers into EnumDef
dialect: postgres
sql: |
  CREATE TYPE mood AS ENUM ('happy', 'sad');

expect_entities:
- kind: enum
  name: mood

expect_schema:
  enums:
    mood:
      name: mood
      values: [happy, sad]
```

Record accepted parser evidence:

```bash
cargo test --test parser -- --record results/parser-results.yaml
```

Current parser support:

| Entity | PostgreSQL | SQLite |
|---|---:|---:|
| table | supported | supported |
| column | supported | supported |
| constraint | supported | supported |
| foreign key | supported | supported |
| index | supported | supported |
| trigger | function-backed | body-backed |
| function | supported | unsupported |
| view | supported | supported |
| enum | supported | unsupported |
| extension | supported | unsupported |

## Offline Fixtures

Offline cases live under `tests/cases/offline/` and are grouped by intent:

- `diff/`
- `clarifier/`
- `end_to_end/`
- `parser/postgres/`
- `parser/sqlite/`
- `parser/mysql/`
- `replay/`
- `rollback/`
- `sql/postgres/`
- `sql/sqlite/`
- `validation/`

Folders are for navigation and selection only. Evidence is driven by fixture
metadata: `description`, `group`, `features`, `kind`, and optional `dialect`.

Every offline fixture must have:

- a globally unique `description`;
- a non-empty `group`;
- one or more feature ids from `tests/cases/offline-features.yaml`;
- one clear behavior under test unless the kind is explicitly `end_to_end`.

Current offline fixture count: 132.

Current offline case distribution:

- diff: 25
- clarifier: 25
- end-to-end: 1
- parser: 33
- replay: 8
- rollback: 4
- SQL rendering: 27
- validation: 9

Offline fixture kinds:

- `sql_parse`: parser capability and Gaman lowering classification.
- `sql_to_schema`: SQL DDL lowered into `Schema`.
- `schema_to_migration`: desired schema to generated operations and optional SQL.
- `migration_to_replay`: migrations to deterministic replayed `Schema`.
- `migration_to_sql`: migrations to offline SQL, forward or rollback.
- `end_to_end`: desired schema to generated migration, replayed schema, and SQL.

Record accepted offline evidence:

```bash
cargo test --features sqlite --test offline -- --record results/offline-results.yaml
```

The `offline_coverage` test validates that:

- every fixture references known feature ids;
- every accepted result points to a real fixture;
- every accepted feature has successful evidence;
- modeled operations, clarification kinds, answers, renderer classifications,
  rollback behavior, parser coverage, and unsupported behavior are represented.

Print the offline evidence table:

```bash
cargo run --bin gaman-support-matrix -- --offline
```

## Online Fixtures

Online cases live under `tests/cases/online/`. Each file is a live scenario with
a unique `description`, shared `features`, shared migrations by default, and
optional per-dialect overrides.

Current online fixture count: 67.

The current online suite covers:

- shared PostgreSQL/SQLite migration application;
- empty migrations and linear chains;
- idempotent migrate, target migrate, rollback, migration tracking, and lock
  cleanup;
- table and column create/add/drop/rename/alter flows;
- type/default/nullability changes;
- generated columns;
- single, composite, self-referencing, and cyclic FK behavior where supported;
- indexes, unique constraints, and check constraints;
- ownership-scoped `verify_db` for no-drift, missing-owned, changed-owned, and
  live-only ignored cases;
- composite primary-key and foreign-key verify drift;
- generated-column, view, trigger, enum, index, constraint, and FK verify
  behavior where each dialect supports stable introspection;
- data preservation checks;
- PostgreSQL enums, views, functions, function-name triggers, query triggers,
  and partial indexes;
- SQLite table rebuilds and unsupported-feature errors.

Database URL environment variables:

- `POSTGRES_DATABASE_URL`: PostgreSQL test database. Cases run in generated
  temporary schemas.
- `SQLITE_DATABASE_URL`: optional SQLite URL. When omitted, the harness uses a
  temporary file-backed database.
- `MYSQL_DATABASE_URL`: reserved for future MySQL online cases.

Run live cases:

```bash
cargo test --features sqlite --test online -- --dialect sqlite
set -a; source .env; set +a; cargo test --features sqlite --test online -- --dialect postgres
```

Record accepted online evidence and update the README matrix:

```bash
set -a; source .env; set +a; cargo test --features sqlite --test online -- --record results/online-results.yaml
cargo run --bin gaman-support-matrix -- --update-readme
cargo test --test offline_coverage
```

Online result statuses:

- `success`: at least one accepted online case passed for the feature/dialect;
- `failure`: at least one accepted online case failed for the feature/dialect;
- `unimplemented`: no accepted evidence exists, the dialect section is missing,
  the dialect was unavailable, or the dialect is not implemented.

Negative online cases should list `unsupported_feature_errors`, not the
unsupported feature itself. This prevents an expected SQLite function error from
counting as SQLite function support.

## README Support Matrix

`tests/cases/support-matrix.yaml` defines the README feature rows. It references
accepted online evidence from `results/online-results.yaml`, offline evidence
from `results/offline-results.yaml`, and explicit design notes for unsupported
or bounded rows. The README support matrix is generated from those files and
wrapped in checked markers.

`cargo test --test offline_coverage` fails when:

- README support rows drift from generated output;
- a supported or partial evidence cell has no successful evidence;
- a partial or unsupported cell has no design note;
- accepted evidence points to a missing case;
- a case references an unknown product feature.

Policy:

- a green live README cell must have online evidence;
- a green offline README cell may use offline evidence, because the feature is
  offline by design;
- `◐` is allowed only for deliberately bounded support with evidence and a note;
- `❌` is allowed only for unsupported-by-design or database-unsupported rows;
- planned/unimplemented cells must stay non-green until evidence is recorded.

## Adding New Tests

1. Add a YAML fixture first.
2. Run the narrow fixture or group.
3. If it fails because the fixture is wrong, fix the fixture.
4. If it fails because Gaman is wrong, add the smallest Rust unit test that
   reproduces the underlying module failure.
5. Fix the implementation.
6. Rerun the YAML fixture.
7. Refresh accepted evidence if the fixture changes support evidence.
8. Regenerate README if online evidence changes.

Use YAML fixtures for:

- migration behavior;
- dialect support claims;
- parser capability;
- expected unsupported-feature errors;
- live inspect/verify/data behavior.

Use Rust unit tests for:

- exact helper behavior;
- precise error variants;
- mock executor lifecycle ordering;
- regression tests derived from failing YAML scenarios;
- property tests for deterministic invariants where many generated shapes are
  more useful than one golden case.

## Release-Oriented Checks

Before release, run:

```bash
cargo test -p gaman-core
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo test --test offline_coverage
set -a; source .env; set +a; cargo test --features sqlite --test online -- --record results/online-results.yaml
cargo run --bin gaman-support-matrix -- --update-readme
cargo fmt
git diff --check
cargo package --allow-dirty
```
