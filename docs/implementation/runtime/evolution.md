# Runtime: Data Evolution

The runtime half of source-native schema evolution. It consumes the read-only `EvolutionWitness` that `marrow-check`'s [preview](../check/evolution.md) produced and commits the durable data rewrite the witness describes — backfills, transforms, index rebuilds/drops, retire deletes — as one atomic `WritePlan` plus a metadata stamp. The stamp also publishes the activated catalog snapshot into the store's catalog rows, so the accepted catalog advances in the same transaction as the data and epoch it describes.

The witness is a proof, not a work list. `apply` re-runs preview against the live source/catalog/store and demands byte-for-byte equality (any change since preview is `Drift` before a single write is staged). It re-derives every obligation from the live store one identity at a time, never from an identity list in the witness, then reconciles staged counts against the witness before commit. Drift, a blocking obligation, a count mismatch, a transform body fault, or a store error leaves the store byte-identical.

## Drivers

One staging engine serves two callers:

- **`evolve apply`** — explicit, operator-driven, full obligation set.
- **`run` auto-apply** — unattended; applies only zero-record-mutation changes, otherwise fences.

`verify_activation_completion` re-proves a stamped commit's evidence against a recomputed witness; it is the verifier backup restore and the apply suites reuse, not a separate staging path.

## Apply control flow

`apply` is the spine: validate witness → fence → gate repair/destructive obligations → stage each obligation into the plan → deferred index-rebuild pass → reconcile counts → atomic commit with stamp → return `ActivationReceipt`. Staging appends typed `PlanStep`s (`WriteData`/`DeleteData`/`WriteIndex`/`DeleteIndexSubtree`/`StampMetadata`); `commit_apply_plan` re-asserts the store commit pin inside the transaction and commits once.

## Modules

| File | Responsibility |
| --- | --- |
| `evolution/mod.rs` | Module tree and public surface; re-exports `apply`/`try_auto_apply`/`verify_activation_completion`/`fence`/`rebuild_store_indexes`/`commit_catalog_baseline`. |
| `evolution/apply.rs` | Apply orchestrator; defines `Approval`, `ApplyOutcome`, `ActivationReceipt`, `ApplyError`, `StagedWork`, `stage_obligation`, `reconcile_counts`, `commit_apply_plan`. |
| `evolution/baseline.rs` | `commit_catalog_baseline`: freeze a project's first proposed catalog into an empty store as one `StampMetadata` step through `WritePlan` (catalog rows, epoch, engine profile, commit metadata via the shared `metadata_stamp`); a no-op when the store already holds a catalog or any saved data. |
| `evolution/validate.rs` | `validate_witness` (re-preview, byte equality, `Drift`), `assert_commit_pin` (`StoreCommitDrift`), and `assert_accepted_catalog_pin` (the store's published catalog digest must match the witness's accepted catalog, else `CatalogDrift`). |
| `evolution/window.rs` | Activation-window `fence` (engine profile, catalog epoch, schema-bearing source digest) and the `metadata_stamp` / `current_engine_profile` shared with managed writes. |
| `evolution/admission.rs` | Gates `RepairRequired` (`NotActivatable`) and destructive retires; requires maintenance plus an exact per-id scoped `Approval`. |
| `evolution/auto_apply.rs` | Classifies the witness's heaviest record obligation (`RunObligation`) and applies only `ZeroMutation` via the production path, else `MustFence`. |
| `evolution/backfill.rs` | Stages non-transform verdicts: default backfills, index subtree rebuilds, index drops, retire deletes; re-scans each root from the live store. |
| `evolution/transform.rs` | Per-record checked-transform execution; binds reads as `old`, runs the pure body, encodes to the target leaf; splits discharge divergence (`Corruption`) from `TransformBodyFaulted`. |
| `evolution/evidence.rs` | Domain-separated SHA-256 folds: ordered `EvidenceDigest` for default cells, bounded order-independent `EvidenceSetDigest` for index rows, retire-evidence digest. |
| `evolution/locate.rs` | Read-only `MemberLocation`/`PathStep` resolution and per-record iteration of a place's stored records. |
| `evolution/lifecycle.rs` | Sole owner of retired-id classification: proposal entries Reserved now but Active in the accepted catalog. |
| `evolution/rebuild.rs` | Restore-side `rebuild_store_indexes`; re-derives declared indexes from committed records inside the caller's transaction. |
| `evolution/completion.rs` | `verify_activation_completion` orchestrating the seven completion verifiers that re-prove a stamped commit against a recomputed witness. |
| `evolution/completion/{default,index,transform,retire,receipt,proposal,verdict}.rs` | Per-aspect re-proofs of stamped evidence against the recomputed witness (default cells, index digests, transform bytes, retire counts, default receipts, stamp identity, no residual repair verdict). |

## Invariants worth knowing before editing

- Witness byte equality is the consistency proof; staging never trusts a witness identity list.
- Two concurrency guards: `validate_witness` checks the commit pin once, `commit_apply_plan` re-asserts it inside the transaction.
- The fence checks the store's *pre-apply* shape (the digest the witness recorded), so a shape-changing apply does not fence itself.
- Stamp and fence read the same facts by construction, so a store this binary just wrote passes its own fence.
- Index rebuilds are deferred to a second pass so they see same-apply defaults/transforms via the staged-data view.
- Evidence digests share one recipe across staging and completion, so a completed activation reproduces the digest its stamp recorded.
- The activated catalog snapshot, epoch, and data commit in one transaction, so the accepted catalog never advances without the data it describes and there is no post-commit publish window.

## Discrepancies with stated design

- `mod.rs`'s top doc comment describes only the apply path; the module also owns auto-apply, restore-side index rebuild, the open fence, and activation-completion verification.
- `apply` maps a retire count that overflows `usize`→`u64` to `ApplyError::Drift` (via `retire_counts_u64`), reusing `Drift` for an arithmetic impossibility rather than an internal-error variant.
- `completion/proposal.rs` (`verify_proposal_identity`) runs only when `witness.proposal_catalog` is `Some`, returning `Drift` otherwise; completion verification is implicitly specced for proposal-bearing activations only.

## Read next

- `evolution/apply.rs` — `apply`: whole control flow, no-op short-circuit, staging loop, reconciliation, commit.
- `evolution/backfill.rs` — `stage_default_backfill` / `scan_default_cells`: live-store re-derivation, proposal-new vs accepted-optional fail-closed rule, evidence seeding.
- `evolution/transform.rs` — `visit_transform_writes`: shared per-record engine reused by apply and completion; body-fault vs discharge-divergence split.
- `evolution/completion.rs` — `verify_activation_completion`: how the seven verifiers re-prove a stamped commit's evidence against a recomputed witness.
- `evolution/window.rs` — `fence` / `metadata_stamp`: the stamp/fence symmetry that ties apply, run, and completion together.
