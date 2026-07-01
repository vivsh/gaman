# Gaman Architecture

Gaman is an offline-first migration engine for applications that need
deterministic schema planning without depending on a live database at generation
time.

The long-term SQLx database targets are:

- PostgreSQL.
- SQLite.
- MySQL / MariaDB.

## Conceptual Model

All schema frontends are reduced to the same internal `Schema` representation.
Schema frontends enter the model through:

- YAML and JSON schema files.
- SQL DDL parsing.
- Rust structs through `IntoTable`.
- Live database introspection.
- Replayed migration history.

The internal schema is the comparison boundary. Once inputs are normalized into
that model, the diff engine compares current desired state against previous
replayed state and emits operations.

```text
YAML / JSON schema --+
SQL DDL schema     --+--> Schema IR --+
Rust structs       --+                |
                                      +--> DiffEngine --> operations --> migration
migration history --> replay --------+
```

At the highest level, Gaman moves through this pipeline:

```text
Schema input --> Replay --> Diff --> Operations --> SQL --> Executor
```

Migration replay is deterministic, offline, and side-effect free. It consumes
migration files and produces an in-memory `Schema`; it never connects to a
database or mutates external state.

The key property is that migration generation depends only on:

- The desired schema input.
- The migration files already present.
- The selected dialect.

It does not depend on live database state.

## Schema Representation

Gaman defines a strict, deliberately bounded schema superset:

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
  - foreign keys;
  - unique constraints;
  - check constraints.
- Extensions.
- Triggers.
- Functions.
- Views.

No feature outside this superset is planned as first-class model metadata. Use
`Statement` for database-specific features outside the superset.

Primary keys are table-level schema metadata. Frontends may use column-level
`primary_key: true` as shorthand, including on multiple columns, but
normalisation must produce explicit, deterministic `Table.primary_key` metadata
with a constraint name and ordered column list. Explicit table-level
`primary_key` input preserves its name and order; shorthand derives the name
from the table and the order from the table's column order.

In this document, the term opaque schema objects means extensions, triggers,
functions, and views. Gaman tracks their identity and definition as whole
objects; it does not structurally diff their internals. The supported lifecycle
for opaque schema objects is create, replace, and delete. Gaman should not
generate fine-grained `ALTER` statements for them.

If a database does not have one of these object types, that type is absent for
that dialect. It should not be forced into the dialect's introspection or diff
contract. If a migration explicitly asks that dialect to execute an absent or
unimplemented feature, Gaman should raise an early, clear error rather than emit
no-op SQL.

## Design Principles

- Migration generation is offline and deterministic.
- Migration replay is offline and deterministic.
- Offline generation must not require a selected live database driver.
- Offline planning, replay, diffing, disambiguation, canonicalization, and SQL
  rendering must compile without SQLx, Tokio, filesystem access, environment
  variables, terminal I/O, TLS, or database executors.
- The offline layer should be practical for browser use through
  `wasm32-unknown-unknown`; browser callers provide schemas and migrations as
  strings or in-memory values.
- SQL rendering for `sql_migrate` should be offline but should match the SQL
  that migration application will run.
- Migration application and `inspect_db` require a live database.
- Unsupported and unimplemented features should fail as early as possible.
- Dialects should support native behavior for their engine; migration files are
  not expected to be portable across database engines.
- Shared code owns graph, replay, diff, validation orchestration, and lifecycle.
  Dialect-specific behavior belongs in dialect and executor modules.
- Implementation must not deviate from this architecture. If the architecture
  is wrong or incomplete, update it deliberately before changing code away from
  it.

## Non-Goals

- Full coverage of every PostgreSQL, SQLite, MySQL, or MariaDB DDL feature.
- Modeling database features outside the strict superset.
- Portable migration files across database engines.
- Automatic modeling of arbitrary SQL in `Statement` operations.
- Lossy introspection that silently ignores unsupported schema shapes.
- Runtime migration discovery for embedded use. Embedded migrations should be
  compiled into the binary.

## Normalization And Canonicalization

