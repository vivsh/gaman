# Gaman Architecture

Gaman is an offline-first schema migration system. It turns a desired schema
and committed migration history into deterministic migration operations and SQL
without connecting to a database. A live database is needed only to apply
migrations, inspect its catalog, or verify owned schema state.

This document defines Gaman's conceptual model, pipelines, guarantees, and
boundaries. Command syntax, configuration, file locations, and other interface
details belong in the README and API documentation.

## Architecture At A Glance

```text
authored schema ------> prepare ----> input schema
migration history ----> replay -----> replayed schema
live database catalog -> inspect ----> inspected schema

input schema     - replayed schema -> diff -> clarification -> migration -> SQL plan
inspected schema - replayed schema -> drift -> local repair plan

SQL plan + target state -> live application -> tracked target state
```

## Goal

Gaman defines a strict, deliberately bounded schema model that can be authored
from multiple frontends and applied across supported SQL dialects. It is not a
general SQL parser, a database cleanup tool, or an attempt to model every
database feature.

The supported schema superset is:

- tables and columns, including type, name, nullability, default, and generated
  metadata;
- primary keys, foreign keys, indexes, unique constraints, and check
  constraints;
- schemas or namespaces where a dialect supports them;
- extensions, enums, functions, triggers, and views as opaque schema objects;
- raw SQL statements as an explicit escape hatch.

An unsupported or unimplemented feature must fail as early as Gaman can
deterministically establish that it is outside this model or dialect capability.
It must never become silently ignored SQL or a lossy no-op.

## Conceptual Model

Gaman works with three forms of the same schema representation. The two
subtractions in the overview are conceptual comparisons, not text or set
subtraction: desired state minus replayed state produces migration work;
inspected state minus replayed state produces live drift.

Every frontend builds the same intermediate representation before Gaman applies
normalization, canonicalization, validation, diffing, replay, or rendering.

| Representation | Origin | Role |
|---|---|---|
| Desired schema | Authoring frontend | States the intended schema. |
| Replayed schema | Committed migration graph | Defines migration-owned expected state. |
| Inspected schema | Live catalog | Describes the database state available for verification. |

## Granular And Opaque Objects

Gaman has two representation classes.

**Granular schema objects** have structured metadata that Gaman can compare and
modify at the supported operation level. Tables, columns, keys, indexes, and
constraints are the principal examples.

**Opaque schema objects** have a known kind and identity but source that Gaman
does not promise to understand semantically. Functions, triggers, views,
extensions, and advanced dialect-specific definitions may be opaque. They are
handled as create, drop, or replace units rather than as internally edited SQL.

Opaque source is preserved exactly as authored or first inspected. Gaman uses a
conservative lexical comparison only after exact text differs, so formatting
and comments do not create migration churn. That comparison is not used to
rewrite stored source or to claim live body equivalence.

| Representation | Migration behavior | Live verification behavior |
|---|---|---|
| Granular | Compare and render supported structural operations. | Compare dialect-registered stable properties. |
| Opaque | Create, drop, or replace the whole object. | Check owned presence and stable metadata only when registered. |

## Input And Preparation

Structured frontends provide modeled schema data. Schema SQL is a first-class
authoring format: it declares desired state with supported dialect `CREATE` DDL.
Changing schema SQL changes desired state; diffing that state against replayed
history produces migrations that can update a live target.

SQL input is intentionally bounded to recognized `CREATE` definitions. A
recognized definition that lowers fully becomes modeled schema. A recognized
definition that cannot be lowered safely may become an untrusted opaque object.
`ALTER`, `DROP`, data manipulation, transactions, and unrecognized object kinds
are not schema input; they fail rather than entering a partially understood
state.

Gaman has no abstract type language and performs no application-type to
database-type conversion. Schema SQL and structured frontends use the selected
database's type names directly. Known aliases can be canonicalized for
validation and comparison, but the database type remains the source language.

After input becomes schema IR, preparation performs dialect-agnostic
normalization, dialect-specific canonicalization, structural validation, and
dialect validation.

Normalization makes frontend shorthand explicit and deterministic. Examples
include deriving omitted object names, resolving table-level primary-key
metadata, and synchronizing convenience column flags.

Canonicalization is dialect-owned. It normalizes known type aliases and
dialect-specific metadata without rewriting opaque source. Dialect type catalogs
are intentionally incomplete: they assist normalization and typo suggestions;
they do not define the universe of valid database types.

SQL lexical analysis has one dialect-owned tokenization boundary. Segmentation,
statement classification, parser recovery, default-expression comparison, and
opaque comparison consume the same source-preserving tokens. Unquoted words
have uppercase comparison values; quoted identifiers, literals, and authored
source remain exact. Tokenization never rewrites schema or migration content.

