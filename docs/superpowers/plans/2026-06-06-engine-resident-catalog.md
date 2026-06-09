# Engine-Resident Catalog Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the committed JSON accepted catalog with an engine-resident transactional catalog table while preserving Marrow's stable identity, explicit evolution, and fail-closed activation contracts.

**Architecture:** Introduce one typed catalog semantic model, store accepted catalog rows under a new tree-cell catalog family, and publish catalog rows in the same store transaction as activation metadata and data/index changes. Keep checker binding as the semantic owner of proposal, rename, retire, and structural-signature decisions; the store owns only typed persistence and transactionality.

**Tech Stack:** Rust, cargo workspace, `marrow-store` ordered-byte backend, redb native backend, existing checker/runtime/evolution test harnesses.

---

## Architecture Decisions To Approve First

This plan assumes these decisions. Stop before implementation if any are rejected.

- The native store becomes the production source of truth for the accepted catalog. `marrow.catalog.json` is not kept as a production fallback after the migration lanes finish.
- `acceptedCatalog` in `marrow.json` is retired from production semantics. During migration it may name a one-time import/export artifact, but final binding reads the store catalog table.
- Durable accepted identity requires a durable store. A memory-only project may still run source checks and produce proposals, but it cannot persist accepted catalog identity across processes.
- `check` stays read-only. For a configured native store, it reads the catalog table through a read-only store handle when the store exists; if the store is absent, it treats the accepted catalog as missing and reports the same pending-identity warnings.
- The catalog table is private engine metadata, not language data. Normal Marrow saved-data access, source declarations, runtime expressions, standard library functions, data CLI reads/writes, and user transactions must not be able to address, query, scan, mutate, or observe catalog rows as a database object.
- Baseline identity and evolution activation write accepted catalog rows, catalog epoch, engine profile, commit metadata, data cells, and index cells in one store transaction.
- Backups include the accepted catalog table. Restore reconstructs data, indexes, metadata, and catalog rows in one transaction, so restored stores do not require a file-publish resume step.

## Current Evidence

- JSON accepted catalog is currently `CatalogMetadata { epoch, digest, entries }` in `crates/marrow-project/src/lib.rs`.
- Checker binding reads `project_root / config.accepted_catalog` in `crates/marrow-check/src/catalog.rs`, then records accepted epoch/digest/entries and proposal state in `ProgramCatalog`.
- `commit_pending_identity` writes only the baseline accepted catalog; later changes intentionally go through `evolve apply`.
- `evolve apply` currently commits store data/metadata first, then writes JSON last, creating the explicit resume window in `cmd_evolve/mod.rs`.
- `TreeStore` already has transaction, metadata, data, index, read-snapshot, and native redb persistence APIs. It does not have a catalog key family.
- Backups currently stream data-family cells only. They bind source digest, catalog epoch, engine profile, and commit metadata, but not the catalog entries themselves.

## Lane 0: Spec And Public Contract

**Files:**
- Modify: `docs/language/resources-and-storage.md`
- Modify: `docs/data-evolution.md`
- Modify: `docs/project-config.md`
- Modify: `docs/backend-contract.md`
- Modify: `docs/error-codes.md`
- Modify: `marrow-decisions/adr/catalog-identity/02-catalog-lifecycle-and-identity-binding.md`

