# Gaman Architecture

Gaman is an offline-first migration engine. Its core job is to turn schema input
from several frontends into deterministic migration operations and dialect SQL
without needing a live database at generation time.

The long-term database families are the SQLx relational backends:

- PostgreSQL.
- SQLite.
- MySQL / MariaDB.

Current per-database support belongs in the README support matrix. This document
defines the architecture and invariants; it should not drift into a stale status
table.

Implementation must not deviate from this architecture. If the architecture is
wrong or incomplete, update it deliberately before changing code away from it.

## Conceptual Model

All frontends produce the same internal `Schema` representation:

- YAML and JSON schema files.
- SQL DDL parsing.
- Rust builders and `IntoTable` derive output.
- Live database introspection.
- Replayed migration history.

The internal schema is the comparison boundary. Frontends build IR; Gaman then
prepares that IR for a selected dialect before diffing, SQL planning, live
verification, or migration execution.

At the highest level:

```text
schema input --> Schema IR --> prepare --> diff --> operations --> SQL
                    ^                         ^                    |
                    |                         |                    v
              frontends only          migration replay         executor
```

Migration replay is deterministic, offline, and side-effect free. It consumes
migration files and produces an in-memory `Schema`; it never connects to a
database or mutates external state.

Migration generation depends only on:

- the desired schema input;
- the committed migration files;
- the selected dialect;
- explicit disambiguation decisions.

It must not depend on live database state.

## Schema Representation

Gaman defines a strict schema superset:

- Tables.
- Columns:
  - name;
  - type;
  - nullability;
  - default;
  - generated expression.
- Indexes.
- Constraints:
  - primary keys, including composite primary keys;
  - foreign keys, including composite foreign keys;
  - unique constraints;
  - check constraints.
- Extensions.
- Triggers.
- Functions.
- Views.

No feature outside this superset is planned as first-class model metadata. Use
`Statement` for database-specific SQL outside the modeled superset.

Primary keys are table-level schema metadata. Column-level `primary_key: true`
is accepted as frontend shorthand, including on multiple columns, but
normalization must produce deterministic `Table.primary_key` metadata with a
constraint name and ordered column list. Explicit table-level primary-key input
preserves its name and order.

Foreign keys are table-level schema metadata with ordered source and target
column lists. Column-level `references` is single-column shorthand only.
Normalization turns shorthand into explicit table-level `ForeignKey` metadata.
Composite foreign keys must use table-level metadata so names, source column
order, target table, and target column order survive replay, diffing, rendering,
introspection, and verification.

Frontend object names may be omitted when Gaman can derive a deterministic name.
After normalization, canonical schema state is always fully named. Public
derived names use PostgreSQL-style names, not a `gmn_` prefix:

- primary key: `{table}_pkey`;
- foreign key: `{table}_{source_columns_joined}_fkey`;
- index: `{table}_{columns_joined}_idx`;
- unique constraint: `{table}_{columns_joined}_key`;
- column check constraint: `{table}_{column}_check`;
- unnamed table check constraint: `{table}_check`.

Derived names use the bare table name and preserve column order. Collisions fail
validation rather than being silently suffixed. Internal runtime-only names may
use `__gaman_*`; those are not user schema object names.

Opaque schema objects means extensions, triggers, functions, and views. Gaman
tracks opaque schema objects as whole objects. It may create, replace, or delete
them, but it should not generate fine-grained internal `ALTER` statements for
their bodies or definitions.

If a database does not have a modeled object type, that type is absent for that
dialect. If a migration asks that dialect to execute an absent or unimplemented
feature, Gaman must raise a clear error instead of emitting no-op SQL.

## Design Principles

- Offline generation, replay, diffing, disambiguation, and SQL rendering are
  deterministic.
- Offline features must not require SQLx, Tokio, live database drivers,
  filesystem access, environment variables, terminal I/O, TLS, or executors.
- The offline layer should compile for `wasm32-unknown-unknown`; browser callers
  provide schemas and migrations as strings or in-memory values.
- `sql_migrate` is offline and renders the operation SQL live migration
  application would execute for the same migration and replay state.
