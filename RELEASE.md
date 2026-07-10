# Release Checks

This document lists release-oriented validation. It is intentionally separate
from `TESTING.md` so the contributor testing guide can stay stable and concise.

Before release, run the normal Rust and fixture gates, refresh accepted evidence
when behavior changed, regenerate support tables, and verify packaging.

## Suggested release gate

```bash
cargo test -p gaman-core
cargo test -p gaman
cargo test -p gaman --features sqlite
cargo test -p gaman --no-default-features --features offline
cargo check -p gaman --no-default-features --features offline --target wasm32-unknown-unknown
scripts/refresh-evidence.sh
cargo fmt
git diff --check
cargo package --allow-dirty
```

## Notes

- Run live PostgreSQL checks with a disposable database configured by
  `POSTGRES_DATABASE_URL`.
- SQLite online checks can use a temporary file-backed database when
  `SQLITE_DATABASE_URL` is not set.
- Review all checked-in evidence diffs before committing a release change.
- Coverage reports are generated with `scripts/coverage.sh` and written under
  `results/coverage/`.
