# Gaman Architecture

Gaman is conceived as a desired-state migration system for applications that
need migrations to be deterministic, reviewable, embeddable, and safe across
multiple database products.

It does not begin with a live database and attempt to infer migration history.
Instead, it combines a declared target schema with committed migration history
to plan the next migration offline. Live database access is reserved for
application, inspection, verification, repair, and verified adoption.

This document explains that design and the responsibilities around which the
implementation is organized. Command usage belongs in the [README](README.md),
while exact compatibility claims belong in the generated
[support evidence](docs/support-evidence.md).

## Why Gaman Exists

Migration-first tools make every schema change begin as an imperative script.
Live-diff tools make the current database an input to migration planning. Both
approaches can make it difficult to answer what an application owns, what state
it expects, and whether the same migration would be generated elsewhere.

Gaman starts from a different premise:

- Applications should declare the database state they intend to own.
- Committed migrations should remain the durable history of that ownership.
- Migration generation should not depend on mutable production state.
- Ambiguous or destructive changes should require explicit decisions.
- Database-specific behavior should remain visible rather than being hidden
  behind an inaccurate lowest common denominator.

The result is a workflow in which desired state describes the destination,
migration history describes the journey, and live inspection checks whether the
database still satisfies that contract.

## The Three Sources of Information

### Desired state

Desired state is the application's declaration of what should exist. It can be
composed from SQL, YAML, JSON, and Rust declarations. Source order and file
layout are organizational choices and must not change schema meaning.

Desired schema is primarily create-oriented. It describes entities rather than
an ordered list of alterations.

### Committed migration history

Committed migrations are replayed to reconstruct the current Gaman-owned state.
They are the planning baseline and the durable audit trail. Migration
dependencies form an explicit history rather than relying on filesystem order.

### Live database state

Live state is evidence, not planning input. Gaman inspects it to verify owned
entities, identify drift, construct checked repairs, or confirm that an existing
schema can be safely adopted. Live differences never silently rewrite desired
state or committed history.

## End-to-End Lifecycle

Gaman follows one lifecycle regardless of whether it is used through the CLI or
embedded in an application:

1. Compose the complete desired schema.
2. Normalize and validate declarations for the selected dialect.
3. Replay committed migrations into canonical owned state.
4. Compare replayed state with desired state semantically.
5. Surface decisions for ambiguous, destructive, or database-sensitive changes.
6. Produce a deterministic, dependency-ordered migration.
7. Render operations using the selected database dialect.
8. Apply operations and record success through the migration store.
9. Inspect and verify the live database against committed ownership.
10. Repair verified drift or adopt matching existing state when explicitly requested.

The same typed migration operations are used for rendering, replay, rollback,
execution, verification repair, and protocol transport. This prevents each host
from developing a different interpretation of a migration.

## Schema Composition and Validation

Applications often define one schema across several modules and formats. Gaman
therefore treats schema loading as composition followed by terminal validation.

Declarations may refer to entities contributed elsewhere. Validation occurs
after composition so declaration order does not become a hidden dependency.
Conflicting identities, unresolved references, invalid ownership, and
dialect-incompatible declarations fail before migration planning.

Canonicalization gives equivalent declarations one stable representation. That
representation supports deterministic fingerprints, migration output, replay,
and comparison.

## Ownership Model

Gaman distinguishes three forms of owned state.

### Modeled entities

Modeled entities have structure that Gaman understands well enough to validate,
compare, and migrate granularly. Tables, columns, keys, constraints, and ordinary
indexes belong here.

### Opaque entities

Opaque entities have a known kind, identity, dependency boundary, and lifecycle,
but their SQL is treated as an indivisible declaration. This is appropriate for
advanced or database-specific objects whose internal SQL cannot be interpreted
safely and generally.

Opaque does not mean unmanaged. Gaman can own creation, presence, replacement,
and removal while refusing to invent granular edits.

### Managed rows

Managed rows are the bounded exception to create-only desired state. They allow
applications to declare selected data whose lifecycle belongs with schema
migrations, such as task lanes, roles, or application configuration.

Ownership is limited to declared row identities and declared columns. Other rows
and columns in the same table remain outside Gaman ownership. Managed rows are
expected to be changed through desired schema and migrations rather than by
external application writes.