Gaman has two cleanup layers because schema input can come from several
frontends: Rust, YAML, JSON, SQL authored by hand, SQL generated from database
introspection, and replayed migrations.

Normalization is database-agnostic. It handles shared model sugar and should not
encode database-specific assumptions:

- Inline column `references` becomes table-level foreign-key metadata.
- Inline column `check` becomes table-level check-constraint metadata.
- Inline trigger bodies become modeled function definitions where supported.
- Missing table names can be filled from schema map keys.

Canonicalization is dialect-specific. It cleans semantically equivalent frontend
output into the comparison form for one database engine:

- Type aliases such as PostgreSQL `int4` to `integer`.
- SQLite type names and, over time, SQLite affinity-aware comparison.
- Schema qualification rules.
- Dialect-specific object identity rules.
- Catalog quirks from live introspection.

PostgreSQL assumptions such as `public` schema handling must not leak into
SQLite validation. SQLite should reject schema-qualified objects rather than
silently canonicalizing them away.

## Migration Graph

Migrations form a DAG. Each migration declares its dependencies, and the graph is
validated at `Migrator` construction. The migrator caches topological order so
later planning and execution can reuse the same graph state.

The DAG exists to make ancestry explicit. It lets independent branches coexist
while requiring a deliberate merge migration before new generated work can build
on more than one head.

Parallel histories are valid only when joined by an explicit merge migration.
`make_migration` refuses to generate ordinary migrations when multiple heads are
present, because otherwise new operations would have ambiguous ancestry.

For embedded multi-crate use, child migration trees are namespaced at compile
time. An application crate can combine migrations from several crates without
runtime file discovery or ID collisions.

## Pipeline

### Construction

`MigrationEngine` is the embedding-facing API. It wraps configuration, embedded
migration sources, optional schema input, and optional dialect override.

`Migrator` is the core engine. Construction performs the first critical checks:

1. Load all migrations from the selected source.
2. Insert them into the migration graph.
3. Validate dependency integrity.
4. Cache the graph's topological order.

### Migration Generation

`make_migration` follows this flow:

1. Reject graph conflicts.
2. Validate the desired schema.
3. Replay existing migrations into previous schema state.
4. Diff desired state against replayed state.
5. Run the disambiguator for ambiguous or risky changes.
6. Let the selected dialect reorder operations if needed.
7. Compute dependencies from touched namespaces and graph history.
8. Save the new migration unless running in dry-run mode.

The disambiguator is part of the generation pipeline, not an afterthought. It
should run before migration files are written and should cover:

- Ambiguous operations, such as rename candidates that otherwise look like
  drop-and-add.
- Risky operations, especially data-loss or data-rewrite changes.
- Changes that need explicit user intent, such as casts or backfills.

### SQL Rendering

`sql_migrate` renders the same SQL plan that live migration application will run,
but without connecting to a database. Rendering therefore uses replayed schema
state when an operation needs context, such as SQLite table rebuilds.

Partially supported operations may emit SQL comments only when the live path
would do the same thing. If live migration would fail, offline SQL rendering
should fail early too.

`Dialect::operation_to_sql()` remains a simple single-operation renderer. SQLite
operations that require rebuild context should fail there and instruct callers to
render through `Migrator`.

### Live Migration

`migrate` and `migrate_with` use the lifecycle shown below for every dialect:
validate the plan, install tracking, acquire the lock, apply or roll back the
selected migrations, and release the lock on success or failure.

Each migration is applied independently. SQL is rendered before execution, a
transaction is used when `atomic: true`, the migration ID is recorded only after
SQL succeeds, and SQL or record failures roll back that migration transaction.

Rollback builds inverse operations in reverse order and renders them through the
same dialect path. Operations without an inverse make rollback fail before any
SQL is executed.

### Tracking

Applied migrations are stored in `gaman_migrations`. Dialects provide the
tracking table DDL and applied-migration query. Recording and unrecording use the
same migration IDs that appear in the graph.

### Verification

`verify_db` compares live introspection against replayed migration state.

Verification is strongest for the strict relational core:

- Tables.
- Columns.
- Indexes.
- Foreign keys.
- Unique and check constraints where introspection can model them.