- [ ] Record the approved decision: accepted catalog identity is engine-resident for native stores, not a committed JSON ABI.
- [ ] Update the language docs to say durable identity is invisible, store-resident, and advanced only by state-establishing flows.
- [ ] State explicitly that catalog rows are not Marrow language data: there is no `^catalog` root, no catalog resource, no query surface, no stdlib catalog read/write API, and no normal data CLI operation over catalog rows.
- [ ] Update project config docs to remove `acceptedCatalog` as a production input. If a migration command is approved, describe it as import/export tooling only.
- [ ] Update data-evolution docs so preview/apply describe one store transaction rather than store-then-file publication.
- [ ] Add or update error codes for missing catalog table, corrupt catalog table, catalog/source mismatch, and durable store required.
- [ ] Verification:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-docs \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow --test usage_cli
```

## Lane 1: One Catalog Semantic Model

**Files:**
- Create: `crates/marrow-catalog/Cargo.toml`
- Create: `crates/marrow-catalog/src/lib.rs`
- Modify: `Cargo.toml`
- Modify: `crates/marrow-project/Cargo.toml`
- Modify: `crates/marrow-project/src/lib.rs`
- Modify: `crates/marrow-check/Cargo.toml`
- Modify: `crates/marrow-check/src/catalog.rs`
- Modify: `crates/marrow-check/tests/catalog_presence.rs`
- Modify: `crates/marrow-project/tests/config.rs`

**Intent:** Avoid duplicate catalog semantics. Move `CatalogMetadata`, `CatalogEntry`, `CatalogEntryKind`, `CatalogLifecycle`, digest, validation, and structural-signature decoding into a local `marrow-catalog` crate. `marrow-project` may re-export for compatibility during this lane only; production code should import from `marrow-catalog`.

- [ ] Write failing tests in `crates/marrow-catalog/src/lib.rs` for:
  - digest rejects modified entries;
  - duplicate stable IDs fail;
  - duplicate `(kind, path)` across aliases fail;
  - reserved aliases block reuse;
  - structural signatures decode through one typed enum.
- [ ] Move catalog types and validation from `marrow-project` into `marrow-catalog`.
- [ ] Keep JSON serde support in `marrow-catalog` only for migration/import/export tests. Do not add production code paths that prefer JSON over store rows.
- [ ] Update `marrow-project` config parsing so it owns only project config, not catalog semantics.
- [ ] Update checker imports from `marrow_project::Catalog*` to `marrow_catalog::Catalog*`.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-model \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow-catalog -p marrow-project -p marrow-check catalog_presence
```

**Blocking review criteria:** no duplicate catalog kind/lifecycle enums, no diagnostic-prose matching for catalog semantics, no new compatibility helper that keeps JSON as a production source.

## Lane 2: Store Catalog Table

**Files:**
- Modify: `crates/marrow-store/Cargo.toml`
- Create: `crates/marrow-store/src/catalog.rs`
- Modify: `crates/marrow-store/src/lib.rs`
- Modify: `crates/marrow-store/src/cell.rs`
- Modify: `crates/marrow-store/src/tree.rs`
- Modify: `crates/marrow-store/src/metadata.rs`
- Modify: `crates/marrow-store/tests/tree_store.rs`
- Modify: `crates/marrow-store/tests/redb_store.rs`

**Intent:** Add a transactional table-shaped catalog surface to `TreeStore` without changing checker/runtime behavior yet.

- [ ] Add a new physical key family in `cell.rs`, separate from meta, data, and index families. Do not overload `MetaCell::Commit`.
- [ ] Keep the catalog family out of every data/index decoder and traversal used by normal language access. `decode_data_cell_key`, `DataPathSegment`, `DataAddress`, `IndexAddress`, record scans, index scans, and saved-data backup cell framing must never parse catalog rows as user-addressable cells.
- [ ] Define typed row keys:
  - catalog header row for epoch and digest;
  - catalog entry row keyed by stable ID;
  - optional alias index row keyed by `(kind, path)` if needed for bounded lookup.
- [ ] Define a compact typed row encoding in `marrow-store/src/catalog.rs`. It must reject malformed kind, lifecycle, digest, ID, length, UTF-8, and duplicate row data when decoding into a snapshot.
- [ ] Add `TreeStore` APIs:

```rust
pub fn read_catalog_snapshot(&self) -> Result<Option<CatalogSnapshot>, StoreError>;
pub fn replace_catalog_snapshot(&self, snapshot: &CatalogSnapshot) -> Result<(), StoreError>;
pub fn catalog_snapshot_digest(&self) -> Result<Option<String>, StoreError>;
```

- [ ] `replace_catalog_snapshot` must delete the whole catalog family and write the new header/rows inside the caller's active transaction when one exists.
- [ ] Add tests proving:
  - memory store round-trips a catalog snapshot;
  - redb store persists catalog rows across reopen;
  - rollback restores the previous catalog snapshot along with data/index/meta;
  - catalog family scans do not include data/index/meta cells;
  - data-family and index-family scans do not include catalog rows;
  - normal tree data APIs cannot read or delete catalog rows through crafted `CatalogId`, `SavedKey`, or `DataPathSegment` values;
  - malformed catalog rows report `store.corruption`.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-store \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow-store --features native catalog
