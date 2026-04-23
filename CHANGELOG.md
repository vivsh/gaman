# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.17] - 2026-04-23

### Added

- Auto-generated migration names — `--name` is now optional for `make_migrations`. When omitted, the name is derived from the operations: single entity → entity name (e.g. `users`), two entities → `entity_a_entity_b`, three or more or no named entities → `auto_YYYYMMDD_HHMM` timestamp (same convention as Django)
- `--name` remains required for `--empty` and `--merge`

## [0.3.16] - 2026-04-23

## [0.3.16] - 2026-04-23

### Added

- `Migration::get_entities()` — returns the set of `(EntityKind, name)` pairs touched by a migration's operations; used internally for dependency tracking
- Automatic cross-namespace dependency injection in `make_migration` and `make_empty_migration` — previously, cross-crate migrations had to declare `dependencies` manually or risk applying in the wrong order when a table in one namespace referenced a table in another. Now gaman scans the new operations for referenced entities, resolves which namespace last touched each one, and injects those namespace heads as `dependencies` automatically (Django-style app dependencies)

### Fixed

- `SchemaBuilder::build()` now calls `Schema::normalize()` before returning, so foreign keys declared via `ColumnBuilder::references()` or `#[derive(IntoTable)]` are correctly promoted to `table.foreign_keys` and visible to the diff engine — previously these FKs were silently dropped during diffing, causing `make_migration` to never emit `AddForeignKey` operations for programmatically-built schemas

## [0.3.15] - 2026-04-23

### Added

- `CHANGELOG.md` — full history from 0.1.0
- `release.sh` — automated patch/minor/major release script (bump, commit, tag, push, publish)

## [0.3.14] - 2026-04-23

### Added

- Multi-crate migration composition via `EmbeddedMigrations.children` — assemble a static tree of embedded migration sets at compile time; child IDs are automatically namespaced (e.g. `auth/0001_init`)
- `Schema::merge(other)` — merge two schemas programmatically; duplicate tables are an error, everything else is last-writer-wins
- `SchemaBuilder::load_file(path)` and `SchemaBuilder::load_dir(path)` — load a schema from inside a `with_schema` closure without needing to call `Schema::from_file` directly
- `IntoSchema` trait — allows `with_schema` closures to return either `Schema` (infallible) or `Result<Schema, SchemaLoadError>` (fallible), no unwrap needed
- `SchemaLoadError::DuplicateTable` variant for programmatic duplicate detection
- `TlsMode` enum (currently `NoTls`) in `Config` struct

### Changed

- `with_schema` now returns `Result<Self, EngineError>` instead of `Self` — propagates schema load errors before any action runs
- `TlsMode` moved from engine to `Config`; set TLS via `Config { tls: TlsMode::NoTls, .. }` instead of a builder method
- `graphs::next_number()` skips namespaced IDs (containing `/`) so child migration IDs don't inflate the parent counter

### Removed

- `MigrationEngine::with_tls` — use `Config.tls` instead
- `MigrationEngine::with_database_url` — use `Config.database_url` instead
- `MigrationEngine::with_schema_file` — use `with_schema(|s| s.load_file("schema.yaml"))` instead

## [0.3.13] - 2026-04-22

### Changed

- Renamed `include_migrations!` macro to `embedded_migrations!`
- `EmbeddedMigrations` is now a plain struct with `files`, `dir`, and `children` fields, usable as a `static` value and composable across crates

## [0.3.12] - 2026-04-22

### Fixed

- `make_migration` now reads from and writes to `config.migrations_dir` on disk; it no longer touches the embedded slice

## [0.3.11] - 2026-04-22

### Added

- `MigrationEngine` embedding API — construct an engine with `MigrationEngine::new(config, &MIGRATIONS)` and call action methods directly
- `migrate()` now returns `usize` (number of migrations applied)

### Removed

- `Invoke` mode removed; the CLI surface is now accessed via `handle_args()`

## [0.3.10] - 2026-04-18

### Changed

- Integration test harness improvements — tests run faster and produce cleaner output

## [0.3.9] - 2026-04-18

### Fixed

- SQL DDL documentation corrections in README

## [0.3.8] - 2026-04-18

### Fixed

- Minor README and example updates

## [0.3.5] - 2026-04-17

### Added

