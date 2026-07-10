# Gaman Architecture

Gaman is a schema-first migration system. It turns authored schema into a
Gaman-owned `Schema`, derives migration operations from schema differences, and
checks live databases against replayed migration state.

```text
schema input     -> input Schema     -> normalize + prepare(dialect)
migration files  -> replayed Schema  -> defensive prepare(dialect)
live database    -> inspected Schema -> normalize_inspected_schema(dialect)

input Schema    + replayed Schema  -> diff  -> migration operations
replayed Schema + inspected Schema -> drift -> verification findings

migration graph -> render SQL -> apply -> record migration rows
```

The central design rule is explicit guarantees. Gaman does not need to model
every database feature to be useful, but it must be honest about what it models,
what it treats as opaque, and what `verify` can or cannot detect.

## Core Guarantees

Gaman has two representation classes and two independent guarantee dimensions.

```text
granular entities:
  parsed into structured Gaman fields
  diffed structurally at the migration granularity supported for that kind
  verified through any dialect-registered stable property comparators

opaque entities:
  known EntityKind + name + raw SQL/source where available
  diffed as whole objects
  verified by presence only
```

Granular guarantees are intentionally narrow:

- tables and table structure;
- columns;
- primary keys where modeled;
- foreign keys where modeled;
- enum/type label changes;
- supported type/null/default/generated/check details where registered by the
  dialect.

Everything else may be parsed internally on a best-effort basis, but its public
migration guarantee is coarse: create, drop, or replace. A modeled non-table
entity may still have stable properties registered for live drift even when its
repair operation remains coarse. Its opaque form is always presence-only.

Opaque does not mean unknown or ignored. It means Gaman knows the object kind and
identity, but does not promise property-level migrations or drift detection for
its body/source.

## Crate Boundaries

```text
gaman-core
  Schema model, parser boundary, SQL segmentation, dialect processors, diff,
  replay, clarifier, drift contracts, repair planning, migration graph, SQL
  plan rendering, and offline planner.

gaman
  CLI/runtime, config, schema file loading, executors, live catalog inspection,
  migration application, tracking table I/O, and online test harnesses.
```

`gaman-core` is database-I/O-free. Live database work belongs in the native crate
and executor layer.

## EntityKind Boundary

`EntityKind` is the closed set of schema objects Gaman recognizes. Input SQL must
classify to a known `EntityKind` and have an object name. SQL outside this set is
rejected immediately.

Allowed known kinds are currently:

- table;
- column;
- primary key / constraint;
- foreign key;
- index;
- trigger;
- function;
- view;
- enum/type;
- extension.

Objects such as policies, event triggers, grants, procedures, sequences, rules,
and materialized views are not accepted unless they are deliberately promoted
into `EntityKind` and given lifecycle behavior.

## Schema Sources

Gaman has three schema sources.

```text
Input Schema
  authored YAML/JSON/SQL/Rust schema
  normalized and prepared before migration diff

Replayed Schema
  migration files replayed into Schema
  expected to be normalized already
  prepared defensively before drift

Inspected Schema
  live database catalog reflected into Schema
  normalized as inspected state before drift
```

Input schemas are the desired state. Replayed schemas are the current migration
history state. Inspected schemas are what the live database actually contains.

## YAML, JSON, Rust, And SQL Input

YAML, JSON, and Rust builder input are structured Gaman schema inputs. They
should contain only modeled structures. Raw/opaque metadata is reserved for SQL
input lowering, inspection, and migration history replay.

SQL is the escape hatch for dialect-specific definitions.

```text
SQL input
  -> segment_sql(sql, dialect)
  -> lexical classification: EntityKind + name
  -> attempt full parser/lowering
  -> modeled entity if lowering succeeds
  -> raw opaque entity if classification succeeds but lowering fails
  -> error if outside EntityKind or missing name
```

A SQL statement is therefore always either fully modeled or raw opaque. There is
no partial entity state.

Raw SQL created from input is untrusted until the clarifier accepts it. Raw SQL
created by inspection or replay is trusted because the database or migration
history has already accepted it. Authored YAML, JSON, and Rust builders cannot
set raw source, trust, fingerprints, or unmanaged table option metadata.

## Normalization And Preparation

`normalize` performs dialect-neutral structural cleanup:

- stable names and map keys;
- table-owned child attachment;
- inline column references/checks lowered into modeled table metadata;
- schema consistency that does not require dialect-specific aliases.

`prepare(dialect)` performs dialect-specific lifecycle work:

- type normalization/canonicalization;
- dialect validation;
- migration validation;
- dialect-specific capability checks.

Parser output does not choose a default dialect. Every schema load path supplies
an explicit `Dialect`.

## Parser Boundary

The `parsers` module is the only SQL parsing boundary.

```text
SQL file
  -> segments::segment_sql(sql, dialect)
  -> per-segment classification
  -> private sqlparser parse when useful
  -> dialect lowerer
  -> Schema
```

Rules:

