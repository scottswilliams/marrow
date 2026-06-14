# Freeze-Gate Evidence

This ledger is the storage-engine/04 gate-35 evidence packet for the accepted
LayoutEpoch 0 release contract. It records producing-lane evidence, reviewer
verdict requirements, and the verification commands required before the
`v0.1.0` tag.

Each producing lane records the evidence families it owns. After that lane
lands on `main`, the integrated-lane table records the producing commit. The
final W7.2 soundness and idiom/spec reviews are the release-contract review for
the assembled packet; W7.4 repeats the fresh full-workspace gate before the
`v0.1.0` tag.

## Status

| Item | State |
|---|---|
| storage-engine/04 | Accepted for the v0.1 release contract. |
| LayoutEpoch | `0` frozen. |
| Engine profile | `key=v0`, `layout-epoch=0`, `digest=77944eb86c08b665`. |
| W7.2 assembly run | Green in `/Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow` with `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final`; workspace tests report zero ignored tests. |
| W7.3 docs-truth run | Integrated on `main` at `70d31008287291be741c5bd245258a42503c369b`; soundness and idiom/spec re-reviews passed after adversarial scanner probes. |
| W7.4 final tag gate | Integrated on `main` at `7929527d4ac3ceacb0d8da57112e81f1575021f1`; tag `v0.1.0` pushed and peeled to that commit. |

## Seeded Evidence

| Family | Lane | Evidence | Verification |
|---|---|---|---|
| Conformance | W2.8/W3.2/W4.7/W4.8 | Backend conformance, reverse and bounded scans, identity-component laws, and CommitId-density laws are covered by store, runtime, and fixture suites. | `cargo test -p marrow-store --features native`; `cargo test -p marrow-run --test main commit_id_conformance::`; `cargo test -p marrow-run --test main eval_index_iteration::bounded_index_range_streams_matching_records`; `cargo test -p marrow-run --test main eval_index_identity::index_over_identity_field_streams_matching_records`; `cargo test -p marrow-run --test main eval_layer_enumeration::reversed`; `cargo test -p marrow-check --test main v01_fixtures::`; `cargo test -p marrow --test main v01_cli::` |
| Ordering/codecs | W2.2/W2.8 | Saved-key codecs round-trip representative values and preserve typed byte order. Data-cell keys round-trip all cell kinds. Commit metadata round-trips all fields and rejects trailing bytes. Existing value and identity payload codec suites stay in the native store family. | `cargo test -p marrow-store --features native` |
| Crash/recovery | W2.2 | Native redb crash harness covers kill-before-outer-commit, kill-after-outer-commit, and bounded commit-race both-or-invisible checks. Store-open robustness remains in the native store suite. | `cargo test -p marrow-store --features native --test main crash_recovery_harness::`; `cargo test -p marrow-store --features native` |
| Backup | W2.2/W2.4/W3.8 | Corrupt and trailing-byte restore refusals leave targets unchanged; epoch-mismatch restore is refused; backup and fmt writes publish atomically through adjacent temp files; owner-only artifact modes are pinned on Unix. | `cargo test -p marrow --test main backup_cli::`; `cargo test -p marrow --test main fmt_cli::` |
| Evolution | W3.8 | Multi-epoch lifecycle, alias retention, second-epoch drift, and retired-path reservation are covered by the lifecycle suite. | `cargo test -p marrow-run --test main evolution_apply_lifecycle::` |

## Integrated Lane Evidence

| Lane | Integrated Commit | Reviewer Verdicts | Broad Gates |
|---|---|---|---|
| W2.2 | `ea6ce9cadf0ec84cfaa5070c703329fefd9c43ce` | Boyle soundness PASS; Hume idiom/spec PASS; W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | `cargo build --workspace`; `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `python3 tools/docs_lint.py`; `git diff --check` |
| W2.4 | `4b0415dd27827410cc12e719fd2c04dced5fe8bd` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Atomic backup/fmt write paths and owner-only artifact mode suites are included in the W7.2 assembly run. |
| W2.8 | `f0caffaec951a911976aa9958bbc3ae30d7fcdc6` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Linear navigation, key ordering, byte fingerprints, and bounded-scan suites are included in the W7.2 assembly run. |
| W3.2 | `2a3b3fbf26a26943138f865985f1824c647ad3dd` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Commit-id density and reader/writer contract suites are included in the W7.2 assembly run. |
| W3.4 | `d6b0dfab55faa66216eb2607e44983af9ae4543e` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Activation-stamp, backup-manifest, and restore identity suites are included in the W7.2 assembly run. |
| W3.8 | `023bcbcf68eb3efec8edbe77a03321dcd2aef934` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Multi-epoch evolution and epoch-mismatch restore suites are included in the W7.2 assembly run. |
| W4.7/W4.8 | `f21b9eefb37c63165540eb1681776fc6feb27fa4` / `c6e90842edb32d99d027f053958d65651bfe172b` | W7.2 soundness PASS (release-contract re-review); Final W7.2 idiom/spec PASS. | Reverse/bounded saved-read and conformance-corpus suites are included in the W7.2 assembly run. |
| W7.3 | `70d31008287291be741c5bd245258a42503c369b` | Banach soundness PASS; Turing idiom/spec PASS. | W7 absence scan, docs lint, release tidy, and full workspace gates passed on the W7.3 branch and again after fast-forward integration on `main`. |
| W7.4 | `7929527d4ac3ceacb0d8da57112e81f1575021f1` | Mencius soundness re-review PASS; Linnaeus idiom/spec re-review PASS. | W7.4 focused families, release tidy, docs/absence scans, full workspace gates, and post-integration main gates passed before `v0.1.0` was tagged and pushed. |

## W7.2 Assembly Commands

Focused version contract check:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-build cargo test -p marrow --test main usage_cli::version_prints_engine_profile_tuple --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow/Cargo.toml
```