For opaque schema objects, verification should track signatures and identities
where the dialect can introspect them deterministically:

- Functions: schema, name, arguments/signature, return type, language, and other
  stable metadata where available.
- Triggers: table, name, timing, events, scope, and referenced function or action
  identity where available.
- Extensions: name, schema, and version where available.
- Views: schema, name, and stable definition metadata where available.

Opaque schema object bodies and view definitions are preserved exactly as
authored, or as first exported by `inspect_db`. Gaman does not rewrite this
source when storing, replaying, rendering, or writing migrations. Offline diff
first compares source text exactly; only on mismatch does it use a conservative,
dialect-agnostic lexical canonicalizer to ignore formatting-only differences
outside quoted and protected regions.

Live database catalogs may normalize, rewrite, or omit source text, so
`verify_db` does not compare opaque bodies or view definitions against catalog
text. It verifies only deterministic metadata such as identity, signature,
language, trigger wiring, extension version, and enum labels.

## Lifecycle

The complete migration lifecycle is:

```text
             offline                                live database required
             -------                                ----------------------

schema input --> normalize --> canonicalize --+
                                              |
migrations --> load graph --> replay ---------+--> diff --> disambiguate
                                                        |
                                                        v
                                               write migration file
                                                        |
                                                        v
                                              render SQL offline
                                                        |
                                                        v
                            install tracking --> acquire lock --> apply/rollback
                                                        |             |
                                                        |             v
                                                        |       record/unrecord
                                                        |             |
                                                        +--> release lock

inspect_db -----------------------------------------> live introspection
verify_db  --> replay offline ----------------------> compare live schema
```

Generation, replay, diffing, disambiguation, and SQL rendering are offline.
Application, `inspect_db`, and the live side of `verify_db` require a database
connection.

## Dialect Boundary

The shared engine should know about operation sequencing, replay, graph state,
validation hooks, disambiguation, and lifecycle. It should not know how a
specific database quotes identifiers, rebuilds tables, acquires locks, or
handles unsupported features.

Dialect modules own:

- SQL rendering.
- Dialect-specific operation validation.
- Type canonicalization.
- Operation reordering when required.
- Tracking-table SQL.
- Unsupported-feature errors.

Executor modules own:

- Database connections.
- Statement execution.
- Transaction commands.
- Lock acquisition and release.
- Live introspection.

This keeps cross-database leakage visible. For example, SQLite-specific rebuild
planning belongs in the SQLite dialect module, while PostgreSQL advisory locks
belong in the PostgreSQL executor.

## Offline Core And WASM Goal

Gaman is split around an offline-first core:

- `gaman-core` is the pure offline engine. It contains schema IR, operations,
  migrations, graph ordering, replay, diffing, disambiguation data structures,
  dialect canonicalization, dialect SQL rendering, string-based schema parsing,
  and `OfflinePlanner`.
- `gaman-core` physically owns the offline implementation modules under
  `gaman-core/src/`. The root `gaman` crate re-exports those modules for
  compatibility and must not include core implementation files through path
  bridges.
- `gaman` remains the compatibility facade. Default features keep the native CLI
  and database behavior. `--no-default-features --features offline` exposes the
  offline core without compiling database drivers.
- Native database execution, live introspection, locking, tracking-table
  installation, and live `verify_db` are native-only concerns behind DB features.
- CLI parsing, dotenv loading, terminal prompting, and filesystem-backed
  migration writing are native-only concerns behind CLI/filesystem features.

The offline acceptance target is:

```text
cargo check -p gaman-core --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
cargo check -p gaman --no-default-features --features offline-sqlite --target wasm32-unknown-unknown
```

Offline builds must not compile SQLx, Tokio, argh, dotenvy, native TLS, or
executor modules. `sql_migrate` remains offline by using the same renderer that
live migration application uses for the selected dialect and replay state.
Use the facade's `offline-sqlite` feature, or `gaman-core` with its `sqlite`
feature, when browser/offline callers need SQLite SQL rendering without a live
SQLite driver.

## Dialect Scope

