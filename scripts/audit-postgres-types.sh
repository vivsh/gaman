#!/usr/bin/env bash
set -euo pipefail

: "${POSTGRES_DATABASE_URL:?set POSTGRES_DATABASE_URL to a pristine PostgreSQL database URL}"

# This is an audit aid, not a build input. It intentionally emits candidates so
# maintainers can classify additions as native column types, pseudo-types, or
# catalog implementation details before changing the checked catalog.
psql "$POSTGRES_DATABASE_URL" --no-psqlrc --tuples-only --no-align \
  --command "
    SELECT t.typname
    FROM pg_type AS t
    JOIN pg_namespace AS n ON n.oid = t.typnamespace
    WHERE n.nspname = 'pg_catalog'
      AND t.typtype = 'b'
      AND t.typname NOT LIKE '\\_%'
    ORDER BY t.typname;
  "
