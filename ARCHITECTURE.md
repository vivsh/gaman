# Gaman Architecture

Gaman is a schema-first migration system. It models database structure as a
Gaman-owned `Schema`, derives migration operations from schema differences, and
uses dialect modules to render, inspect, and verify database-specific behavior.

```text
schema input          migration planning              live database
-----------           ------------------              -------------
YAML/JSON/Rust  ->    desired Schema
SQL CREATE DDL  ->    normalize
                     prepare(dialect)

migration files ->    replay baseline Schema
                     diff desired vs baseline
                     clarify risky operations
                     write migration

migration graph ->    render SQL plan
migration graph ->    apply SQL + record rows

live database   ->    inspect Schema
replayed Schema ->    verification registry -> drift findings
```

## Core Principles

- `Schema` is the internal source of truth. SQL text is input/output, not the
  primary model.
- Every database-specific decision belongs behind a `DialectProcessor`, parser,
  inspector, renderer, or verification registry.
- Parsing SQL is for loading schema state from `CREATE` statements only. Gaman
  does not parse arbitrary migration scripts into operations.
- Offline diffing and live verification are different contracts. Offline diff
  compares modeled schemas to generate migrations. Live verification compares
  only properties that the selected dialect can inspect accurately.
- Opaque objects are not invisible. Their presence and stable metadata can be
  verified, but body/source text is verified only when a dialect explicitly
  registers that property.
- Backward compatibility is not a design constraint yet. Prefer clear APIs over
  aliases or implicit defaults.

## Crate Boundaries

```text
gaman-core
  Schema model, parser boundary, dialect processors, diff, replay, clarifier,
  migration graph, SQL plan rendering, and offline planner.

gaman
  Native CLI/runtime, file loading, executors, live inspection, verification,
  migration application, and test harnesses.
```

`gaman-core` must remain usable without a live database. Live database work stays
in the native crate and executor layer.

## Schema Lifecycle

```text
load input
  -> parse or deserialize
  -> normalize
  -> prepare(dialect)
  -> diff/render/inspect/verify
```

`normalize` performs dialect-neutral structural cleanup:

- stable map keys and generated names;
- table-owned child attachment;
- inline column references/checks lowered into modeled child metadata;
- schema consistency that does not require dialect-specific aliases.

`prepare(dialect)` performs dialect-specific lifecycle work:

- canonical type aliases;
- dialect validation;
- migration validation where applicable;
- dialect-specific schema constraints.

Parsing does not canonicalize dialect type aliases. Parser output is normalized;
schema loading prepares it with the explicit dialect supplied by the caller.

## Parser Boundary

The `parsers` module is the only SQL parsing boundary.

```text
SQL file
  -> segments::segment_sql(sql, dialect)
  -> sqlparser parses each segment privately
  -> dialect lowerer accepts modeled CREATE statements
  -> Schema
```

Rules:

- Public parser APIs expose only Gaman-owned types.
- `sqlparser` AST, tokenizer, parser errors, and dialect types do not escape the
  `parsers` module.
- `parse_sql(sql, dialect)` requires an explicit `Dialect`; there is no parser
  default dialect.
- Only `CREATE` statements for modeled `EntityKind` values are lowered.
- `ALTER`, `DROP`, DML, transactions, grants, policies, and other statements are
  not schema-loading inputs.
- Unsupported parsed statements return structured Gaman parser errors.

Modeled parser targets are:

- `Table`, including columns, primary keys, unique/check constraints, generated
  columns, and foreign keys;
- `Index`, including unique and partial indexes where modeled;
- `Trigger`;
- `Function` where the dialect can lower it;
- `View`;
- PostgreSQL `Enum` and `Extension`.

SQLite lowers its supported subset. MySQL currently has segmentation and dialect
selection only; schema lowering/rendering/validation remain unsupported.

## SQL Segmentation

`parsers::segments` splits SQL source into statement slices before any AST
parsing. It is a boundary detector, not a parser.

`SqlSegment` carries:

- ordinal;
- raw SQL slice;
- half-open byte offsets into the original source;
- source line/column range;
- optional lexical statement classification.

