# Testing

Gaman is tested through Rust unit tests, property tests, and YAML fixture
harnesses. The YAML fixtures are the main product evidence: they describe
observable behavior, supported dialect behavior, and expected unsupported cases.

Most migration planning is tested entirely offline without requiring a running
database. Live databases are only required for behavior that cannot be modeled
deterministically.

This file is a contributor guide. Detailed fixture schemas live in:

- [Parser fixtures](docs/parser-fixtures.md)
- [Offline fixtures](docs/offline-fixtures.md)
- [Online fixtures](docs/online-fixtures.md)
- [Evidence and support matrix](docs/evidence.md)
- [Detailed support evidence](docs/support-evidence.md)
- [Release checks](RELEASE.md)

## Overview

### Testing philosophy

Tests should prove Gaman behavior at the boundary where users observe it.

Testing hierarchy:

1. Rust unit tests verify implementation details.
2. Offline YAML fixtures verify deterministic migration behavior.
3. Online YAML fixtures verify real database behavior.
4. Accepted evidence generates the public support matrix.

YAML fixtures are used because they describe externally observable behavior, and their accepted results can be reused as evidence for user-facing documentation such as the README support matrix.

For migration behavior, that boundary is usually a YAML fixture rather than a narrow unit test. Unit tests still matter, but they protect implementation invariants, exact error behavior, helper logic, and regressions discovered from fixtures.

The fixture harnesses are deliberately split by lifecycle stage:

1. Parser fixtures prove SQL `CREATE` statements lower into Gaman schema models.
2. Offline fixtures prove deterministic behavior without a live database.
3. Online fixtures prove behavior that requires a real database.
4. Accepted result files record support evidence.
5. Generated support tables consume accepted evidence rather than hand-written
   claims.

### Testing hierarchy

- `gaman-core` unit tests cover schema IR, normalization, canonicalization,
  validation, replay, lexical diff, semantic drift, clarification, SQL
  segmentation/parsing, and dialect SQL planning.
- `tests/parser.rs` runs parser YAML fixtures under `tests/cases/parser/`.
- `tests/offline.rs` runs deterministic YAML fixtures under
  `tests/cases/offline/`.
- `tests/online.rs` runs live database YAML fixtures under
  `tests/cases/online/`.
- `tests/offline_coverage.rs` validates accepted evidence and the generated
  README support matrix.
- Focused integration tests such as `tests/sqlite_dialect.rs` and
  `tests/yaml_adapter.rs` cover smaller crate-level behavior.
- Property tests live beside the `gaman-core` modules they exercise.

### Offline vs online testing

Offline tests should cover everything that can be modeled deterministically:
parsing, normalization, replay, diff, drift comparators, SQL rendering,
clarification, rollback planning, and expected unsupported behavior.

Online tests should be reserved for behavior that needs a real database:
migration application, transactions, locks, catalog inspection, `inspect`,
`verify`, constraints, data preservation, and dialect-specific database
quirks.

Prefer the cheapest layer that proves the behavior. Do not add an online case
when the same rule can be proven deterministically in `gaman-core` or the
offline harness.

`gaman check_schema` is a focused live validation command, not a migration
test. It prepares each SQL schema statement without executing it, so it is
covered by small executor and CLI tests rather than the online fixture matrix.

## Testing Policy

### YAML-first policy

All new migration behavior should start with a YAML fixture. The fixture should
state the behavior in user-visible terms and should be narrow enough that a
failure identifies the lifecycle stage.

Use parser fixtures for SQL DDL loading behavior. Use offline fixtures for
schema planning, replay, rendering, rollback, and semantic drift. Use online
fixtures only when live database behavior is part of the claim.

Unsupported behavior should also be covered by fixtures. A clear unsupported
error is a supported product behavior.

### When to write unit tests

Write Rust unit tests when the behavior is smaller than a product fixture or
when a fixture exposes a bug in a specific helper. Good unit-test targets are:

