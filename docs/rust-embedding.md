# Embedding Gaman In Rust

Gaman's Rust API exposes the same lifecycle used by the CLI without requiring
CLI parsing, terminal output, prompts, filesystem storage, or a specific
database driver.

## API Layers

- `OfflinePlanner` performs deterministic schema replay and migration generation.
- `MigrationEngine` provides direct storage, tracking, replay, rendering, and application control.
- `MigrationRunner` provides the uniform typed command interface used by hosts.
- `MigrationStore`, `TrackingStore`, `Executor`, and `SchemaInspector` are host boundaries.
- `NativeRunnerFactory` wires native configuration, storage, SQLx execution, and tracking.

Use `MigrationEngine` for fine-grained lifecycle control. Use
`MigrationRunner::run_command` for the same command semantics as the CLI, WASM,
or a future FFI host.

## Features

Select only the required native dialect:

```toml
[dependencies]
gaman = { version = "0.3", default-features = false, features = ["postgres"] }
```

Native dialect features are `postgres`, `sqlite`, `mysql`, and `mariadb`.
Offline consumers can enable `offline` without a live database executor.

## Structured Schema Input

Rust builders produce the same prepared schema model used by SQL, YAML, and
JSON ingestion. Builders expose modeled fields only; authored raw opaque
metadata remains a SQL-input capability.

```rust
use gaman::core::Dialect;
use gaman::schema::{SchemaBuilder, TableBuilder};

let schema = SchemaBuilder::new(Dialect::Postgres)
    .table_def(
        TableBuilder::new("users")
            .column("id", "bigserial", |column| column.primary_key())
            .column("email", "text", |column| column.not_null())
            .build(),
    )
    .build()?;
# Ok::<(), gaman::schema::SchemaValidationError>(())
```

`IntoTable` remains a plain integration trait for model crates that provide
their own derives or schema conventions.

## Direct Engine Use

`MigrationEngine<M, T, E>` is generic over application-owned adapters:

- `M: MigrationStore` loads and saves migration definitions.
- `T: TrackingStore` reads and updates applied migration IDs.
- `E: Executor` prepares or executes SQL and owns transaction and lock behavior.

The engine does not read paths, environment variables, process arguments, or
terminal input. Its methods expose migration generation, status, SQL planning,
application, replay, and repair primitives directly.

## Uniform Runner Use

`MigrationRunner<M, T, E>` wraps one engine. When `E` also implements
`SchemaInspector`, the runner supports inspection, verification, and repair.

```text
resolved host input
  -> runner::Command
  -> MigrationRunner::run_command
  -> CommandResult | CommandError
```

Clarification is returned as `CommandError::NeedsInput`. A host collects
`Decision` values, creates a retry with `Command::with_decisions`, and retries
the borrowed command. The original resolved command remains available to the
host.
The runner never prompts or prints.

Commands, results, diagnostics, and clarification payloads are serializable
Gaman-owned values. `CommandEnvelope` and `CommandResponse` carry
`COMMAND_PROTOCOL_VERSION`, allowing WASM and future FFI hosts to reject an
incompatible request before lifecycle work begins. See
[Command Protocol](command-protocol.md) for the transport contract.

Protocol version 2 and the runner adapter traits are the stable host boundary.
The complete schema/entity model and direct low-level engine conveniences remain
pre-0.5 APIs and may still be refined.

Every runner command uses one validated `MigrationCatalog` snapshot. Direct
engine calls remain intentionally independent and observe fresh migration
storage.

The snapshot belongs to a command-scoped engine view; `MigrationEngine` does not
retain it after the command. Independent commands therefore always reload the
backing migration store.

## Native Runner Factory

`NativeRunnerFactory` constructs a native runner without opening a database
connection. The lazy executor connects only when a command needs tracking, SQL
preparation or execution, inspection, verification, or repair.

Use:

- `NativeRunnerFactory::from_directory(config)` for filesystem history;
- `NativeRunnerFactory::from_embedded(config, migrations)` for compiled history
  with generated migrations written to the matching directory;
- `NativeRunnerFactory::from_store(config, store)` for a caller-owned store.

## Embedded Migrations

`EmbeddedMigrations` remains a plain static tree so framework macros can compose
migrations across crates:

```rust
use gaman::EmbeddedMigrations;

static MIGRATIONS: EmbeddedMigrations = EmbeddedMigrations {
    files: &[],
    dir: "migrations",
    children: &[("auth", &auth::MIGRATIONS)],
};
```

Child IDs and local dependencies are namespace-prefixed, such as
`auth/0001_initial`. Duplicate qualified IDs are rejected. Embedded history is
read-only; generated migrations are persisted through the matching
`DirectoryMigrationStore`. A directory mismatch is rejected before writes.

## Custom Adapters

Implement `MigrationStore` for in-memory, virtual, embedded, or
application-owned history. Implement `TrackingStore` when applied state does
not live in Gaman's database table. Implement `Executor` and `SchemaInspector`
to integrate a custom runtime or database transport.

These traits use structured Gaman errors and asynchronous boxed futures. They
do not depend on `argh` or native CLI types.