Gaman accepts native database type syntax directly and performs no
application-type conversion. PostgreSQL recognizes stable user-declarable
built-ins and aliases from PostgreSQL 14 onward, including native `uuid`;
`pgcrypto` is an optional extension and not the provider of that type. SQLite
preserves declared type text and uses its documented affinity algorithm for
comparison. Both catalogs are UX metadata, never a closed validity registry.

Unknown types are handled through trust on first use. Known built-in aliases,
modeled types, extension types, and types already present in replayed history
are accepted. A newly introduced unknown type requires an explicit planning
decision before it can enter migration history. Migration history is therefore
the project-local approval record for custom and user-defined types.

## Deterministic Offline Pipeline

Planning, replay, diffing, clarification, and SQL rendering are deterministic,
offline, and side-effect free. They operate only on strings and in-memory
values; they do not need a driver, live connection, filesystem, environment,
terminal, or tracking store.

The same dialect rendering path is used for offline SQL planning and live
application. An operation that requires schema context, such as a SQLite table
rebuild, is rendered from the current replayed state rather than as an isolated
statement.

Raw SQL remains the escape hatch for intentional unmanaged work. It is rendered
as authored and does not mutate Gaman's replayed schema.

## Migration Graph And Replay

Migrations form a directed acyclic graph. Dependencies make ordering explicit;
parallel histories require an explicit merge migration before application can
continue with a single head.

Replay is deterministic, offline, and side-effect free. It applies ordered
migration operations to an in-memory schema and produces the same schema for
the same graph every time. It neither contacts a database nor consults ambient
state.

Rollback planning reverses selected migrations and their operations only when
every required inverse exists. If an inverse is unavailable, planning fails
before partial rollback SQL is emitted.

## Clarification And Risk

Gaman does not silently guess when a schema diff is ambiguous or risky. It
produces structured clarification requests for cases such as:

- possible table, column, or enum-value renames;
- new non-null data requirements;
- type changes requiring a cast expression;
- new unknown data types;
- destructive or coarse changes to opaque definitions;
- unmanaged dialect-specific metadata that cannot be migrated granularly.

Clarification is a planning boundary. An interactive frontend or library caller
supplies structured decisions; the migration engine does not depend on terminal
prompting.

Primary-key mutations are intentionally not generated automatically. They are
backend-sensitive schema surgery and require explicit SQL. Other supported
foreign-key and constraint changes can be planned when the dialect can render
them safely.

## Dialect Boundary

Each dialect owns its database-specific behavior:

- type aliases and known extension types;
- canonicalization and validation;
- SQL DDL rendering and capability errors;
- SQL DDL lowering from supported input;
- catalog interpretation during inspection;
- stable-property comparison during verification.

Shared planning code asks a dialect to prepare schema and render a migration; it
does not embed PostgreSQL, SQLite, or future MySQL syntax rules.

PostgreSQL, SQLite, MySQL, and MariaDB are separate dialect contracts. MySQL and
MariaDB share private family infrastructure for tokenization, wire execution,
and common DDL, but retain distinct processors, type canonicalization, catalog
interpretation, drift registries, evidence, and release gates. MySQL targets the
8.4 LTS line; MariaDB targets the 11.4 and 11.8 LTS lines.

SQLite uses a deterministic table-rebuild strategy for supported alterations
that SQLite cannot express directly. MySQL-family DDL implicitly commits, so
modeled schema migrations are non-atomic and partial failures are reported
without claiming rollback.

MySQL-family tables and stable column metadata are granular. Auto-increment,
generated storage, automatic update expressions, explicit character set and
collation, visibility, and comments participate in the normal modeled
lifecycle. Advanced indexes and stored program source use the opaque lifecycle.
Storage engines, table defaults, partitions, and other table-level vendor
clauses are unmanaged table options: they are preserved, clarified, rendered,
and excluded from live drift.

## Live Application Lifecycle

Live application adds execution and durable state around the same offline plan.
It validates the graph and target, serializes access to the target environment,
renders from replayed state, executes selected migrations, and updates applied
state only after successful work.

The target state may be ahead of or behind the current applied state. Application
therefore converges in the required direction while retaining the selected
target as applied.

| Phase | Architectural responsibility | Durable effect |
|---|---|---|
| Plan | Select and render migrations from replayed state. | None. |
| Apply | Execute the rendered migration at the target environment. | Database schema or data changes. |
| Track | Update applied-migration state after successful work. | Target state is recorded. |
| Recover | Roll back failed atomic work and release the lock. | Failed migration is not recorded. |

Atomic migrations execute inside their supported transaction boundary. A failed
atomic migration rolls back its work and is not recorded as applied. Lock
release is attempted after both successful and failed application paths.

Tracking applied migrations is a runtime concern, not an offline-planning
concern. The default native implementation stores tracking data in the target
database. The architecture permits other tracking stores for hosts such as a
browser without making the offline core depend on them.

