# Freeze-Gate Evidence

This ledger is the storage-engine/04 gate-35 evidence packet. It stays active
until W7.2 assembles the final clean-worktree packet and flips LayoutEpoch 0.

Each producing lane records the evidence families it owns. After that lane
lands on `main`, the lane cleanup commit records the integrating commit and
reviewer verdicts for those rows. W7.2 adds the final fresh full-workspace run,
including the explicit `CARGO_TARGET_DIR`, before storage-engine/04 is flipped.

## Seeded Evidence

| Family | Lane | Evidence | Verification |
|---|---|---|---|
| Crash/recovery | W2.2 | Native redb crash harness covers kill-before-outer-commit, kill-after-outer-commit, and bounded commit-race both-or-invisible checks. Store-open robustness remains in the native store suite. | `cargo test -p marrow-store --features native --test crash_recovery_harness`; `cargo test -p marrow-store --features native` |
| Ordering/codecs | W2.2 | Saved-key codecs round-trip representative values and preserve typed byte order. Data-cell keys round-trip all cell kinds. Commit metadata round-trips all fields and rejects trailing bytes. Existing value and identity payload codec suites stay in the native store family. | `cargo test -p marrow-store --features native` |
| Backup all-or-nothing | W2.2 | Corrupt and trailing-byte restore refusals now also assert the target store remains empty after failure. W2.4 extends this family with atomic backup/fmt writes and replace-only-on-success coverage. | `cargo test -p marrow --test backup_cli` |
| Atomic artifact writes | W2.4 | Backup and fmt writes publish through adjacent temp files and plain `rename`, injected mid-stream failures preserve prior bytes and leave no temp artifact, and Unix backup/store file creation asserts owner-only `0600` under a permissive `umask`. The W2.4 source scan covers the owned backup/restore/fmt write sites, symlink hop limits, and the native store file creation hook. | `cargo test -p marrow --test backup_cli --test fmt_cli`; `! rg -n "File::create\|fs::write" crates/marrow/src/cmd_backup.rs crates/marrow/src/backup/create.rs crates/marrow/src/cmd_restore.rs crates/marrow/src/cmd_fmt.rs crates/marrow-store/src/redb.rs`; `rg -n "create_new\|mode\\(0o600\\)\|fs::rename\|remove_file\|SYMLINK_HOP_LIMIT\|sync_parent_directory\\(created_path\\)" crates/marrow/src/cmd_backup.rs crates/marrow/src/cmd_restore.rs crates/marrow/src/cmd_fmt.rs crates/marrow-store/src/redb.rs` |

## Integrated Lane Evidence

| Lane | Integrated Commit | Reviewer Verdicts | Broad Gates |
|---|---|---|---|
| W2.2 | `ea6ce9cadf0ec84cfaa5070c703329fefd9c43ce` | Boyle soundness PASS; Hume idiom/spec PASS. | `cargo build --workspace`; `cargo test --workspace`; `cargo clippy --workspace --all-targets -- -D warnings`; `cargo fmt --all -- --check`; `python3 tools/docs_lint.py`; `git diff --check` |

## Final Assembly Rules

The W7.2 assembly run is green only when every fixture in the five gate-35
families passes in one fresh clean worktree with zero ignored tests inside those
families. Deterministic crash kill points must pass every time, and bounded
crash soak must show zero both-or-invisible violations.

The final packet records:

- the producing lane for each family;
- the integrating `main` commit for each family;
- reviewer verdicts for the lane and final integration review;
- the suite names run for each family;
- the fresh W7.2 full-workspace command with explicit `CARGO_TARGET_DIR`.

Any red family blocks storage-engine/04 acceptance and the v0.1 tag until a fix
lane lands and the entire affected family reruns fresh.
