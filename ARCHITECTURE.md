# Gaman Architecture

Gaman is an offline-first schema migration system. It turns desired database
state and committed migration history into deterministic migrations, while
keeping live inspection and drift verification as separate concerns.

This document explains the purpose, philosophy, and concepts behind Gaman. CLI
usage belongs in the README; exact support claims belong in the generated
evidence documentation.

## Purpose

Many migration tools begin with hand-written migration scripts or a live
database comparison. Gaman begins with two committed facts:

- the schema a project wants;
- the migrations the project already owns.

Replaying migration history reconstructs expected state. Comparing desired
state with that replayed state produces the next migration. Because planning
does not depend on ambient database state, the same inputs produce the same
result on a developer machine, in CI, and during review.

## Philosophy

Gaman follows a small set of principles:

- **Committed history is authoritative.** Migration ownership comes from the
  migration graph, not everything found in a database.
- **Planning is offline.** A database is not needed to calculate or render the
  next migration.
- **Database syntax remains visible.** Gaman uses native database types and DDL
  rather than inventing a portable type language.
- **Risk is explicit.** Ambiguous and destructive changes require clarification
  instead of silent guesses.
- **Support is bounded.** Unsupported syntax fails clearly or enters a declared
  opaque lifecycle; it is never silently discarded.
- **Evidence controls claims.** Public compatibility statements follow accepted
  parser, offline, and live fixtures.

## The Three Schema States

Gaman reasons about three related schema states:

| State | Source | Purpose |
| --- | --- | --- |
| Desired | Authored schema | Describes what the project wants |
| Replayed | Committed migrations | Describes what migration history owns |
| Inspected | Live database catalog | Describes what is deployed |

Desired and replayed state are compared to generate migrations. Replayed and
inspected state are compared to detect drift. These comparisons have different
goals and intentionally use different equality rules.

## CREATE-Only Desired State

Gaman tracks supported DDL `CREATE` definitions only. A schema file declares
objects that should exist; it is not a stream of commands to execute.

Inline column checks and references are authored-schema shorthand. Committed
migrations and replayed state use canonical named constraints and foreign keys,
so planning, replay, rendering, and verification share one entity identity.

Supported desired state can include:

- tables and columns;
- primary and foreign keys;
- unique and check constraints;
- indexes;
- enums and extensions where the dialect supports them;
- functions, triggers, and views.

Functions use typed parameters, including PostgreSQL default expressions. Their
identity is schema-qualified name plus ordered parameter types; defaults and
parameter names do not change an overload identity. Function dependencies are
explicit root-entity edges, never inferred from SQL bodies or source-file order.
YAML and Rust use `kind::target` declarations. SQL files use repeatable leading
`-- @depends-on kind::target` comments on `CREATE FUNCTION`; unknown leading
`@` directives fail during segmentation and are not database metadata.

`ALTER`, `DROP`, transaction control, and arbitrary utility statements do not
become desired schema state. The one bounded data exception is a top-level
managed-row declaration: a finite set of keyed configuration/reference rows
whose table is present in the final composed schema. Structural and managed-row
changes are derived by comparing desired and replayed state. Arbitrary data
transformations and unsupported database surgery remain explicit migration SQL.

Managed rows preserve this boundary by using stable unique keys, structured
values, deterministic replay, and explicit insert/update/delete migration
operations. They never claim unrelated rows or turn authored SQL into desired
DML.

## Modeled and Opaque Objects

Gaman does not pretend every database object can be represented safely at the
same granularity.

**Modeled objects** expose structure that can be compared and migrated in
smaller operations. Tables, columns, keys, ordinary constraints, and ordinary
indexes are the main examples.

**Opaque objects** have a known kind and identity, but their internal SQL is not
interpreted deeply enough for safe granular edits. Functions, triggers, views,
extensions, and advanced index definitions may use this lifecycle.

Opaque does not mean ignored. Opaque objects are migration-owned, can be
created or removed, and can participate in coarse replacement. Formatting and
comments do not create authored migration churn. Live verification is more
conservative: opaque definitions are generally verified by owned presence and
stable modeled metadata, not body-text equivalence.

SQL input and `SchemaBuilder::opaque` may introduce untrusted opaque
definitions. They use the same statement classifier and raw fallback lowerer,
then converge into the same validation, fingerprinting, clarification,
migration, and replay lifecycle. YAML and JSON remain structured-only authored
formats.

Opaque entity source is exactly one plain `CREATE` statement. Authored and
committed opaque definitions cannot contain `CREATE OR REPLACE` or `CREATE IF
NOT EXISTS`: Gaman owns existence, replacement, and removal. Accepted opaque
replacement is a clarified `DROP` followed by the stored plain `CREATE`; raw
migration statements remain the explicit escape hatch for custom lifecycle SQL.

Tables are always modeled. Dialect-specific table clauses that Gaman cannot
manage granularly may be preserved as unmanaged options, but the table body and
its core entities must remain understandable.

Rust table builders may append unmanaged prefixes and suffixes around that
modeled body. These clauses are preserved for table creation; changing them on
an existing table records an acknowledged state change but requires explicit
raw migration SQL for the physical database alteration.