## Inspection, Verification, And Repair

Inspection reflects live catalog state into the common schema representation.
It aims to be faithful without inventing unsupported structured semantics:

- fully recoverable objects become modeled objects;
- known objects with unrecoverable details become opaque objects when possible;
- source is preserved where the catalog exposes it;
- unsupported catalog shapes fail clearly rather than being mis-modeled.

Verification is ownership-scoped drift detection, not a full-database cleanup
planner. The final replayed schema defines ownership. Live-only tables, columns,
indexes, constraints, foreign keys, triggers, views, enums, functions, and
extensions are ignored. Missing or changed owned metadata is reported as drift.

Verification compares only metadata that a dialect can inspect consistently.
For opaque objects, presence and registered stable metadata may be checked, but
body or source changes are not considered reliable live drift evidence. Gaman
does not claim body equivalence when the database may reformat or rewrite source.

Repair is local recovery from verified drift. It can plan one-off SQL for safe,
renderable granular differences and selected missing opaque objects with trusted
source. It does not write migration history, alter tracking state, or turn
repair work into a migration implicitly.

## Runtime And Storage Boundaries

The offline core owns the schema model, preparation, replay, graph handling,
diffing, clarification, dialect rendering, offline planning, and the reusable
migration lifecycle engine. The lifecycle engine depends only on caller-supplied
storage, tracking, and SQL-execution traits; it performs no filesystem, network,
database-driver, or runtime I/O. It is featureless and practical for browser/WASM
use.

Native integrations add live execution, inspection, tracking, filesystem-backed
sources, and presentation layers. Migration definitions and applied-migration
state are separate storage concerns so callers can provide in-memory, embedded,
filesystem, database, or host-specific implementations as appropriate.

Hosts cross the command boundary in three stages. `command_args` defines the
shared token grammar, help, and parser diagnostics through `argh` annotations.
Each host resolves paths, configuration, and direct schema input into a typed
runner command. `MigrationRunner` is the sole lifecycle-command facade over a
flexible `MigrationEngine`; it coordinates migration storage, tracking, SQL
execution, inspection, drift, and repair through caller-owned adapters. It does
not read process arguments, files, environment variables, stdin, or produce
terminal output.

Runner commands form a versioned, serializable protocol. Commands are borrowed
during execution, so a host retains resolved input and can create an immutable
retry with additional clarification decisions. Results and diagnostics contain
only Gaman-owned data; driver errors and host handles do not cross this
boundary. CLI presentation, WASM serialization, and future FFI bindings are
adapters over the same command contract.

Each runner command observes one immutable `MigrationCatalog`. The runner loads
and validates the catalog once, then creates a command-scoped engine view backed
by that snapshot. The base engine never retains command state, so cancellation
or failure cannot affect a later command. Direct `MigrationEngine` calls remain
available and observe a fresh snapshot for each independent call.

Storage, tracking, execution, and inspection errors retain distinct categories.
Database tracking orchestration is generic, while installation, listing,
recording, and removal SQL are owned by the selected dialect processor.

Clarification is a typed suspension, not host behavior. A runner returns
`CommandError::NeedsInput` with the exact clarification list. The host may
collect `Decision` values, add them to the same resolved make command, and
retry. CLI uses terminal prompts; WASM and future FFI hosts return the request
as structured data.

The native crate contains only configuration/path resolution, lazy SQLx
connection adapters, filesystem migration storage, and presentation. It does
not have a second migrator or command-orchestration layer. WASM follows the same
runner protocol. Exact token arrays are its only textual browser input and use
the shared `argh` grammar; command-line string splitting is not a host contract.

| Concern | Required environment |
|---|---|
| Preparation, replay, diff, clarification, and SQL planning | Offline and in-memory. |
| Migration application, inspection, and verification | A live target database. |
| Migration definitions and applied-state tracking | Caller-provided storage appropriate to its host. |

## Evidence And Limits

Gaman uses layered evidence:

```text
Rust unit tests
  -> implementation invariants
offline YAML fixtures
  -> deterministic parser, replay, diff, clarification, and SQL behavior
online YAML fixtures
  -> live application, inspection, verification, and data behavior
accepted evidence
  -> public support matrix
```

The README support matrix is generated from accepted evidence and explicit
design boundaries. A green claim requires evidence; a bounded or unsupported
claim must explain its limit.

Current deliberate limits include:

- Gaman models a strict schema superset rather than all database features.
- SQL schema input accepts recognized `CREATE` definitions only.
- Opaque source changes are not reliable live verification inputs.
- Primary-key mutation generation remains manual SQL.
- MySQL and MariaDB support is released independently only after each product's
  parser, offline, drift, and live evidence gates pass.

Architecture changes must be deliberate. Implementation must not silently
broaden, weaken, or contradict these guarantees.