- Migration application, `inspect_db`, and the live side of `verify_db` require
  a database connection.
- Migration files are engine-specific. Gaman should support native behavior for
  each dialect instead of forcing a lowest-common-denominator file format.
- Unsupported and unimplemented features fail as early as possible.
- Shared code owns graph ordering, replay, diff orchestration, disambiguation,
  validation orchestration, and lifecycle.
- Dialect-specific behavior stays inside dialect modules; executor-specific
  behavior stays inside executor modules.

## Non-Goals

- Full coverage of every DDL feature in any database.
- Schema metadata outside the strict superset.
- Portable migration files across engines.
- Structural modeling of arbitrary SQL inside `Statement`.
- First-class data migration operations.
- External process invocation during migration application.
- Lossy introspection that silently drops unsupported modeled metadata.
- Runtime migration discovery for embedded use. Embedded migrations should be
  compiled into the binary.

## Schema Preparation

Schema preparation runs after frontend input becomes Gaman IR. It has three
separate responsibilities.

Normalization is database-agnostic frontend sugar cleanup:

- column `references` shorthand becomes table-level foreign-key metadata;
- column `check` shorthand becomes table-level check constraints;
- column primary-key flags become explicit table-level primary-key metadata;
- trigger names are derived when omitted;
- missing table names can be filled from schema map keys.

Canonicalization is dialect-specific cleanup:

- built-in type aliases such as PostgreSQL `int4` become canonical names;
- SQLite type aliases map to Gaman's chosen affinity names where appropriate;
- schema qualification rules are normalized for the selected dialect;
- live introspection quirks are brought into the same comparison form.

Validation checks structural correctness:

- duplicate objects;
- unknown referenced tables and columns;
- invalid primary-key, foreign-key, index, and constraint metadata;
- unsupported dialect features;
- invalid dependency graph state before planning or execution.

Preparation is intentionally not the same thing as proving every column type is
known. A type can be absent from the dialect catalog and still be valid project
schema after replay trust or explicit user approval.

## Dialect Type Catalogs

Each dialect keeps type knowledge in frequently editable files under its dialect
module:

- `data_types.rs` lists native built-in types, aliases, canonical names,
  affinity rules, and typo suggestions.
- `extension_types.rs` lists popular extension or externally provided types.

These catalogs are intentionally incomplete. They are used for deterministic
canonicalization, typo suggestions, and helpful prompts. They are not a claim
that Gaman knows the full database type universe. Unknown types must not be
rejected solely because they are absent from these files.

Unknown data-type handling uses trust on first use (TOFU) and is replay-aware:

1. Replay committed migrations into the previous schema.
2. Prepare the previous schema and desired schema for the selected dialect.
3. Collect trusted project-local types from the replayed previous schema.
4. Accept known built-in types, aliases, known extension types, and modeled enum
   types.
5. Accept unknown types already present in replayed history.
6. Ask only when the desired schema introduces a new unknown type.
7. Apply the user's decision before diffing:
   - use a known canonical type; or
   - keep the authored type exactly.

A committed migration containing a custom, domain, extension, or user-defined
type is the approval record. Gaman does not maintain a separate custom-type
registry.

## Migration Graph

Migrations form a DAG. Each migration declares dependencies. The graph is
validated when the migrator is constructed, and the migrator caches topological
order for later planning and execution.

The DAG makes ancestry explicit. Parallel histories may coexist, but ordinary
generation refuses to build on multiple heads. Independent histories must be
joined through an explicit merge migration.

Embedded multi-crate migration trees are namespaced at compile time. Child
migration IDs and dependencies are rewritten into stable namespaces so crates can
compose migration histories without runtime discovery or ID collisions.

## Generation Pipeline

`make_migration` follows this order:

```text
load graph
  |
  v
reject multiple heads
  |
  v
replay committed migrations ----------------+
  |                                         |
  v                                         v
prepare previous schema              prepare desired schema
  |                                         |
  +------------ collect trusted types ------+
                    |
                    v
        resolve newly introduced unknown types
                    |
                    v
        prepare resolved desired schema again
                    |
                    v
              diff previous -> desired
                    |
                    v
        disambiguate operation-level risks
                    |
                    v
         dialect reorder / dependency calc
                    |
                    v
              write migration file
```

