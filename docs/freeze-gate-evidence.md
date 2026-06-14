# Freeze-Gate Evidence

This ledger is the storage-engine/04 gate-35 evidence packet for the accepted
LayoutEpoch 0 release contract. It records producing-lane evidence, reviewer
verdict requirements, and the verification commands required before the v0.1
tag.

Each producing lane records the evidence families it owns. After that lane
lands on `main`, the integrated-lane table records the producing commit. The
final W7.2 soundness and idiom/spec reviews are the release-contract review for
the assembled packet; W7.4 repeats the fresh full-workspace gate before the tag.

## Status

| Item | State |
|---|---|
| storage-engine/04 | Accepted for the v0.1 release contract. |
| LayoutEpoch | `0` frozen. |
| Engine profile | `key=v0`, `layout-epoch=0`, `digest=77944eb86c08b665`. |
| W7.2 assembly run | Green in `/Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.2/marrow` with `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-roadmap/W7.2-final`; workspace tests report zero ignored tests. |
| W7.3 docs-truth run | Green in `/Users/scottwilliams/Dev/.worktrees/marrow-roadmap/W7.3/marrow`; soundness and idiom/spec re-reviews passed after adversarial scanner probes. |

## Seeded Evidence

| Family | Lane | Evidence | Verification |
|---|---|---|---|
| Crash/recovery | W2.2 | Native redb crash harness covers kill-before-outer-commit, kill-after-outer-commit, and bounded commit-race both-or-invisible checks. Store-open robustness remains in the native store suite. | `cargo test -p marrow-store --features native --test main crash_recovery_harness::`; `cargo test -p marrow-store --features native` |
| Ordering/codecs | W2.2 | Saved-key codecs round-trip representative values and preserve typed byte order. Data-cell keys round-trip all cell kinds. Commit metadata round-trips all fields and rejects trailing bytes. Existing value and identity payload codec suites stay in the native store family. | `cargo test -p marrow-store --features native` |
| Backup all-or-nothing | W2.2 | Corrupt and trailing-byte restore refusals now also assert the target store remains empty after failure. W2.4 extends this family with atomic backup/fmt writes and replace-only-on-success coverage. | `cargo test -p marrow --test main backup_cli::` |
| Atomic artifact writes | W2.4 | Backup and fmt writes publish through adjacent temp files and plain `rename`, injected mid-stream failures preserve prior bytes and leave no temp artifact, and Unix backup/store file creation asserts owner-only `0600` under a permissive `umask`. The W2.4 source scan covers the owned backup/restore/fmt write sites, symlink hop limits, and the native store file creation hook. | `cargo test -p marrow --test main backup_cli::`; `cargo test -p marrow --test main fmt_cli::`; `! rg -n "File::create\|fs::write" crates/marrow/src/cmd_backup.rs crates/marrow/src/backup/create.rs crates/marrow/src/cmd_restore.rs crates/marrow/src/cmd_fmt.rs crates/marrow-store/src/redb.rs`; `rg -n "create_new\|mode\\(0o600\\)\|fs::rename\|remove_file\|SYMLINK_HOP_LIMIT\|sync_parent_directory\\(created_path\\)" crates/marrow/src/cmd_backup.rs crates/marrow/src/cmd_restore.rs crates/marrow/src/cmd_fmt.rs crates/marrow-store/src/redb.rs` |
| Store concurrency contract | W2.5 | CLI store-open tests cover both directions of the native process-lock contract: read-only commands report `store.locked` while a writer holds the store, and write-capability commands report `store.locked` while a read-only inspection holds it. The docs render the Concurrency section, the `store.locked` remedy, and the outbox idiom without adding source syntax. | `cargo test -p marrow --test main store_open_robustness_cli::`; `rg -n "Read-only opens coexist\|read-write open\|writer or a read-only inspection\|outbox" docs/backend-contract.md docs/language/resources-and-storage.md docs/error-codes.md` |

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
- the fresh full-workspace tag-gate commands with explicit `CARGO_TARGET_DIR`.

Any red family found during the W7.4 tag gate blocks the v0.1 tag until a fix
lane lands and the entire affected family reruns fresh.
