# Offline Fixtures

Offline fixtures prove deterministic Gaman behavior without a live database.
They are the primary test layer for schema loading, migration planning, replay,
SQL rendering, rollback planning, clarification, validation, and semantic drift
comparator behavior.

## Location

Offline cases live under `tests/cases/offline/` and are grouped by intent:

- `clarifier/`
- `diff/`
- `end_to_end/`
- `parser/postgres/`
- `parser/sqlite/`
- `parser/mysql/`
- `replay/`
- `rollback/`
- `sql/postgres/`
- `sql/sqlite/`
- `validation/`
- `verify/`

Folders are for navigation and harness selection. Evidence is driven by fixture
metadata, not by folder names.

## Common metadata

Every offline fixture has:

- `description`: globally unique human-readable behavior.
- `group`: fixture grouping used for evidence and navigation.
- `features`: one or more feature ids from `tests/cases/offline-features.yaml`.
- `dialect`: optional fixture dialect, defaulting to PostgreSQL.
- `kind`: fixture schema discriminator.

Each fixture should test one clear behavior unless the kind is explicitly
`end_to_end`.

## Fixture kinds

### `parser`

Parser smoke fixtures cover segmenting, parsing, and lowering behavior inside
the offline harness, including unsupported parser cases.

Important fields:

- `parser_dialect`: SQL parser dialect.
- `sql`: SQL input.
- `expect_parse`: `ok` or `error`.
- `expect_lowering`: `ok`, `unsupported`, or `error`.
- `expect_schema`: expected schema when lowering succeeds.
- `expect_error`: expected error substring for failures.

### `sql_to_schema`

Loads SQL DDL into a Gaman schema for the fixture dialect.

Important fields:

- `sql`: SQL input.
- `expect_schema`: expected loaded schema.
- `expect_error`: expected error substring.

### `schema_to_migration`

Diffs a desired schema against replayed migrations and checks generated
operations, clarification output, SQL, or expected errors.

Important fields:

- `name`: migration name.
- `migrations`: existing migration graph.
- `current`: desired schema.
- `decisions`: optional clarification decisions.
- `expect_no_changes`: no-op expectation.
- `expect_clarifications`: completed clarification output.
- `expect_pending_clarifications`: pending clarification output.
- `expect_operations`: generated operations.
- `expect_sql`: generated SQL.
- `expect_error`: expected error substring.

### `sql_schema_to_migration`

Loads the desired schema through the public SQL ingestion path, then runs the
same migration planning lifecycle as `schema_to_migration`. Use this kind for
opaque SQL entities and unmanaged table options, which structured authored
schemas cannot represent.

Important fields:

- `sql`: desired SQL schema input.
- `name`: migration name.
- `migrations`: existing migration graph.
- `decisions`: optional clarification decisions.
- `expect_no_changes`: no-op expectation.
- `expect_clarifications`: completed clarification output.
- `expect_pending_clarifications`: pending clarification output.
- `expect_operations`: generated operations.
- `expect_schema`: replayed schema after the generated migration.
- `expect_sql`: generated SQL.
- `expect_error`: expected loading or planning error.

### `migration_to_replay`

Replays migrations into schema state.

Important fields:

- `migrations`: migration graph.
- `expect_schema`: expected replayed schema.
- `expect_error`: expected replay error.

### `migration_to_sql`

Renders migrations to SQL in the forward or rollback direction.

Important fields:

- `direction`: `forward` or `rollback`.
- `ids`: optional selected migration ids.
- `migrations`: migration graph.
- `expect_sql`: expected SQL.
- `expect_error`: expected render error.

### `verify`

Runs semantic drift comparison without a live database. This directly models the
same conceptual inputs used by `verify`:

- `replayed`: expected schema state reconstructed from tracked migrations.
- `inspected`: observed schema state reflected from a live database.

Important fields:

- `schema`: optional database schema/scope, defaulting to `public`.
- `replayed`: replayed schema.
- `inspected`: inspected schema.
- `expect_findings`: exact property-level drift findings.
- `expect_operations`: repair operations projected from findings.
- `expect_report`: formatted report lines using `expected` and `observed`.
- `expect_error`: expected normalization or drift error.

Example:

```yaml
description: PostgreSQL inspected literal cast default semantically matches replayed schema

group: verify
features:
- verify.postgres_default_cast_no_drift
dialect: postgres
kind: verify
schema: public
replayed:
  tables:
    preferences:
      columns:
      - name: theme
        type: text
        nullable: false
        default: '''light'''
inspected:
  tables:
    preferences:
      columns:
      - name: theme
        type: text
        nullable: false
        default: '''light''::text'
expect_findings: []
expect_operations: []
expect_report: []
```

### `end_to_end`

Runs desired schema through migration generation, replay, and SQL rendering.
Use this sparingly for lifecycle confidence; prefer narrower fixture kinds for
specific behavior.

Important fields:

- `name`: migration name.
- `migrations`: existing migration graph.
- `current`: desired schema.
- `decisions`: optional clarification decisions.
- `expect_operations`: generated operations.
- `expect_schema`: replayed schema after generated migration.
- `expect_sql`: generated SQL.
- `expect_error`: expected error substring.

## Running offline fixtures

```bash
cargo test -p gaman --test offline
cargo test -p gaman --test offline -- tests/cases/offline/diff/add_nullable_email.yaml
cargo test -p gaman --test offline -- tests/cases/offline/verify
cargo test -p gaman --test offline -- 'tests/cases/offline/parser/postgres/*.yaml'
```

Record accepted offline evidence deliberately:

```bash
cargo test -p gaman --features sqlite --test offline -- --record results/offline-results.yaml
```