There are two disambiguation layers:

- Type disambiguation runs before diffing because it changes the desired schema
  that all frontends share.
- Operation disambiguation runs after diffing because it resolves ambiguous or
  risky operations, such as renames, type casts, and not-null backfills.

Interactive CLI generation may ask for these decisions. Non-interactive
generation must not read stdin or choose defaults: it fails with the pending
clarification IDs/messages. `make_migration --check` follows the same no-prompt
rule.

Generated migration files must be self-contained. They can contain modeled
operations and literal `Statement` SQL, but not sidecar approvals, external data
file references, or subprocess invocations.

## SQL Rendering

`sql_migrate` is the canonical offline SQL plan for migration operations. It
renders the same operation SQL that live migration application will execute, but
without opening a database connection.

`sql_migrate` intentionally excludes lifecycle SQL:

- tracking-table installation;
- locks;
- transaction boundaries;
- record and unrecord statements.

Rendering uses replayed schema state when an operation needs context. SQLite
table rebuilds are the primary example: the renderer must know the table shape
before and after a migration. Therefore `Dialect::operation_to_sql()` remains a
single-operation convenience API, while context-dependent operations must render
through `Migrator` or `OfflinePlanner`.

If live migration would fail for an unsupported operation, offline SQL rendering
must fail as well. Partially supported operations should not degrade into empty
SQL.

## Live Migration Lifecycle

Live migration application keeps runtime concerns outside offline planning:

```text
validate plan
   |
   v
install tracking table
   |
   v
acquire lock
   |
   v
apply or roll back selected migrations
   |
   v
record / unrecord migration IDs
   |
   v
release lock on success or failure
```

Each migration is applied independently. For `atomic: true`, SQL execution and
recording happen inside one transaction. The migration ID is recorded only after
operation SQL succeeds. SQL failure or record failure rolls back that migration
transaction. Rollback builds inverse operations in reverse order and fails before
execution if any selected operation is not reversible.

## Inspect And Verify

`inspect_db` requires a live database. It turns live catalog metadata into
`Schema` IR, then prepares that schema for the selected dialect before returning
or writing it.

`verify_db` compares live introspection against replayed migration state.
Verification is strongest for the relational core:

- tables;
- columns;
- indexes;
- primary keys;
- foreign keys;
- unique constraints;
- check constraints where the dialect can introspect them deterministically.

For opaque schema objects, verification should compare deterministic metadata,
not body text:

- functions: schema, name, arguments/signature, return type, language, and other
  stable metadata where available;
- triggers: table, name, timing, events, scope, referenced function, or direct
  query metadata where available;
- extensions: name, schema, and version where available;
- views: schema, name, and stable definition metadata where available.

Opaque source text is preserved exactly as authored or first exported by
`inspect_db`. Gaman does not rewrite source while storing, replaying, rendering,
or writing migrations. Offline diff first compares source exactly; only on
mismatch does it use conservative lexical canonicalization to suppress
formatting-only churn outside quoted and protected regions.

Live database catalogs may normalize, rewrite, or omit source text, so
`verify_db` does not claim deep body-equivalence for functions, triggers, or
views unless a future dialect-specific mode can recover deterministic source.

Trigger query source is stored as `query`, not as a function body. PostgreSQL
renders query triggers by wrapping the query in a generated trigger function and
supplying the normal return statement (`RETURN NEW` for row triggers,
`RETURN NULL` for statement triggers). SQLite renders trigger queries directly.
Non-default PostgreSQL trigger return behavior requires an explicit modeled
function and `function_name`.

## Dialect Boundary

Dialect modules own:

- SQL rendering;
- dialect-specific validation;
- type canonicalization;
- native and extension type catalogs;
- operation reordering;
- context-aware rendering such as SQLite table rebuilds;
- tracking-table SQL;
- unsupported-feature errors.

Executor modules own:

- database connections;
- statement execution;
- transaction commands;
- lock acquisition and release;
- live introspection.