- public parser APIs expose only Gaman-owned types;
- `sqlparser` AST/tokenizer/parser types do not escape `parsers`;
- schema loading accepts `CREATE` statements for known `EntityKind` values only;
- `ALTER`, `DROP`, DML, transactions, grants, policies, event triggers, and
  unclassified statements are not schema input;
- if a known `CREATE` cannot be lowered, SQL input may preserve it as an
  untrusted raw opaque entity.

## SQL Segmentation

`parsers::segments` splits SQL source before AST parsing. It is a boundary
detector, not a schema lowerer.

`SqlSegment` carries:

- ordinal;
- raw SQL slice;
- half-open byte offsets into the original source;
- source line/column range;
- optional lexical statement classification.

The segment invariant is:

```text
segment.sql == &source[segment.start_byte..segment.end_byte]
```

Segmentation preserves leading whitespace and comments with the following
statement. Terminators and MySQL `DELIMITER` directives are excluded from
returned SQL slices.

Classification is conservative and lexical:

```text
DDL(EntityKind + object name) | DML(Select | Insert | Update | Delete) | None
```

It identifies broad intent and object identity. It does not validate SQL syntax.

## Schema Model

`Schema` remains the central model. Entity structs such as `Table`, `Column`,
`Index`, `FunctionDef`, `TriggerDef`, and `ViewDef` remain the working shapes for
diff, replay, rendering, inspection, and drift.

Internally, entity structs may carry skipped metadata for raw/opaque lifecycle
state. That metadata is not user schema syntax and must not be deserializable
from YAML/JSON input.

The invariant is:

```text
modeled entity:
  structured fields are authoritative

opaque entity:
  kind + name are authoritative
  raw SQL/source may be available for create/replace
  structured fields are not compared semantically
```

## Diff

Diff compares input schema against replayed schema to produce migrations.

```text
input Schema + replayed Schema -> DiffEngine -> operations
```

Granular entities use structural diff. For example, column add/drop/rename,
type/null/default changes, and enum label changes can produce fine-grained
operations.

Opaque entities use whole-object diff:

- missing opaque object -> create raw object;
- removed opaque object -> drop raw object;
- raw-vs-raw with equal canonical token hash -> no operation;
- raw-vs-raw with different canonical token hash -> clarification before
  replace/drop-create;
- raw-vs-modeled -> clarification before choosing a coarse operation.

The token fingerprint is a versioned SHA-256 digest of length-delimited lexical
tokens. It ignores whitespace and comments outside protected regions, preserves
literal and quoted content, and does not attempt semantic SQL equivalence.

Unparsed table modifiers are tracked separately from granular table/column
structure. Table/column diff remains granular, but changes to unparsed table
modifiers require clarification to ignore because Gaman cannot safely generate a
fine-grained operation for them yet.

## Clarifier

The `clarifier` module handles choices that are unsafe or ambiguous.

It owns interaction for:

- rename vs drop/create choices;
- destructive changes;
- unknown or dialect-sensitive types;
- untrusted raw opaque entities;
- opaque definition changes that would require coarse replacement;
- unparsed table modifier changes that Gaman cannot migrate granularly.

Accepted raw entities become trusted in migration history. Rejected raw entities
block migration generation.

## Migration Model And Replay

A migration is an append-only file containing:

- migration id;
- dependencies;
- operations;
- atomic flag;
- optional description/metadata.

`ReplayEngine` reconstructs schema state from migration operations without a
live database. It also provides source metadata used for dependency calculation.

Replay treats accepted raw opaque operations as trusted because they already
entered migration history.

## SQL Rendering

SQL rendering is dialect-owned. Dialect processors render operations into SQL and
validate capability limits.

Granular operations render structured SQL. Opaque operations render raw SQL for
create/replace when raw source is available. Drops for opaque objects are
dialect-specific and supported only when Gaman can safely identify the object.

`Operation::Statement` remains for unmanaged arbitrary SQL and is not the primary
representation for known raw `EntityKind` objects.

## Native Runtime

The native crate adds live database behavior.

```text
Config + Environment
  -> Executor
  -> Migrator
  -> apply / inspect / verify / repair
```

Executors own:

- connections;
- SQL execution;
- migration locks;
- tracking table reads/writes;
- catalog queries used by inspection.

The tracking table records applied migrations only after successful application.
Atomic migrations run inside a transaction where supported.

## Inspection

Inspection reflects a live database into `Schema`.

```text
live catalog -> inspected Schema
```

Inspection is onboarding-oriented and should be as faithful as possible. For
known `EntityKind` objects, inspection should not fail merely because the object
contains unmodeled syntax.

Inspection rules:

- if the object is fully recoverable as modeled schema, emit modeled fields;
- if only kind, name, and raw/source are recoverable, emit trusted opaque entity;
- preserve source text where the database exposes it;
- avoid inventing fake structured semantics;
- never silently coerce advanced opaque details into modeled fields.