- exact error variants;
- normalization and canonicalization helpers;
- parser segmenter edge cases;
- drift comparator callbacks;
- replay ordering and dependency invariants;
- SQL rendering fragments;
- regression cases derived from a failing fixture.

Keep message copy out of migration behavior tests. Clarifier tests should assert
structured `Clarification` and `Decision` values rather than prompt wording.

### Property-test policy

Property tests are for deterministic `gaman-core` invariants. They complement
fixtures but do not create support evidence.

Use property tests for generated checks such as idempotency, deterministic
ordering, replay round-trips, reversible rollback behavior, and identifier
rendering. Keep generators small, valid by construction, and private to test
modules unless reuse is clearly useful.

Do not use property tests for CLI behavior, filesystem behavior, prompting, live
databases, or random SQL execution.

Run property tests through the normal core commands:

```bash
cargo test -p gaman-core
cargo test -p gaman-core --features sqlite
```

### Dialect type catalogs

Dialect catalogs improve alias normalization, suggestions, and TOFU prompts;
they never reject a type merely because it is absent. PostgreSQL catalog
references live in `tests/catalogs/postgres-native-types.yaml`; SQLite affinity
examples live in `tests/catalogs/sqlite-affinity.yaml`. Core unit tests keep
both artifacts aligned with the implementation.

To audit a pristine PostgreSQL server for new built-in catalog candidates, run:

```bash
POSTGRES_DATABASE_URL=postgres://... scripts/audit-postgres-types.sh
```

The audit is advisory. Review its output before classifying a candidate as a
column type, a function-only pseudo-type, or an internal catalog type.

### Evidence policy

Do not hand-edit support claims. Accepted evidence files, generated README
support tables, and the generated detailed support evidence page are the source
of truth.

Checked-in accepted evidence lives in:

- `results/parser-results.yaml`
- `results/offline-results.yaml`
- `results/online-results.yaml`

Local or ad-hoc evidence should use ignored output paths, such as
`results/online-support-results.yaml` or files under `results/coverage/`.

See [Evidence and support matrix](docs/evidence.md) for accepted evidence,
recording commands, README matrix generation, and detailed support evidence
generation.

## Test Layout

### Core unit and property tests

Core tests live inside `gaman-core/src/` beside the modules they exercise. They
cover deterministic schema lifecycle behavior and dialect-independent rules.

### Parser harness

`tests/parser.rs` runs YAML fixtures in `tests/cases/parser/`. These fixtures
track SQL `CREATE` statements that successfully lower into Gaman-owned schema
entities through `gaman::parsers::parse_sql(sql, dialect)`.

Detailed fixture format: [Parser fixtures](docs/parser-fixtures.md).

### Offline harness

`tests/offline.rs` runs YAML fixtures in `tests/cases/offline/`. These fixtures
cover deterministic lifecycle behavior: parser smoke tests, schema loading,
lexical diff, semantic drift, replay, SQL rendering, rollback, clarification,
validation, and end-to-end planning.

Detailed fixture format: [Offline fixtures](docs/offline-fixtures.md).

### Online harness

`tests/online.rs` runs YAML fixtures in `tests/cases/online/`. These fixtures
cover live PostgreSQL and SQLite behavior, including apply, rollback,
tracking, locks, inspect, verify, data checks, and expected live errors.

Detailed fixture format: [Online fixtures](docs/online-fixtures.md).

### Coverage and support checks

`tests/offline_coverage.rs` validates accepted evidence and generated README
support rows. Coverage reports are generated by `scripts/coverage.sh` and should
stay under `results/coverage/`.

## Running Tests

### Common commands

```bash
cargo test -p gaman-core
cargo test -p gaman --test parser
cargo test -p gaman --test offline
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
```

Offline/WASM compile boundary:

```bash
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
```

Local non-live gate:

```bash
scripts/check.sh
```

### Schema SQL prepare checks

