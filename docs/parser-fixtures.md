# Parser Fixtures

Parser fixtures track SQL statements that Gaman can load into schema state. They
are independent from offline migration planning and call only the public parser
API:

```rust
gaman::parsers::parse_sql(sql, dialect)
```

The parser is a schema loader, not a migration parser. It accepts `CREATE`
statements for entity kinds Gaman models. `ALTER`, `DROP`, `DELETE`, and other
non-`CREATE` statements should be represented as expected-error fixtures.

## Location

Parser fixtures live under `tests/cases/parser/` and are grouped by dialect:

- `tests/cases/parser/postgres/`
- `tests/cases/parser/sqlite/`
- `tests/cases/parser/mysql/`
- `tests/cases/parser/mariadb/`

MySQL and MariaDB are recorded independently. Both use the private MySQL-family
AST parser where syntax overlaps, while MariaDB-specific recovery and opaque
fallback remain separate dialect behavior.

## Fixture shape

```yaml
description: PostgreSQL CREATE TYPE enum lowers into EnumDef
dialect: postgres
sql: |
  CREATE TYPE mood AS ENUM ('happy', 'sad');

expect_entities:
- kind: enum
  name: mood

expect_schema:
  enums:
    mood:
      name: mood
      values: [happy, sad]
```

Supported fields:

- `description`: globally unique human-readable behavior.
- `dialect`: `postgres` or `sqlite`.
- `sql`: SQL DDL input.
- `expect_entities`: compact entity coverage assertions.
- `expect_schema`: exact Gaman `Schema` expected after parsing.
- `expect_error`: optional substring for expected parser or lowering errors.

`expect_schema` and `expect_error` are mutually exclusive. Successful fixtures
must provide `expect_entities`; error fixtures must not list entities.

## Entity assertions

Entity assertions describe the schema objects expected to exist after parsing.
They intentionally stay compact so the exact shape remains in `expect_schema`.

```yaml
- kind: table
  name: users

- kind: column
  table: users
  name: email

- kind: constraint
  table: users
  name: users_email_key

- kind: foreign_key
  table: posts
  name: posts_user_id_fkey

- kind: index
  table: users
  name: users_email_idx

- kind: trigger
  table: users
  name: users_ai

- kind: function
  name: audit_users

- kind: view
  name: active_users

- kind: enum
  name: mood

- kind: extension
  name: pgcrypto
```

## Parser lifecycle under test

Parser fixtures exercise the public parser lifecycle:

1. Segment SQL text before AST parsing.
2. Parse each segment with the selected dialect.
3. Reject unsupported or non-schema-loader statements clearly.
4. Lower supported statements into Gaman-owned schema structs.
5. Normalize the resulting schema.

Statement segmentation is parser-independent and is tested in `gaman-core`.
Segmenter unit tests cover semicolon boundaries, final statements without a
terminator, PostgreSQL dollar-quoted bodies, SQLite trigger bodies, MySQL
`DELIMITER` routines, comments, byte offsets, and narrow statement
classification.

## Running parser fixtures

```bash
cargo test -p gaman --test parser
cargo test -p gaman --test parser -- tests/cases/parser/postgres
cargo test -p gaman --test parser -- tests/cases/parser/sqlite/sqlite_trigger_body.yaml
cargo test -p gaman --test parser -- 'tests/cases/parser/postgres/*.yaml'
```

Record a local parser result for review. Successful records contain expected and
observed entities separately; rejected cases never count as lowering support.

```bash
cargo test -p gaman --test parser -- --record /tmp/parser-results.yaml
```