```

**Blocking review criteria:** no raw string parsing outside the catalog row decoder, no unbounded materialization beyond the bounded accepted catalog snapshot itself, no store dependency on `marrow-project`, no catalog table access through normal data/index APIs.

## Lane 3: Accepted Catalog Provider For Checker

**Files:**
- Modify: `crates/marrow-check/src/analysis.rs`
- Modify: `crates/marrow-check/src/catalog.rs`
- Modify: `crates/marrow-check/src/lib.rs`
- Modify: `crates/marrow-check/src/program.rs`
- Modify: `crates/marrow-check/tests/catalog_presence.rs`
- Modify: `crates/marrow-check/tests/evolution_discharge.rs`

**Intent:** Make checker catalog binding consume an accepted catalog supplied by a caller, while preserving source-only check behavior and proposal semantics.

- [ ] Add an accepted-catalog input to the project check path. The input should be `Option<CatalogSnapshot>`, not a path and not a store handle.
- [ ] Keep `bind_catalog` as the owner of source entries, rename resolution, retire resolution, ID allocation, and proposal creation.
- [ ] Delete direct file reading from `read_accepted_catalog` after tests are moved to provider inputs.
- [ ] Add tests proving:
  - source-only check with no accepted snapshot proposes epoch 1 and writes nothing;
  - check with accepted snapshot binds IDs exactly as current JSON tests did;
  - invalid accepted snapshot is reported as `check.catalog_intent`;
  - proposal-only IDs still bind activation defaults/transforms but not ordinary facts.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-check \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow-check catalog_presence evolution_discharge
```

**Blocking review criteria:** no checker dependency on native store opening, no fallback branch that silently reads JSON when a store snapshot was expected, no duplicated source catalog entry classifier.

## Lane 4: CLI Store Opening And Baseline Commit

**Files:**
- Modify: `crates/marrow/src/main.rs`
- Modify: `crates/marrow/src/cmd_check.rs`
- Modify: `crates/marrow/src/cmd_run.rs`
- Modify: `crates/marrow/src/cmd_test.rs`
- Modify: `crates/marrow/src/serve/mod.rs`
- Modify: `crates/marrow/tests/check_cli.rs`
- Modify: `crates/marrow/tests/run_cli.rs`
- Modify: `crates/marrow/tests/test_cli.rs`
- Modify: `crates/marrow/tests/serve_cli.rs`
- Modify: `crates/marrow/tests/usage_cli.rs`

**Intent:** Load accepted catalog snapshots from the configured native store and commit first-run baseline identity into the store, not a file.

- [ ] Add CLI helper functions:
  - read-only catalog snapshot for `check`;
  - write-capable native store open for `run` and `evolve apply`;
  - memory-store behavior that keeps checks read-only and refuses durable baseline persistence when no native store is configured.
- [ ] Keep catalog snapshot reads/writes in CLI/checker/evolution/restore plumbing only. Do not expose a language-level command, source declaration, runtime builtin, or ordinary data CLI path that can inspect or mutate catalog rows.
- [ ] Replace `commit_pending_identity(project_root, config, program)` with a store-backed baseline commit that:
  - requires a write-capable `TreeStore`;
  - writes the proposed catalog snapshot;
  - writes catalog epoch, engine profile, and commit metadata in the same transaction;
  - rechecks against the now-accepted store snapshot.
- [ ] Update `run` so a clean project with pending identity opens the store, commits the baseline catalog transaction, then executes.
- [ ] Preserve no-churn behavior: a second run over the same store does not rewrite the catalog table or advance commit metadata.
- [ ] Add tests replacing file assertions with store snapshot assertions:
  - `check` reads but does not create a store;
  - first `run` creates the native store and catalog table;
  - second `run` does not churn epoch/digest;
  - a Marrow program cannot read, write, delete, count, iterate, or trace catalog rows as saved data;
  - `marrow data` reports/repairs user data only and does not expose catalog rows as records or cells;
  - memory-only durable baseline fails with the approved error.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-cli \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow --test check_cli --test run_cli --test usage_cli --features native