Segmentation preserves leading whitespace and comments as part of the following
statement. Terminators and MySQL `DELIMITER` directives are excluded from the
returned slice. The invariant is:

```text
segment.sql == &source[segment.start_byte..segment.end_byte]
```

The segmenter is intentionally independent of downstream parsing. It tracks
strings, quoted identifiers, comments, dollar quotes, bracket depth, SQLite
trigger bodies, PostgreSQL dollar bodies, and MySQL custom delimiters so parser
errors can be reported against the original source segment.

Classification is lexical and conservative:

```text
DDL(EntityKind) | DML(Select | Insert | Update | Delete) | None
```

It identifies broad-stroke top-level statement intent and object name when it is
confident. It does not validate SQL syntax.

## Migration Model

A migration is an append-only file containing:

- migration id;
- dependencies;
- operations;
- atomic flag;
- optional description/metadata.

The migration graph determines ordering and dependency relationships. Replay
uses migration operations to reconstruct schema state without connecting to a
live database.

`ReplayEngine` owns shared replay behavior:

- replay a graph/order into `Schema`;
- apply one migration with contextual replay errors;
- expose source metadata such as last migration per namespace and entity source
  mapping for dependency calculation.

## Offline Planning

The offline planner creates new migrations from desired schema state.

```text
current migration graph
  -> replay baseline Schema
  -> load desired Schema
  -> diff baseline vs desired
  -> Clarifier.process(raw operations)
  -> resolved operations or pending clarifications
  -> write migration file
```

The diff engine remains generic over schemas. It does not know about live
inspection policy or verification registries.

## Clarifier

The `clarifier` module handles operation-risk clarification. It turns ambiguous
or risky raw operations into either resolved operations or pending
clarifications.

It owns the interaction model for:

- rename vs drop/create choices;
- destructive changes;
- unknown or dialect-sensitive types;
- user decisions and answers.

Clarification ids remain stable fixture-friendly strings such as
`rename_col:...`, `notnull_add:...`, and `unknown_type:...`.

## SQL Rendering And Plans

SQL rendering is dialect-owned. `SqlPlanRenderer` renders migration operations
for a selected dialect without connecting to a database.

```text
migration graph/order
  -> replay baseline as needed
  -> render operation SQL using DialectProcessor
  -> SQL plan
```

Rendering must not duplicate parsing or inspection logic. Dialect processors own
SQL syntax, type names, capability checks, and migration validation.

## Native Runtime

The native crate adds live database behavior:

```text
Config + Environment
  -> Executor
  -> Migrator
  -> apply / inspect / verify
```

Executors own database I/O:

- connection handling;
- migration lock behavior where supported;
- execution of rendered SQL;
- migration tracking table reads/writes;
- catalog queries used by inspection.

The tracking table records applied migrations. A migration is recorded only
after its operations succeed. Atomic migrations run inside one transaction where
the dialect and executor support it.

## Inspection

The `inspection` module owns live reflection:

```text
live database catalog -> reflected Schema -> prepare(dialect)
```

`inspect_db` uses inspection directly. Its contract is onboarding fidelity: emit
the most useful Gaman schema the dialect can recover from the database.

Inspection should:

- preserve useful catalog metadata;
- preserve opaque source text when the database exposes it;
- canonicalize only when it improves stable output without losing meaning;
- avoid inventing fake semantics for unsupported or lossy catalog facts;
- expose diagnostics internally for lossy reflection cases.

PostgreSQL inspection canonicalizes owned sequence-backed integer columns into
serial-like types only when catalog dependencies prove sequence ownership:

```text
integer + owned nextval  -> serial
bigint + owned nextval   -> bigserial
smallint + owned nextval -> smallserial
```

Foreign key `on_delete` is modeled, parsed, rendered, inspected, and verified
when present.

## Verification

The `verification` module owns drift detection for live databases.

```text
replayed Schema
live inspected Schema
  -> scope to owned/requested objects
  -> dialect verification registry
  -> property comparators
  -> VerificationReport
```

`verify_db` does not compare full schemas with `PartialEq`. It uses a
static dialect-specific registry of properties that can be inspected accurately
and deterministically.

A `VerificationReport` contains:

- `DriftFinding` values with operation, entity kind, entity identity, property,
  expected value, actual value, and optional note;
