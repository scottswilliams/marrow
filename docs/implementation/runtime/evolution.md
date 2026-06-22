# Runtime: Data Evolution

The runtime half of source-native schema evolution. It consumes the read-only `EvolutionWitness` that `marrow-check`'s [preview](../check/evolution.md) produced and commits the durable data rewrite the witness describes — backfills, transforms, index rebuilds/drops, retire deletes — plus a metadata stamp. The stamp also publishes the activated catalog snapshot into the store's catalog rows, so the accepted catalog advances in the same transaction as the data and epoch it describes.

The witness is a proof, not a work list. `apply` re-runs preview against the live source/catalog/store and demands byte-for-byte equality before it opens the write transaction. It re-derives every obligation from the live store one identity at a time, never from an identity list in the witness, writes each obligation through transaction-visible store operations, then reconciles bounded receipt counts against the witness before commit. Drift, a blocking obligation, a count mismatch, a transform body fault, or a store error leaves the store byte-identical.

## Drivers

One apply path serves two callers:

- **`evolve apply`** — explicit, operator-driven, full obligation set.
- **`run` auto-apply** — unattended; applies only zero-record-mutation changes, otherwise fences. The session (`project_session.rs`) enters this path on shape drift *and* when `marrow_check::evolution::has_pending_transform` reports a transform the shape fence cannot see: a shape-neutral in-place transform moves no epoch or source digest, so the pending-evolution run blocker would otherwise miss it. A transform already discharged (its target records the transform's own identity, a hash of the target id and body) no longer reads as pending, so the run proceeds without re-fencing — and an unrelated later edit does not re-open it.

## Apply control flow

`apply` is the spine: validate witness → fence → gate repair/destructive obligations → open the store transaction → re-assert the commit pin → write each obligation directly through the transaction → deferred index-rebuild pass → reconcile counts → stamp metadata → commit → return `ActivationReceipt`. The receipt remains in memory for CLI rendering. Only the slim stamp facts are persisted.

## Modules

| File | Responsibility |
| --- | --- |
| `evolution/mod.rs` | Module tree and public surface; re-exports `apply`/`try_auto_apply`/`fence`/`rebuild_store_indexes`/`commit_catalog_baseline`. |
| `evolution/apply.rs` | Apply orchestrator; defines `Approval`, `ApplyOutcome`, `ActivationReceipt`, `ApplyError`, bounded receipt counters, direct transaction-write helpers, `reconcile_counts`, and `commit_apply_transaction`. |
| `evolution/baseline.rs` | `commit_catalog_baseline`: freeze a project's first proposed catalog, or re-establish an already-accepted snapshot, into an empty store as one `StampMetadata` step through `WritePlan` (catalog rows and commit metadata via the shared `metadata_stamp`). When a committed `marrow.lock` is present, the proposal already carries its adopted ids and epoch high-water (resolved at check time), so a fresh checkout over a wiped store re-establishes the committed identity instead of minting fresh. A no-op when the store already holds a catalog or any saved data. The CLI regenerates `marrow.lock` as a one-way projection only after the store commits. |
| `evolution/validate.rs` | `validate_witness` (re-preview, byte equality, `Drift`), `assert_commit_pin` (`StoreCommitDrift`), and `assert_accepted_catalog_pin` (the store's published catalog digest must match the witness's accepted catalog, else `CatalogDrift`). |
| `evolution/window.rs` | Activation-window `fence` (engine profile, catalog epoch, schema-bearing source digest) and the `metadata_stamp` / `current_engine_profile` shared with managed writes; stamp facts are the commit id, epochs, source digest, engine profile, and touched root/index IDs. |
| `evolution/admission.rs` | Gates `RepairRequired` (`NotActivatable`) and destructive retires; requires maintenance plus an exact per-id scoped `Approval`. |
| `evolution/auto_apply.rs` | Classifies the witness's heaviest record obligation (`RunObligation`) and applies only `ZeroMutation` via the production path, else `MustFence`. |
| `evolution/backfill.rs` | Writes non-transform verdicts: default backfills, index subtree rebuilds, index drops, retire deletes; re-scans each root from the live store inside the caller's transaction. |
| `evolution/transform.rs` | Per-record checked-transform execution; binds reads as `old`, runs the pure body, gates an `ErrorCode` target's result through the shared grammar owner, encodes to the target leaf; splits discharge divergence (`Corruption`) from `TransformBodyFaulted`, which carries the offending record identity and the underlying runtime fault code. |
| `evolution/locate.rs` | Read-only `MemberLocation`/`PathStep` resolution and per-record iteration of a place's stored records. |
| `evolution/lifecycle.rs` | Sole owner of retired-id classification: proposal entries Reserved now but Active in the accepted catalog. |
| `evolution/rebuild.rs` | Restore-side `rebuild_store_indexes`; re-derives declared indexes from committed records inside the caller's transaction. |

## Invariants worth knowing before editing

- Witness byte equality is the consistency proof; apply never trusts a witness identity list.
- Two concurrency guards: `validate_witness` checks the commit pin once, `commit_apply_transaction` re-asserts it inside the transaction, and the stamp resolves only from that pinned predecessor.
- The fence checks the store's *pre-apply* shape (the digest the witness recorded), so a shape-changing apply does not fence itself.
- Stamp and fence read the same facts by construction, so a store this binary just wrote passes its own fence.
- Index rebuilds are deferred to a second pass so they see same-apply defaults/transforms through transaction-visible store writes.
- `ActivationReceipt` is render-only state returned to the caller; commit metadata and backup descriptors do not persist per-effect counts.
- The activated catalog snapshot, epoch, and data commit in one transaction, so the accepted catalog never advances without the data it describes. The live store is the sole write-time authority; the project-root `marrow.lock` is a one-way post-commit projection of the committed snapshot and never an input that could rewrite it.

## Read next

- `evolution/apply.rs` — `apply`: whole control flow, no-op short-circuit, transaction write loop, reconciliation, commit.
- `evolution/backfill.rs` — `stage_default_backfill` / `scan_default_cells`: live-store re-derivation, proposal-new vs accepted-optional fail-closed rule, receipt counts.
- `evolution/transform.rs` — `visit_transform_writes`: shared per-record engine for apply; body-fault vs discharge-divergence split.
- `evolution/window.rs` — `fence` / `metadata_stamp`: the stamp/fence symmetry that ties apply, run, and managed writes together.
