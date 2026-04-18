# CLI and Config Reference

This is the command reference that used to live in the README. The README stays focused on the workflow; this file is for the details you look up after you already know how Gaman fits together.

Global flags come before the subcommand:

- `-m <dir>` overrides the migrations directory.
- `-s <file>` overrides the schema file or schema directory.
- `-d <url>` overrides `DATABASE_URL`.

## Everyday commands

### `make_migration [name]`

Diff the declared schema against replayed migration history and write the next migration file.

- `--empty` creates an empty migration shell.
- `--merge` creates a merge migration when there are multiple heads.
- `--check` exits non-zero if changes exist, without writing a file.
- `--dry-run` prints what would be generated.

```bash
gaman make_migration add_posts
gaman make_migration --check
gaman make_migration --dry-run
```

### `migrate`

Apply pending migrations in topological order. Each migration runs in its own transaction unless `atomic: false`.

- `--target <id>` migrates forward or backward to a specific migration.
- `--fake` records migrations as applied without running DDL.
- `--plan` prints the plan and exits.
- `--check` exits non-zero if anything is pending.

```bash
gaman migrate
gaman migrate --target 0003_add_posts
gaman migrate --fake 0001_initial
```

### `sql_migrate [id]`

Print SQL for one migration or for the whole plan. This command does not need a database connection.

- `--backwards` prints rollback SQL instead of forward SQL.

## Inspection commands

### `verify_db`

Compare the live database against replayed migration state and report drift. Today this checks tables and columns; it does not validate views or functions.

```bash
gaman verify_db
gaman verify_db --schema myschema
```

### `inspect_db`

Introspect a live database and emit `schema.yaml`. This is mainly for bootstrapping an existing project.

```bash
gaman inspect_db > schema.yaml
gaman inspect_db --schema myschema --table users
```

### `show_migrations`

List all known migrations with applied and pending markers.

### `config`

Print the resolved configuration and exit.

## Environment variables

Gaman reads three main environment variables:

- `DATABASE_URL`: PostgreSQL connection string. Required for commands that talk to the database.
- `MIGRATIONS_DIR`: directory containing migration YAML files. Defaults to `migrations`.
- `SCHEMA_FILE`: path to the schema input. Defaults to `schema.yaml`. This can point to a `.yaml`, `.sql`, or a directory.

CLI flags win over environment variables when both are provided.