- repair-oriented `Operation` values for existing callers.

CLI output is actionable:

```text
drift: alter_column preferences.posts_per_page
  default: expected <none>, found 10
```

Unregistered properties are outside the verification contract and must not
produce drift.

## Verified Properties

PostgreSQL registry:

| Entity | Verified properties |
|---|---|
| Table | name, schema, owned presence |
| Column | name, type, nullable, default, primary_key, references, check, generated |
| Primary key | name, ordered columns |
| Foreign key | name, source columns, target table, target columns, on_delete |
| Index | name, ordered columns, unique, predicate where stable |
| Constraint | kind and stable definition |
| Trigger | name, timing, events, scope, function_name, language |
| Function | name, schema, arguments, returns, language, volatility, security_definer |
| View | name, schema |
| Enum | name, schema, ordered values |
| Extension | name, schema, version |

SQLite registry:

| Entity | Verified properties |
|---|---|
| Table | name, owned presence |
| Column | name, type/affinity, nullable, default, primary_key, generated |
| Primary key | ordered columns |
| Foreign key | name, source columns, target table, target columns, on_delete |
| Index | name, ordered columns, unique, predicate where stable |
| Constraint | stable definition |
| Trigger | name, timing, events, scope, language |
| View | name |
| Function, Enum, Extension | none |

MySQL verification is unsupported until MySQL schema lowering, rendering,
inspection, and execution exist.

## Opaque Objects

Opaque objects include functions, views, triggers, and raw SQL-like bodies whose
semantic equivalence cannot be reliably proven from reflected text.

Rules:

- Missing owned opaque objects produce drift.
- Stable registered metadata changes produce drift.
- Body/source-only changes do not produce live drift unless the dialect registry
  explicitly verifies that body/source property.
- Live-only opaque objects outside replay ownership are ignored.
- `inspect_db` may export source text for onboarding even when `verify_db`
  ignores it.

Offline diff may compare opaque source text for migration generation. Live
verification uses the registry contract instead.

## Dialect Boundary

A dialect owns:

- SQL rendering;
- type normalization and canonicalization;
- schema validation;
- migration validation;
- parser lowering behavior;
- inspection interpretation;
- verification registry and comparators;
- capability errors for unsupported features.

PostgreSQL is the primary dialect. SQLite supports a smaller feature set and may
need table rebuilds for some changes. MySQL is currently a dialect-selection and
segmentation stub, not a schema lifecycle implementation.

## File Loading And CLI Inputs

Schema loading is dialect-explicit:

```text
from_yaml_str(content, dialect)
from_json_str(content, dialect)
from_sql_str(content, dialect)
load_schema_file(path, dialect)
```

A schema path may be a file or directory. SQL/YAML/JSON inputs all prepare with
the explicit dialect supplied by configuration or caller.

The CLI loads `.env` files only when requested with `--env`. Environment
variables remain configuration overrides, not hidden unconditional input.

## Testing And Evidence

The test suite is layered by lifecycle boundary:

- core unit tests for schema, diff, replay, parser, segmenter, and dialect code;
- parser YAML fixtures for accepted SQL-to-`Schema` lowering evidence;
- offline fixtures for migration planning, clarification, and SQL rendering;
- online fixtures for live apply, inspect, and verify behavior;
- support matrix and results files for recorded feature evidence.

Important result files include:

- `results/parser-results.yaml`;
- `results/online-results.yaml`.

Parser fixtures track what SQL is successfully lowered into Gaman entities.
Online fixtures track what live dialect behavior is applied, inspected, and
verified.

## Current Limits

- Parser loading accepts modeled `CREATE` statements only.
- MySQL schema lowering, rendering, validation, inspection, and verification are
  not implemented.
- Foreign key `on_update`, match type, deferrability, and validation status are
  not modeled yet.
- Function bodies, view definitions, trigger bodies, and trigger `WHEN` clauses
  are not live verification inputs unless promoted into a dialect registry.
- Primary-key mutation generation is limited and may require manual/raw SQL for
  some cases.
- Exact round-trip equality between input SQL and `inspect_db` output is not a
  goal. Canonical schema equivalence under the verification registry is the goal
  for drift detection.