- SQL DDL schema source — define your target schema using raw `CREATE TABLE` SQL via `sqlparser` integration; pass `.sql` files or inline DDL to `with_schema`

## [0.3.0] - 2026-04-17

### Added

- Embedded mode — ship migrations inside your binary with `embedded_migrations!`
- `MigrationEngine` struct as the primary embedding API, replacing the previous CLI-only dispatch model
- Example binaries: `embedded_migrate`, `embedded_structs`, `embedded_yaml`

### Changed

- Major dispatch refactor; embedded and CLI paths now share the same engine core

## [0.1.9] - 2026-04-13

### Fixed

- README structure improvements

## [0.1.8] - 2026-04-13

### Added

- Concurrent index support (`CREATE INDEX CONCURRENTLY`)
- PostgreSQL extensions (`CREATE EXTENSION`)
- Enum types (`CREATE TYPE … AS ENUM`)
- Atomic migration flag — migrations are wrapped in a transaction and marked applied only on success

## [0.1.7] - 2026-04-13

### Fixed

- Diff operation ordering bugs that could generate invalid SQL
- Comprehensive diff ordering tests added

## [0.1.6] - 2026-04-13

### Added

- Atomicity guarantees — each migration runs in a single transaction
- Trigger and function operation ordering

### Fixed

- Drop ordering — dependent objects are dropped before the objects they depend on

## [0.1.5] - 2026-04-13

### Added

- Disambiguator — detects ambiguous diff operations (e.g. rename vs drop+add) and asks the user to resolve them
- Prompter — interactive terminal UI for disambiguation

### Fixed

- Self-referential foreign key deferred ordering

## [0.1.4] - 2026-04-13

### Changed

- Migrations folder is no longer tracked in version control by default (`.gitignore` entry added to generated folder)

## [0.1.3] - 2026-04-12

### Fixed

- `inspect_db` now uses bare table names (without schema prefix) as map keys
- Removed unused test helpers

## [0.1.2] - 2026-04-12

### Added

- Schema drift detection — `verify_db` subcommand compares replayed migration state against the live database and reports differences
- Comprehensive replay determinism tests (189 passing)

## [0.1.1] - 2026-04-12

### Fixed

- All Clippy warnings resolved

## [0.1.0] - 2026-04-12

### Added

- Initial release — deterministic, offline-first PostgreSQL migration engine
- Migration generation from schema diff (no database access required)
- YAML migration file format
- `make_migration`, `migrate`, `fake_migrate`, `show_migrations`, `inspect_db` commands
- DAG-based migration graph with dependency tracking

[unreleased]: https://github.com/vivsh/gaman/compare/v0.3.17...HEAD
[0.3.17]: https://github.com/vivsh/gaman/compare/v0.3.16...v0.3.17
[0.3.16]: https://github.com/vivsh/gaman/compare/v0.3.15...v0.3.16
[0.3.15]: https://github.com/vivsh/gaman/compare/v0.3.14...v0.3.15
[0.3.14]: https://github.com/vivsh/gaman/compare/v0.3.13...v0.3.14
[0.3.13]: https://github.com/vivsh/gaman/compare/v0.3.12...v0.3.13
[0.3.12]: https://github.com/vivsh/gaman/compare/v0.3.11...v0.3.12
[0.3.11]: https://github.com/vivsh/gaman/compare/v0.3.10...v0.3.11
[0.3.10]: https://github.com/vivsh/gaman/compare/v0.3.9...v0.3.10
[0.3.9]: https://github.com/vivsh/gaman/compare/v0.3.8...v0.3.9
[0.3.8]: https://github.com/vivsh/gaman/compare/v0.3.5...v0.3.8
[0.3.5]: https://github.com/vivsh/gaman/compare/v0.3.0...v0.3.5
[0.3.0]: https://github.com/vivsh/gaman/compare/v0.1.9...v0.3.0
[0.1.9]: https://github.com/vivsh/gaman/compare/v0.1.8...v0.1.9
[0.1.8]: https://github.com/vivsh/gaman/compare/v0.1.7...v0.1.8
[0.1.7]: https://github.com/vivsh/gaman/compare/v0.1.6...v0.1.7
[0.1.6]: https://github.com/vivsh/gaman/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/vivsh/gaman/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/vivsh/gaman/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/vivsh/gaman/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/vivsh/gaman/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/vivsh/gaman/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/vivsh/gaman/releases/tag/v0.1.0