Result: pass, `1 passed; 0 failed; 0 ignored; 0 measured; 380 filtered out`.

The final W7.2 assembly commands were run fresh:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final cargo build --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final cargo test --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final cargo clippy --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow/Cargo.toml -- -D warnings
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final cargo fmt --all --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow/Cargo.toml -- --check
python3 tools/docs_lint.py
git diff --check
```

Result: build pass; full workspace tests pass with zero ignored tests; clippy
passes with `-D warnings`; docs lint and whitespace checks pass.

## W7.3 Docs-Truth Commands

Seeded stale-spelling check, before the scanner implementation, proved the
recorded seed was active:

```sh
python3 tools/w7_absence_scan.py --assert-seed at-sugar
```

Result: failed with `seed at-sugar was not detected`.

After implementation, the same seed is detected without any real stale hits:

```sh
python3 tools/w7_absence_scan.py --assert-seed at-sugar
```

Result: `w7_absence_scan seed at-sugar detected __seed__/w7_absence_seed.mw:2`.

The clean cumulative removed-surface scan is:

```sh
python3 tools/w7_absence_scan.py
```

Result: `w7_absence_scan passed`.

The scanner's built-in probe suite covers every removed-pattern family named by
W7.3:

```sh
python3 tools/w7_absence_scan.py --assert-probes
```

Result: `w7_absence_scan probe suite detected 29 pattern families`.
The suite includes reviewed false-negative probes for Rust-side `write`
variants and string fixtures, parser-path stale syntax, evidence fields, glob
grammar, and incidental stale CLI/concat spellings in crate test paths.

Release tidy pins the release-target dependency count and the release binary
size envelope:

```sh
python3 tools/release_tidy.py --target-dir /Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.3-release-tidy
```

Recorded result on `aarch64-apple-darwin`: `dependency_count=28`,
`binary_bytes=5859632`, `binary_baseline_bytes=5860128`,
`binary_max_bytes=7325160`, and
`binary_sha256=243ddc7470212281e4601a24162539157d257cbea409eed25105dfecf3873bfe`.

## W7.4 Final Tag Gate

The W7.4 evidence-packet seed was the missing final-gate section:

```sh
rg -n "## W7\\.4 Final Tag Gate" docs/freeze-gate-evidence.md
```

Result before this section was added: exit `1`, no matches.

The pre-tag W7.4 branch starts from integrated `main` commit
`70d31008287291be741c5bd245258a42503c369b`. The pre-tag packet changes only
`ROADMAP.md` and `docs/freeze-gate-evidence.md`; the tag name is `v0.1.0`,
matching the workspace version and install/stability docs. A remote tag
preflight found no existing `v0.1*` tag:

```sh
git ls-remote --tags origin 'v0.1*'
```

Result: no output.

The W7.4 docs and absence checks are:

```sh
python3 -m py_compile tools/w7_absence_scan.py tools/release_tidy.py tools/docs_lint.py
python3 tools/docs_lint.py
python3 tools/w7_absence_scan.py --assert-probes
python3 tools/w7_absence_scan.py --assert-seed at-sugar
python3 tools/w7_absence_scan.py
git diff --check
find . -name TRACKER.md -print
```

Result: Python syntax pass; docs lint pass; probe suite detected `29` pattern
families; seed detected `__seed__/w7_absence_seed.mw:2`; clean scan passed;
whitespace check passed; no `TRACKER.md` found.

Release tidy and the release binary measurement use the release target
directory `/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-release-tidy`:

```sh
python3 tools/release_tidy.py --target-dir /Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-release-tidy
```

Recorded result on `aarch64-apple-darwin`: `dependency_count=28`,
`binary_bytes=5859632`, `binary_baseline_bytes=5860128`,
`binary_max_bytes=7325160`, and
`binary_sha256=971c17cd46cf569030c0321749bd423de1a5c5096bf3604163fa9ce246544bf7`.

Reference setup: macOS `26.5.1` build `25F80` on Apple M5 Pro with
`25769803776` bytes RAM; `rustc 1.89.0 (29483883e 2025-08-04)`; `cargo 1.89.0
(c24e10642 2025-06-23)`. The measured command was the release binary's
`--version`, which printed:

```text
marrow 0.1.0 engine-profile=(key=v0, layout-epoch=0, digest=77944eb86c08b665)
```

Cold-start measurements from three `/usr/bin/time -l` runs were:

| Run | Wall clock | Maximum resident set size | Peak memory footprint |
|---|---:|---:|---:|
| 1 | `0.20 real` | `2015232` bytes | `1196368` bytes |
| 2 | `0.00 real` | `2031616` bytes | `1212752` bytes |
| 3 | `0.00 real` | `2015232` bytes | `1196368` bytes |

Focused gate-35 family checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-focused cargo test -p marrow-store --features native --test main crash_recovery_harness:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-focused cargo test -p marrow-store --features native --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-v01-check cargo test -p marrow-check --test main v01_fixtures:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-v01-cli cargo test -p marrow --test main v01_cli:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-runtime-bounded cargo test -p marrow-run --test main eval_index_iteration::bounded_index_range_streams_matching_records --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-runtime-identity cargo test -p marrow-run --test main eval_index_identity::index_over_identity_field_streams_matching_records --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-runtime-reversed cargo test -p marrow-run --test main eval_layer_enumeration::reversed --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-docs-corpus cargo test -p marrow-check --test main language_reference_docs:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-backup-family cargo test -p marrow --test main backup_cli:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-fmt-family cargo test -p marrow --test main fmt_cli:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-evolution-family cargo test -p marrow-run --test main evolution_apply_lifecycle:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-commit-id-family cargo test -p marrow-run --test main commit_id_conformance:: --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
```