```

**Blocking review criteria:** no production write to `marrow.catalog.json`, no populated unstamped loophole, no store creation by plain read-only `check`, no language/runtime/data-CLI surface for catalog rows.

## Lane 5: Evolution Apply Publishes Catalog In Transaction

**Files:**
- Modify: `crates/marrow-run/src/write_plan.rs`
- Modify: `crates/marrow-run/src/evolution/apply.rs`
- Modify: `crates/marrow-run/src/evolution/window.rs`
- Modify: `crates/marrow-run/src/evolution/completion.rs`
- Modify: `crates/marrow/src/cmd_evolve/mod.rs`
- Modify: `crates/marrow/tests/evolve_cli.rs`
- Modify: `crates/marrow-run/tests/evolution_apply.rs`

**Intent:** Remove store-then-file publication. Evolution activation writes the proposal catalog snapshot in the same transaction as data/index changes and metadata stamps.

- [ ] Add a write-plan step for catalog snapshot replacement, or extend the metadata stamp into a typed activation step that includes the proposal snapshot.
- [ ] In `apply`, when `witness.proposal_catalog` is present, stage the proposal snapshot replacement before the metadata stamp in the same `commit_apply_plan` transaction.
- [ ] Update witness validation so apply verifies the store's accepted catalog snapshot fingerprint before staging writes.
- [ ] Ensure catalog snapshot replacement is an activation/runtime-internal write-plan operation only. It must not appear as a user `WriteTarget::Data`, `WriteTarget::Index`, trace entry, dry-run data write, or language transaction effect.
- [ ] Delete `resume_completion` only after tests prove no post-store file-publish window remains.
- [ ] Replace tests that rewind `marrow.catalog.json` with tests that:
  - inject rollback before transaction commit and prove catalog/data/index/meta are unchanged;
  - commit activation and prove catalog rows, epoch, source digest, and data changes all advance together;
  - dry-run/trace output never reports catalog rows as normal data writes;
  - mutate store commit metadata or catalog rows and prove apply fails closed.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-evolve \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow-run evolution_apply --features native

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-evolve \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow --test evolve_cli --features native
```

**Blocking review criteria:** no file-publish resume path left alive, no catalog epoch advancement without catalog rows, no proposal digest comparison that ignores row contents, no user-visible language write surface for catalog replacement.

## Lane 6: Backup And Restore Carry Catalog Rows

**Files:**
- Modify: `crates/marrow-store/src/backup.rs`
- Modify: `crates/marrow-store/src/tree.rs`
- Modify: `crates/marrow/src/backup/mod.rs`
- Modify: `crates/marrow/src/backup/archive.rs`
- Modify: `crates/marrow/src/backup/create.rs`
- Modify: `crates/marrow/src/backup/restore.rs`
- Modify: `crates/marrow/tests/backup_cli.rs`

**Intent:** A backup of a store with an engine-resident catalog must be self-contained for restore.

- [ ] Extend the backup format version and manifest to include the accepted catalog fingerprint and catalog row count/checksum.
- [ ] Stream catalog rows separately from data cells, or add a typed catalog section before the data section. Do not mix catalog rows into data-family backup cells.
- [ ] Restore catalog rows, data cells, rebuilt indexes, engine profile, catalog epoch, and commit metadata in one transaction.
- [ ] Remove activation-window restore semantics that depended on a behind JSON file.
- [ ] Add tests proving:
  - backup/restore round trips catalog rows and data;
  - restore rejects a manifest whose catalog fingerprint disagrees with rows;
  - restore rollback leaves target empty after catalog-row corruption;
  - restored store can run immediately without `evolve apply` resume;
  - backup data-cell streams still contain only user data cells, never catalog rows;
  - orphan data checks still reject undeclared store/member IDs under the restored catalog.
- [ ] Run focused checks:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-backup \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow --test backup_cli --features native
```

**Blocking review criteria:** no backup that records only catalog epoch/digest without rows, no restore path that reconstructs catalog identity from source spelling, no catalog rows framed as language data cells.

## Lane 7: Delete JSON Production Surface

**Files:**
- Modify: `crates/marrow-check/src/lib.rs`
- Modify: `crates/marrow-check/src/catalog.rs`
- Modify: `crates/marrow-project/src/lib.rs`
- Modify: `crates/marrow-project/tests/config.rs`
- Modify: `crates/marrow/tests/run_cli.rs`
- Modify: `crates/marrow/tests/evolve_cli.rs`
- Modify: `crates/marrow/tests/usage_cli.rs`
- Modify: `docs/project-config.md`
- Modify: `docs/data-evolution.md`
- Modify: `docs/language/resources-and-storage.md`

**Intent:** Remove obsolete JSON file writer/reader paths and tests that depend on rejected behavior.

- [ ] Delete `write_accepted_catalog` and the filesystem atomic-write tests.
- [ ] Delete direct `acceptedCatalog` config support if the approved decision retires it completely. If migration/import/export tooling is approved, keep that option outside production checking/running/applying.
- [ ] Delete CLI tests that assert `marrow.catalog.json` appears or remains unchanged; replace them with store-catalog assertions.
- [ ] Keep `the_catalog_command_is_absent` unless a separate tooling decision adds catalog inspection.
- [ ] Add an absence scan for language/runtime catalog exposure:

```sh
rg -n "catalog_snapshot|read_catalog_snapshot|replace_catalog_snapshot|CatalogSnapshot" \
  /Users/scottwilliams/Dev/marrow/crates/marrow-run/src \
  /Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_data.rs \
  /Users/scottwilliams/Dev/marrow/crates/marrow-run/src/stdlib
