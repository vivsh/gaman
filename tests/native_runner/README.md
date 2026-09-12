# Native runner PostgreSQL regressions

These tests apply real managed-row migrations through `NativeRunnerFactory`
using both directory-backed and embedded migration assets. Each live test owns
a disposable database, observes committed state through a separate connection,
and cleans up its database even when an assertion fails.

Run against a dedicated dbharness environment:

```bash
target=postgres:16
namespace=gaman-native-rows
harness="$HOME/Projects/dbharness/bin/dbharness"
trap '"$harness" down "$target" --namespace "$namespace"' EXIT
"$harness" up "$target" --namespace "$namespace"
eval "$("$harness" env "$target" --namespace "$namespace" --prefix POSTGRES)"
GAMAN_NATIVE_TEST_ADMIN_URL="$POSTGRES_ADMIN_DATABASE_URL" \
  cargo test -p gaman --locked --no-default-features --features postgres,fs \
  --test native_runner -- --include-ignored
```

Repeat for PostgreSQL 17 and 18. PostgreSQL 18 requires its data volume to be
mounted at `/var/lib/postgresql`; older dbharness configurations using
`/var/lib/postgresql/data` need a container configuration correction first.

Live tests are explicitly ignored during ordinary database-free test runs.
Always use `--include-ignored` for live evidence: an explicitly run live test
fails when `GAMAN_NATIVE_TEST_ADMIN_URL` is missing. This variable must name the
loopback administrative URL of the dedicated dbharness environment, never an
application database. The connection-error test runs without a database.