For example, an expression index should not put `lower(email)` into `columns`.
It should be represented as a known opaque index with name and raw source where
available.

PostgreSQL inspection may canonicalize owned sequence-backed integer columns into
serial-like types only when catalog dependency proves ownership:

```text
integer + owned nextval  -> serial
bigint + owned nextval   -> bigserial
smallint + owned nextval -> smallserial
```

## Drift And Verify

Drift compares replayed schema against inspected schema.

```text
replayed Schema + inspected Schema
  -> dialect drift registry
  -> VerificationReport
```

Drift is semantic and property-based. It does not compare full schemas with
`PartialEq`. Each dialect owns a registry of properties that can be inspected
accurately and deterministically.

Granular drift detects registered property changes, such as column type,
nullability, default, foreign-key target, or enum values.

Opaque drift is intentionally limited:

- expected opaque object present -> no drift;
- expected opaque object missing -> drift;
- live-only opaque object outside ownership -> ignored;
- opaque raw/body/source changed in place -> not detected;
- opaque raw/body/source text is never a live drift input unless explicitly
  promoted into a registered property later.

This is an explicit guarantee boundary. `verify` must not claim source/body drift
for opaque objects because database reflection may rewrite SQL and because
semantic equivalence is dialect-specific.

## Repair

Repair is local drift recovery, not migration authoring.

```text
VerificationReport
  -> repair plan
  -> one-off SQL
  -> optional apply
  -> verify again
```

Repair may fix granular drift where operations are safe and renderable. For
opaque objects, repair can only handle missing owned objects when trusted raw
source is available. It cannot repair changed opaque definitions because drift
does not assert such changes.

Repair never writes migration files and never records tracking-table rows.

## Drift Contract

The drift registry is the user-facing verification contract. Unregistered
properties are ignored by `verify`.

PostgreSQL currently verifies stable modeled properties such as:

| Entity | Verified properties |
|---|---|
| Table | name, schema, owned presence |
| Column | name, type, nullable, default, primary_key, references, check, generated |
| Primary key | name, ordered columns |
| Foreign key | name, source columns, target table, target columns, on_delete, on_update |
| Enum | name, schema, ordered values |
| Opaque entities | owned presence only |

SQLite verifies its stable modeled subset:

| Entity | Verified properties |
|---|---|
| Table | name, owned presence |
| Column | name, type/affinity, nullable, default, primary_key, generated |
| Primary key | ordered columns |
| Foreign key | name, source columns, target table, target columns, on_delete, on_update |
| Opaque entities | owned presence only |

Modeled indexes, functions, triggers, views, constraints, and extensions may
have stable registered properties even though their migration repair is coarse.
When any of these entities is represented as opaque raw SQL, verification is
presence-only regardless of which properties its modeled form registers.

## Dialect Boundary

A dialect owns:

- SQL rendering;
- type normalization and canonicalization;
- schema validation;
- migration validation;
- parser lowering;
- inspection interpretation;
- drift registry and comparators;
- capability errors for unsupported features.

PostgreSQL is the primary dialect. SQLite supports a smaller lifecycle and may
need table rebuilds for some changes. MySQL currently has segmentation and
dialect selection, but not schema lowering, rendering, inspection, or drift.

## CLI Lifecycle

The CLI command model follows the schema lifecycle:

```text
gaman inspect   # database -> schema
gaman make      # schema -> migration
gaman status    # migration application status
gaman show      # migration artifact inspection
gaman sql       # migration -> SQL
gaman apply     # apply pending migrations
gaman verify    # replayed schema vs database
gaman repair    # one-off drift repair
gaman config    # resolved configuration
```

`.env` files are loaded only when requested with `--env`. Environment variables
are configuration overrides, not hidden unconditional input.

## Testing And Evidence

Testing follows lifecycle boundaries:

- core unit tests for schema, diff, replay, parser, segmenter, dialects, and
  drift;
- parser YAML fixtures for SQL segmentation/classification/lowering evidence;
- offline fixtures for planning, clarification, diff, and rendering;
- online fixtures for live apply, inspect, verify, and repair behavior;
- generated evidence docs and result files for supported behavior claims.

Important result files include:

- `results/parser-results.yaml`;
- `results/online-results.yaml`.

README support claims should be backed by fixture evidence, not by aspirational
implementation notes.

## Current Limits

- YAML/JSON input is structured only; raw/opaque metadata is not part of normal
  authored schema syntax.
- SQL input accepts only `CREATE` statements for known `EntityKind` values.
- Opaque live drift detects presence only, not definition changes.
- Opaque migration changes are coarse and require clarification before
  replacement.
- MySQL schema lifecycle support is not implemented.
- Foreign-key match type, deferrability, and validation status are not modeled.
- Function bodies, view definitions, trigger bodies, advanced index semantics,
  and table modifiers are not granular drift inputs unless promoted into modeled
  registry properties later.
- Exact round-trip equality between input SQL and `inspect` output is not a
  goal. Honest guarantees and deterministic supported behavior are the goal.