```

- [ ] Every remaining hit must be activation plumbing or a test-only assertion. No normal expression evaluation, stdlib function, or data CLI operation may expose catalog rows.
- [ ] Scan for stale strings:

```sh
rg -n "acceptedCatalog|marrow\\.catalog\\.json|write_accepted_catalog|read_accepted_catalog|resume_completion" \
  /Users/scottwilliams/Dev/marrow/crates /Users/scottwilliams/Dev/marrow/docs
```

- [ ] Every remaining hit must be migration/export-only, test fixture text for that tool, or a documented error explaining obsolete input.

**Blocking review criteria:** no production fallback branch, no compatibility shim preserving JSON as hidden authority, no comments describing old JSON history, no catalog table access from normal language database operations.

## Lane 8: Full Gates And Adversarial Review

**Files:**
- Review all changed files from Lanes 0-7.

- [ ] Run formatter:

```sh
cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --check
```

- [ ] Run focused test groups again in a fresh target dir:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-final-focused \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  -p marrow-catalog -p marrow-project -p marrow-check -p marrow-store -p marrow-run -p marrow \
  --features native catalog
```

- [ ] Run full test suite:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-final \
cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all-features
```

- [ ] Run clippy:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/marrow-engine-catalog-clippy \
cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml \
  --all-targets --all-features -- -D warnings
```

- [ ] Confirm no `unsafe` was added:

```sh
rg -n "\\bunsafe\\b" /Users/scottwilliams/Dev/marrow/crates
```

- [ ] Run absence scan for old production JSON authority:

```sh
rg -n "acceptedCatalog|marrow\\.catalog\\.json|write_accepted_catalog|read_accepted_catalog|resume_completion" \
  /Users/scottwilliams/Dev/marrow/crates
```

- [ ] Run absence scan for catalog table exposure through language/database APIs:

```sh
rg -n "catalog_snapshot|read_catalog_snapshot|replace_catalog_snapshot|CatalogSnapshot" \
  /Users/scottwilliams/Dev/marrow/crates/marrow-run/src \
  /Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_data.rs \
  /Users/scottwilliams/Dev/marrow/crates/marrow-run/src/stdlib
```

- [ ] Any remaining hit must be reviewed as activation-internal. There must be no syntax, stdlib, `CheckedSavedPlace`, `DataAddress`, `IndexAddress`, dry-run, trace, or data CLI path that treats catalog rows as user data.

- [ ] Review phase:
  - soundness reviewer must try to break catalog/data atomicity, backup restore, stale store/source mismatch, row corruption, and attempts to access catalog rows through normal language saved-data operations.
  - idiom/spec reviewer must inspect touched Rust for duplicate catalog models, oversized dispatchers, raw string encodings outside row codecs, stale comments, fallback JSON paths, and accidental catalog exposure in runtime/data APIs.

## Integration Notes

- Execute each lane in an isolated worktree with its own outside-repo `CARGO_TARGET_DIR`.
- Sequence lanes that touch `crates/marrow-check/src/catalog.rs`, `crates/marrow-run/src/evolution/apply.rs`, or `crates/marrow/src/cmd_evolve/mod.rs`; these are semantic hotspots.
- Storage table work and docs can start file-disjoint, but integrate storage before checker/runtime lanes.
- Rebase on current `main` immediately before integration, then fast-forward or cherry-pick the reviewed lane commit.
- A lane is not done until it reports changed files, focused gate output, reviewer verdicts, and an absence scan for old JSON production authority in its owned area.