Each dialect should implement the strict schema superset where the database has
a matching concept, and should raise deterministic errors where the database
does not. Per-engine support status belongs in the README so users see it before
they read internals.

## Escape Hatches

`Statement` is the supported escape hatch for SQL that Gaman should execute but
not model. It participates in migration ordering and transaction handling, but
it does not alter the replayed `Schema`.

That means a `Statement` can be used for explicit data fixes, specialized
indexes, database-specific clauses, triggers, or one-off DDL. If the statement
changes objects that Gaman also models, the authored schema and later migrations
must account for the resulting state explicitly.

Gaman should not define first-class data migration operations or external
CSV/JSON data-file references. Frontends may read data files or domain inputs
and compile them into explicit `Statement` operations before handing migrations
to Gaman, but the migration file itself must remain self-contained.

External process invocation is outside Gaman's migration contract. `Invoke`,
invoker traits, subprocess execution, and related entities should be removed
from the operation model and native execution layer. Migration application must
execute database operations only; external tooling belongs before migration
generation, not inside migration application.

## Planned Features

The planned feature set is bounded by the strict superset. Planned work should
improve correctness, coverage, and robustness inside that superset rather than
add new schema object types.

### High Priority

- Harden SQLite canonicalization:
  - reject schema-qualified SQLite objects before shared schema normalization;
  - compare SQLite types by affinity where appropriate;
  - preserve authored type declarations for rendering;
  - normalize defaults and generated expressions conservatively.
- Improve SQLite introspection:
  - parse `sqlite_master.sql` for table constraints and generated columns;
  - parse stable view metadata where supported;
  - detect unsupported table shapes instead of silently dropping metadata;
  - cover supported relational-core inspect/verify with live in-memory tests.
- Mature SQLite rebuilds:
  - keep parent-table rebuilds with inbound foreign keys covered by live tests;
  - preserve modeled indexes and constraints across more rebuild scenarios;
  - keep primary-key changes explicitly unsupported until designed.
- Make `sql_migrate` and live migration rendering share the same renderer for
  every dialect, including context-aware paths.
- Improve opaque schema object tracking:
  - track signatures and stable metadata through introspection;
  - keep authored source preservation separate from live metadata verification.

### Medium Priority

- Add SQLite trigger rendering as an opaque schema object if it can be kept
  separate from PostgreSQL trigger-function semantics.
- Add or harden view rendering and introspection for dialects where view
  definitions can be represented deterministically as opaque schema objects.
- Add more round-trip tests:
  - schema to migration to replay;
  - migration to SQL golden output;
  - live inspect to verify no drift for supported objects.
- Improve generated migration durability:
  - temp-file writes;
  - atomic rename;
  - parent-directory creation;
  - refusal to overwrite existing migration IDs.
- Expand benchmark coverage for large diffs, long histories, SQL rendering, and
  replay.

### Lower Priority

- MySQL / MariaDB dialect and executor support.
- Deep body-drift verification for opaque schema objects where canonical source
  text can be recovered deterministically from the live database.
- Richer behavior inside existing schema object types, as long as it does not
  expand the strict superset.

## Testing Strategy

Tests should match the architecture:

- Shared tests cover replay, graph ordering, validation, diffing, file loading,
  embedded migrations, and the public engine API.
- PostgreSQL tests cover SQL rendering, schema-qualified behavior, live
  introspection, relational-core verify, opaque schema object signatures, and
  deterministic errors for unsupported catalog shapes.
- SQLite tests are feature-gated and cover both offline rendering and live
  in-memory execution, especially rebuilds and rollback.
- Negative tests are as important as positive tests: unsupported operations must
  fail before SQL execution when possible.
- `sql_migrate` golden tests should match the statements used by live migration
  application.

Required checks for broad changes:

```bash
cargo test
cargo test --features sqlite
cargo test --no-default-features --features sqlite
cargo clippy --all-targets
cargo clippy --features sqlite --all-targets
cargo clippy --no-default-features --features sqlite --all-targets
```

PostgreSQL integration tests require `TEST_DATABASE_URL` and should be run when
changes touch the PostgreSQL executor or live introspection.