## Migration Planning

Planning is deliberately offline and semantic:

- History is replayed before desired state is compared.
- Equivalent database types and expressions are normalized by the dialect.
- Renames, casts, destructive operations, and other uncertain changes require
  explicit clarification rather than heuristic execution.
- Entity and foreign-key dependencies determine operation order.
- A generated migration is replayed from its committed baseline before it is
  accepted as a valid candidate.
- Stable ordering and canonical identities keep output independent of source
  file order and filter order.

Filtered migration generation narrows the roots selected for one invocation. It
does not create staging state or a second migration workflow. Required changed
dependencies remain part of the selected migration so replay stays valid.

## Execution and Tracking

Execution is intentionally narrower than planning. The migration already
contains the decisions and dependencies needed for the database operation.

The execution boundary is responsible for database I/O, locking, transactions,
affected-row reporting, and migration tracking. It is not allowed to introduce
new migration policy.

Migration success is recorded only after the required database work succeeds.
Transactional databases use rollback on failure. Databases with non-transactional
DDL report partial failure explicitly and do not pretend that earlier statements
were reverted.

Operations that depend on an expected live value use checked writes. A zero-row
or multi-row result is treated as drift or an integrity failure rather than as
successful migration progress.

## Inspection, Verification, Repair, and Adoption

Inspection reflects representable database structure without turning arbitrary
database contents into desired state.

Verification compares live state only with entities owned by committed
migrations. Unmanaged entities are ignored unless they conflict with an owned
identity. Managed-row inspection is targeted to declared keys and never becomes
an unrestricted table dump.

Repair is projected from verified drift into the same typed operations used by
normal migrations. Repair uses observed values as preconditions so concurrent
changes fail safely.

Verified adoption records an existing migration only after the live database is
shown to satisfy its owned result. It is not an unchecked fake application.

## Dialect Boundaries

PostgreSQL, SQLite, MySQL, and MariaDB share the lifecycle but not an artificial
feature set. Capability parity is not a goal.

Each dialect owns its type semantics, SQL rendering, schema validation,
inspection normalization, drift comparison, execution capabilities, and
unsupported boundaries. Shared behavior may be reused, but one database's
syntax or catalog behavior must not be assumed for another.

When a database cannot represent a change safely, Gaman returns a clear error or
requires an explicit decision. It does not emit plausible-looking SQL merely to
make dialects appear uniform.

## Embedding Boundary

Gaman is planned as an embeddable migration engine rather than a CLI-owned
workflow. The core lifecycle is independent of terminal interaction and native
database connectivity.

Hosts provide schema contributions, migration storage, clarification decisions,
and database adapters where required. The CLI, native applications, and portable
hosts use the same migration and command contracts. Protocol incompatibility
must fail before a host can silently ignore a migration capability.

## Implementation Responsibilities

The implementation is organized around stable responsibilities rather than one
large migration pipeline:

1. **State model:** represents canonical desired and replayed ownership.
2. **Input boundary:** parses and composes SQL, structured files, and builders.
3. **Planning boundary:** replays history, computes semantic changes, resolves
   clarifications, and orders dependencies.
4. **Dialect boundary:** validates database-specific capability and renders SQL.
5. **Execution boundary:** performs database I/O, locking, transactions, and tracking.
6. **Assurance boundary:** inspects, verifies, repairs, and verifies adoption.
7. **Evidence boundary:** turns parser, offline, and live fixtures into published
   support claims.

These responsibilities are kept separate so adding a dialect, host, or schema
source does not create a second planning model. Policy belongs in the shared
state and planning contracts; adapters delegate rather than reinterpret it.

## Non-Goals

Gaman does not aim to:

- Infer arbitrary dependencies from SQL bodies.
- Use live drift as an automatic migration generator.
- Manage rows or entities that were never declared as owned.
- Hide destructive changes behind automatic guesses.
- Promise identical features across different database engines.
- Replace explicit SQL migrations for arbitrary bulk data transformations.

## Architectural Standard

New features should preserve offline determinism, canonical replay, explicit
ownership, reversible operations where possible, dialect honesty, and
evidence-backed support claims.

If a feature cannot fit those boundaries, it should remain explicit or
unsupported rather than creating a hidden alternative workflow.
