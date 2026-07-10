# Evidence and Support Matrix

For the detailed generated evidence inventory by parser, inspection, and `verify` behavior, see [Support Evidence](support-evidence.md).

Gaman support claims are generated from accepted fixture evidence. Do not
hand-edit support claims in README tables.

## Accepted evidence files

Checked-in accepted evidence lives in `results/`:

- `results/parser-results.yaml`: parser fixture evidence.
- `results/offline-results.yaml`: deterministic fixture evidence.
- `results/online-results.yaml`: live fixture evidence.

Local/ad-hoc outputs should use ignored paths such as:

- `results/online-support-results.yaml`
- `results/offline-support-results.yaml`
- `results/coverage/`

Accepted evidence should be refreshed only when the related behavior or support
claim intentionally changes.

Refresh all accepted evidence and generated support docs:

```bash
scripts/refresh-evidence.sh
```

## Recording evidence

Parser evidence:

```bash
cargo test -p gaman --test parser -- --record results/parser-results.yaml
```

Offline evidence:

```bash
cargo test -p gaman --features sqlite --test offline -- --record results/offline-results.yaml
```

Online evidence:

```bash
set -a; source .env; set +a; cargo test -p gaman --features sqlite --test online -- --record results/online-results.yaml
```

## Support matrix generation

`tests/cases/support-matrix.yaml` defines README support rows. It references
accepted online evidence, offline evidence, and explicit design notes for
unsupported or bounded rows.

The README support matrix is generated from those files and wrapped in checked
markers.

Generate the matrix:

```bash
cargo run --bin gaman-support-matrix -- --update-readme
```

Generate the detailed support evidence page:

```bash
cargo run --bin gaman-evidence-doc -- --update-doc
```

Check that the detailed support evidence page is current:

```bash
cargo run --bin gaman-evidence-doc -- --check
```

Print offline evidence:

```bash
cargo run --bin gaman-support-matrix -- --offline
```

Validate evidence and README support rows:

```bash
cargo test -p gaman --test offline_coverage
```

## Evidence validation rules

`offline_coverage` fails when:

- README support rows drift from generated output;
- `docs/support-evidence.md` drifts from generated output;
- a supported or partial evidence cell has no successful evidence;
- a partial or unsupported cell has no design note;
- accepted evidence points to a missing fixture;
- a fixture references an unknown product feature.

Support matrix policy:

- a green live README cell must have online evidence;
- a green offline README cell may use offline evidence when the feature is
  offline by design;
- partial support requires evidence and a design note;
- unsupported support requires an explicit design note;
- planned or unimplemented cells must stay non-green until evidence is recorded.

## Reviewing evidence diffs

Evidence diffs should be reviewed like code. A changed result file can mean a
feature was added, a fixture was renamed, support moved between dialects, or a
behavior regressed.

When evidence changes, verify that:

- fixture descriptions still describe user-visible behavior;
- feature ids are stable and intentional;
- new failures are expected-error cases, not accidental regressions;
- README support rows are regenerated when support claims change.