Shared code may ask a dialect to validate or render a migration from a replayed
schema state. Shared code must not branch on PostgreSQL-specific, SQLite-
specific, or future MySQL-specific syntax.

## Offline Core And WASM Goal

`gaman-core` physically owns the offline implementation:

- schema IR;
- operations and migrations;
- graph ordering;
- replay;
- diffing;
- disambiguation data structures and resolution;
- dialect canonicalization and SQL rendering;
- string-based schema and migration parsing;
- `OfflinePlanner`.

The root `gaman` crate is the compatibility facade. Default features expose the
native CLI and database layer. `--no-default-features --features offline`
exposes offline APIs without compiling database drivers. `offline-sqlite`
enables SQLite rendering without linking the live SQLite executor.

Native-only concerns remain outside the offline core:

- SQLx executors;
- live `inspect_db`;
- live `verify_db`;
- locks and tracking installation;
- filesystem-backed migration sources and writers;
- CLI parsing, dotenv loading, and terminal prompting.

Offline acceptance targets:

```bash
cargo check -p gaman-core --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline-sqlite --target wasm32-unknown-unknown
```

Offline builds must not compile SQLx, Tokio, argh, dotenvy, native TLS, or
executor modules.

## Escape Hatches

`Statement` is the only migration-file escape hatch. It embeds literal SQL in a
migration and participates in ordering, transaction handling, SQL rendering, and
rollback when a `down` statement exists.

`Statement` does not mutate the replayed `Schema`. If it changes modeled schema
objects, the authored schema and later migrations must account for that state
explicitly.

Gaman should not define first-class data migration operations. Frontends may read
CSV, JSON, application metadata, or domain inputs and compile them into explicit
`Statement` operations before handing migrations to Gaman. Migration files
themselves must remain self-contained.

External process invocation is outside the migration contract. Invoker traits,
remote execution, subprocess execution, and related entities are not part of the
operation model or native execution layer.

## Roadmap Boundaries

Planned work should improve correctness, coverage, and robustness inside the
strict superset rather than add new schema object families.

High-priority work:

- Improve live introspection and `verify_db` coverage for supported relational
  metadata.
- Harden opaque schema object signatures and stable metadata comparison.
- Expand SQLite live introspection for generated-column expressions, check
  constraints, views, triggers, and other metadata SQLite does not expose in the
  same structured way as tables, columns, foreign keys, and indexes.
- Add more round-trip tests:
  - schema to migration to replay;
  - migration to SQL golden output;
  - live inspect to verify no drift for supported objects.
- Expand benchmarks for large diffs, long histories, replay, SQL rendering, and
  SQLite rebuild planning.

Lower-priority work:

- MySQL / MariaDB dialect and executor support.
- Deeper dialect-specific body verification for opaque schema objects where
  deterministic catalog source can be recovered.
- Richer behavior inside existing schema object types, as long as it does not
  expand the strict superset.

## Testing Strategy

Tests should match the architecture:

- Shared tests cover schema preparation, replay, graph ordering, diffing,
  disambiguation, SQL planning, embedded migrations, and public API behavior.
- Dialect catalog tests cover alias canonicalization, extension-type recognition,
  typo suggestions, and unknown-type preservation.
- PostgreSQL tests cover SQL rendering, schema-qualified behavior, live
  introspection, relational-core verification, opaque metadata, and deterministic
  errors for unsupported catalog shapes.
- SQLite tests are feature-gated and cover offline rendering, live in-memory
  execution, rebuild planning, rollback, and unsupported-feature failures.
- Negative tests are as important as positive tests: unsupported operations must
  fail before SQL execution when possible.
- `sql_migrate` golden tests should match the statements used by live migration
  application.

Required checks for broad changes:

```bash
cargo test
cargo test --features sqlite
cargo test --no-default-features --features sqlite
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline-sqlite --target wasm32-unknown-unknown
cargo clippy --all-targets
cargo clippy --features sqlite --all-targets
cargo clippy --no-default-features --features sqlite --all-targets
```

PostgreSQL integration tests require `TEST_DATABASE_URL` and should be run when
changes touch the PostgreSQL executor or live introspection.