`check_schema` uses `DATABASE_URL` to prepare every segmented `.sql` statement
without executing it. It does not read migrations, install tracking state,
acquire locks, or start transactions. YAML and JSON schema inputs are reported
as ignored.

```bash
DATABASE_URL=sqlite::memory: gaman --schema schema.sql check_schema
DATABASE_URL=postgres://localhost/myapp gaman --schema schema check_schema
```

Prepare validation catches database syntax and prepare-time semantic failures;
it does not prove effects that require executing statements in order.

Coverage:

```bash
scripts/coverage.sh
```

### Fixture selection

The parser, offline, and online harnesses accept case files, directories, or
quoted globs after `--`.

```bash
cargo test --test parser
cargo test --test parser -- tests/cases/parser/postgres
cargo test --test parser -- tests/cases/parser/sqlite/sqlite_trigger_body.yaml
cargo test --test parser -- 'tests/cases/parser/postgres/*.yaml'

cargo test --test offline
cargo test --test offline -- tests/cases/offline/diff/add_nullable_email.yaml
cargo test --test offline -- tests/cases/offline/verify
cargo test --test offline -- 'tests/cases/offline/parser/postgres/*.yaml'

cargo test --features sqlite --test online -- --dialect sqlite
cargo test --features sqlite --test online -- tests/cases/online/sqlite_rebuild_drop_column.yaml
```

Selection rules:

- no args runs all YAML files under the harness root;
- a file arg runs exactly that file;
- a directory arg recursively runs YAML files under that directory;
- a quoted glob is expanded by the harness inside the harness root;
- missing, non-YAML, metadata-only, and outside-root paths fail early.

### Live database environment

PostgreSQL online tests require `POSTGRES_DATABASE_URL`. Cases run in generated
temporary schemas. SQLite online tests can use `SQLITE_DATABASE_URL`; when it is
omitted, the harness uses a temporary file-backed database. `MYSQL_DATABASE_URL`
is reserved for future MySQL online cases.

Typical live runs:

```bash
cargo test --features sqlite --test online -- --dialect sqlite
set -a; source .env; set +a; cargo test --features sqlite --test online -- --dialect postgres
```

## Adding Tests

### Contributor workflow

1. Add the narrowest YAML fixture that describes the behavior.
2. Run that fixture or fixture directory.
3. If the fixture is wrong, fix the fixture.
4. If Gaman is wrong, add the smallest Rust unit test around the responsible
   module.
5. Fix the implementation.
6. Rerun the narrow fixture.
7. Refresh accepted evidence when the fixture changes support evidence.
8. Regenerate README support tables when accepted online/offline evidence
   changes support claims.

### Decision tree

Use a parser fixture when the question is: can this SQL `CREATE` statement be
segmented, parsed, and lowered into Gaman schema structs?

Use an offline fixture when the question is deterministic: what operations,
clarifications, replayed schema, SQL, rollback SQL, or drift findings should
Gaman produce?

Use an online fixture when the question depends on live database behavior:
applying migrations, inspecting catalogs, verifying drift against a database,
locking, transactions, constraints, or data preservation.

Use a Rust unit test when the behavior is a small internal invariant, a precise
error branch, a parser segmenter edge case, a comparator rule, or a regression
extracted from a broader fixture.

Use a property test when many generated deterministic shapes are more valuable
than a single golden case.

## Results

The `results/` directory stores generated evidence, reports, and local outputs.
Checked-in evidence files document accepted product behavior; ignored files are
for local runs, coverage reports, and ad-hoc support checks.

Accepted evidence:

- `results/parser-results.yaml`: accepted parser fixture results.
- `results/offline-results.yaml`: accepted deterministic fixture results.
- `results/online-results.yaml`: accepted live fixture results.

Local outputs:

- `results/online-support-results.yaml`: default online harness output when no
  explicit accepted evidence path is provided.
- `results/coverage/`: local coverage reports.

Accepted evidence should be refreshed deliberately. If a test run writes a
checked-in result file, review the diff as part of the behavior change.