PostgreSQL range partitioning is modeled table metadata. A partitioned parent
names its range key, and each child partition is a table identity with explicit
inclusive start and exclusive end bounds. Child creation depends on its parent,
so migration ordering is deterministic in both directions. Gaman can create or
remove a modeled partition hierarchy, but it never converts an existing plain
table into a partitioned table automatically; that data-moving transition
requires an explicit raw SQL migration.

## Deterministic Migration Planning

Migration history forms an explicit dependency graph. Replaying that graph
produces expected schema state. Comparing it with desired state produces
ordered operations and a reviewable migration artifact.

Planning includes normalization, dialect-aware comparison, clarification, and
SQL rendering, but none of these steps need a live database. Rollback planning
uses the same history and fails before emitting a partial plan when a safe
inverse is unavailable.

Migration generation may apply invocation-scoped root-entity filters after the
semantic diff is known. Filtering is not persisted staging: it selects complete
owned operation groups, adds the minimum changed dependency closure, limits
clarification to that candidate, and validates the result by replaying it from
the committed baseline. An unfiltered invocation retains the normal global
planning path and exposes any changes left for later migrations.

Filters use canonical `kind::glob` syntax and retain legacy `kind:glob` input
compatibility. Dependency selectors use exact `kind::target` identities only;
they never accept globs and name-only function references must resolve uniquely.

Raw SQL remains an escape hatch inside migration history. It is executed as
authored, but it does not silently mutate Gaman's modeled replayed schema.

## Clarification and Trust

Some differences cannot be interpreted safely from structure alone. Examples
include possible renames, casts, new non-null requirements, unfamiliar types,
and destructive replacement of opaque objects.

Gaman suspends planning and asks for a structured decision. Accepted decisions
become part of committed migration history, so replay does not ask again.

Primary-key mutation is intentionally manual because it is backend-sensitive
schema surgery. Composite primary and foreign keys themselves remain modeled
and tracked.

## Dialect Honesty

Each supported database has its own contract for native types, canonical forms,
DDL capabilities, inspection fidelity, and drift comparison. Shared concepts
do not imply identical SQL or identical support.

PostgreSQL has the broadest coverage. SQLite uses table reconstruction for
supported changes its native alteration syntax cannot express. MySQL and
MariaDB have separate support evidence and release boundaries despite their
shared family history.

When a dialect cannot represent or execute a change safely, Gaman reports that
limit rather than emitting plausible-looking SQL.

## Live Application

Applying migrations adds locking, execution, and durable migration tracking to
the offline plan. A migration is recorded only after its required work
succeeds.

Transactional databases can roll back failed atomic work. Databases whose DDL
implicitly commits cannot provide migration-level rollback; Gaman reports
partial completion honestly and leaves the failed migration unrecorded.

The tracking table records migration state. It is not used to infer desired
schema and does not replace replay.

## Inspection, Drift, and Repair

Inspection reflects a live database into Gaman's schema concepts. Its goal is
faithful onboarding, not forcing every catalog detail into a granular model.
Known objects should remain visible even when advanced details are opaque.

Verification compares migration-owned replayed state with inspected state.
Only properties that a dialect can recover accurately and deterministically
are drift inputs. Findings identify the entity, property, expected value, and
observed value.

Live-only objects outside migration ownership are ignored. Missing owned opaque
objects can be detected, but body-only changes are not claimed as drift when
catalog source cannot be compared reliably.

Repair is local recovery from verified drift. It plans or applies safe repairs
without creating migration files or changing migration history. This keeps
environment-specific drift out of the shared migration graph.

Repair operations remain typed. Where PostgreSQL requires a proven explicit
conversion, such as `text` to `jsonb`, the projected `AlterColumn` carries its
own `USING` expression. Repair never guesses arbitrary conversion SQL; normal
migration planning retains its explicit cast clarification for those changes.

## Adoption

Inspection is read-only and exports only schema that can be represented as
normal authored YAML. It never writes opaque catalog source or unmanaged table
syntax into a project's desired state.

Adoption is a deliberate composition of inspection, authored-schema update,
ordinary offline planning, and optional verified fake application. It allows a
team to declare selected existing entities before recording the matching
migration history, without treating the live database as an alternative source
of migration truth. A verified fake application records no migration until the
replayed expected state matches the inspected database through the dialect
drift contract. Blind fake application remains an explicit override.

## Evidence and Limits

Gaman's public guarantees are layered:

```text
unit tests -> implementation invariants
offline fixtures -> parser, replay, diff, clarification, and SQL behavior
online fixtures -> application, inspection, verification, and repair behavior
accepted results -> public compatibility claims
```

The compatibility matrix is therefore descriptive, not aspirational. A green
claim requires accepted evidence. Bounded and unsupported behavior must retain
an explicit explanation in the detailed support document.

Architecture changes should preserve these principles deliberately. New syntax
or broader support must not weaken deterministic replay, clarification, dialect
honesty, or evidence-backed claims.