Result: crash harness `4 passed; 0 ignored`; native store family `85` unit
tests and `88` integration tests passed with zero ignored tests; v01 check
fixture `1 passed`; v01 CLI/corpus `2 passed`; bounded range stream `1 passed`;
identity-component stream `1 passed`; reversed runtime checks `5 passed`; docs
reference corpus `2 passed`; backup family `27 passed`; fmt artifact family `22
passed`; evolution lifecycle family `4 passed`; CommitId-density family `2
passed`. Every focused family reported zero ignored tests.

The fresh full-workspace W7.4 broad gate is:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-final cargo build --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-final cargo test --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-final cargo clippy --workspace --all-targets --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml -- -D warnings
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.4-final cargo fmt --all --manifest-path /Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.4/marrow/Cargo.toml -- --check
git diff --check
rg -n '#\s*\[\s*(?:ignore(?:\s*=|\s*\])|cfg_attr\([^\]]*\bignore\b)' crates docs README.md
rg -n '(^|[^A-Za-z0-9_])unsafe\s*(\{|fn|impl|trait|extern)' crates
git diff -- Cargo.lock Cargo.toml crates/*/Cargo.toml
```

Result: build passed; full workspace tests passed; clippy passed with
`-D warnings`; formatter check passed; whitespace check passed; ignored-test and
unsafe scans had no matches; manifest and lockfile diff was empty.

W7.4 review verdicts: Mencius soundness re-review PASS; Linnaeus idiom/spec
re-review PASS. The pushed `v0.1.0` tag is an annotated tag with remote tag
object `ffd53e0f6634d22bdddc371d6a18ae612b242c6f`; it peels to
`7929527d4ac3ceacb0d8da57112e81f1575021f1`.

The post-tag `marrow-decisions` gate-36 deletion commit is
`c22f87c05bd8859208570e94f9741f767cefe5e3`: it deletes
`adr/foundations/03-prototype-status-and-replacement.md` and removes that ADR's
two README index entries. The local `marrow-decisions` checkout has no remote
configured, and GitHub lists no `scottswilliams/marrow-decisions` repository.

## Final Assembly Rules

The W7.4 tag gate is green only when every fixture in the five gate-35
families passes in one fresh clean worktree with zero ignored tests inside those
families. Deterministic crash kill points must pass every time, and bounded
crash soak must show zero both-or-invisible violations.

The final packet records:

- the producing lane for each family;
- the integrating `main` commit for each family;
- reviewer verdicts for the lane and final integration review;
- the suite names run for each family;
- the fresh full-workspace tag-gate commands with explicit `CARGO_TARGET_DIR`;
- the post-tag `marrow-decisions` deletion commit hash.

Any red family found during the W7.4 tag gate blocks the `v0.1.0` tag until a
fix lane lands and the entire affected family reruns fresh.
