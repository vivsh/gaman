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

The script requires PostgreSQL and MySQL test URLs for currently claimed live
support. MariaDB remains optional while its live matrix is planned. It stages
parser, offline, online, README, and detailed-document outputs, assigns one
generation identifier, validates all files, then publishes them together.
Failure leaves the accepted bundle unchanged.

## Local result recording

The harness flags below are for diagnostics and review. They do not publish an
accepted bundle. Add `--failure-output <path>` to retain a failed run safely.

Parser results:

```bash
cargo test -p gaman --test parser -- --record /tmp/parser-results.yaml
```

Offline evidence:

```bash
cargo test -p gaman --features sqlite --test offline -- --record /tmp/offline-results.yaml
```

Online evidence:

```bash
set -a; source .env; set +a; cargo test -p gaman --features sqlite --test online -- --record /tmp/online-results.yaml
```

## Support matrix generation

`tests/cases/support-matrix.yaml` defines README support rows. Every claim names
an exact fixture and the checks or assertions that prove it for the same
dialect. Feature labels organize evidence; they do not independently prove a
support cell.

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
- accepted result files have different generation identifiers;
- a case descriptor names a check or assertion the case did not execute;
- offline evidence is reused across dialects;
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
