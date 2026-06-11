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
