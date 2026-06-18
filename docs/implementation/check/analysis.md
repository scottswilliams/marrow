# Analysis & Tooling Facts

The read-only, transport-free surface that editors, the CLI, and backup/restore
consume on top of the checker. It owns no semantics: it walks the parse and
facts the checker already built and the keys the store already holds, so editor
and CLI views cannot drift from the checked program.

Two halves live in `crates/marrow-check/src`:

- **`analysis`** runs the IDE-grade pipeline (discover, overlay-or-disk read, parse, check source roots plus configured tests) into an `AnalysisSnapshot` that retains every parse, error files included, and answers cursor queries (`type_at`/`scope_at`) by reconstructing the checker's lexical scope without emitting diagnostics.
- **`tooling`** turns a `CheckedProgram` plus a `TreeStore` into typed saved-data facts: schema-validated path queries, paged child/walk traversal, and integrity verdicts.

`CheckedProgram` also exposes the static entry footprint surface built from
checked facts: `effect_closure`, `entry_footprints`, `entry_cost_shapes`, and
`entry_store_open_mode`. These queries expand lowered direct callee refs, not
source names, and report typed store/index ids plus the
`write_effects_reachable` bit. Store-open classification also stays
write-capable for first-run catalogs, pending catalog proposals, and reachable
transaction blocks.

## Analysis pipeline

`analysis.rs` assembles the snapshot in two passes: pass 1 parses all files and builds the project-wide module set, saved-root owner set, and the single deferred script; pass 2 resolves imports against the full set. Ownership and uniqueness (module name, saved root, at-most-one module-less script) are therefore decided on project-wide counts, not first-seen order. A parse-error file contributes no checked module but stays in the snapshot so editor tooling still works on broken buffers; the program is best-effort, not all-or-nothing.

`cursor.rs` is checker-faithful by construction: it replays the checker's own
binding primitives (`file_prelude`, `for_frame`, `local_binding`, `bind`,
`resolve_type`, `infer_type`, `for_each_child_expr`), so the reconstructed scope
cannot drift from the one the checker builds. A binding at or after the cursor
offset is outside the scope at that offset; the tightest covering expression
wins.

## Analysis API contract

The analysis API contract consumed by `marrow-lsp` is read-only, query-native,
snapshot-scoped, and version-aware. It exposes checker facts and
checker-faithful derived views; it does not parse language structure a second
time, infer facts from diagnostic prose, or open, repair, or create stores
during ordinary check. These public surfaces recompute from the checked program
or snapshot:

- `AnalysisSnapshot::sites_for(catalog_id)` filters the snapshot's `UseSite`
  table, which is built by one post-lowering walk over runtime-lowered module
  constants, function bodies, and evolve transform bodies. Use sites are keyed
  by accepted or proposal catalog ids and typed as saved roots, resource
  members, store indexes, enums, or enum members.
- `CheckedFacts::store_indices` carries `StoreIndexFact::usage` as a
  `StoreIndexUsageBitmap`; every current index fact reports no observed
  read/write use.
- `CheckedProgram::entry_cost_shapes` reports distinct static store/index
  operation shapes per public entry from the same lowered call graph and direct
  effects as `entry_footprints`. It is a model-audit surface, not a runtime
  multiplicity counter: repeated reads of the same saved member are one point
  read shape, and a counted index branch is one range-scan shape.
- `CheckedProgram::effect_closure`, `entry_footprints`,
  `entry_store_open_mode`, and `write_effects_reachable` provide the transitive
  checked-fact view for editor and tooling classification. They expand lowered
  direct callees and report typed store/index ids, not source spellings.
- `BindingIndex::rename_action` returns source edits plus a canonical
  `evolve rename` fragment for saved-data-backed definitions, so editor callers
  do not synthesize catalog paths or formatter output themselves.
- `BindingIndex::parameter_definition` maps a parameter definition or use back
  to its checked `FunctionFact`, `LocalFact`, and parameter ordinal. Editor
  callers use that identity to join token-tight source spans to signature facts
  without re-parsing parameter declarations.
- `CheckedProgram::checked_read_only_expression` parses and checks an injected
  expression against one checked module, rejects writes, host effects, and
  unindexed saved collection lookups with source-level diagnostic codes, and
  returns a `CheckedReadOnlyExpression` handle. Runtime evaluation uses
  `marrow_run::evaluate_checked_read_only_expression`, which reuses the checked
  lowered expression and the production evaluator.
- `evolution::evolution_preview(snapshot, backup)` returns a `WitnessFactSet`.
  With no backup it is schema-only and marks the live-store path deferred; with
  a backup path it streams archive cells to add bounded count and sample facts.
  It never opens a live store.
- Ordinary `marrow check` reads each source file through the analysis pipeline
  and reads the fixed `marrow.catalog.json` artifact when present. It does not
  open, repair, or create the native store.

## Tooling facts

Path resolution is the single chokepoint: `resolve_query_steps` validates source-text or wire segments against a checked place's identity keys and member tree into a `StorageDataQuery` (physical store `CatalogId`, identity keys, data path), emitting typed `QueryError` on malformity. `ToolingError` keeps request-malformity (`Query`) distinct from store faults (`Store`); a missing or malformed checked catalog id stays `StoreError::Corruption` on purpose. Callers match variants, never prose.

`shape.rs::classify_data_path` is the one member-tree shape owner, so the walk cursor's value-position test and integrity orphan detection share a single definition of "declared value path." Every walk and child listing pages with explicit limits, resume cursors, and truncated flags; counts use `checked_add` into `StoreError::LimitExceeded`. Integrity separates declared values (decode, key-type, enum-membership, and canonical identity referent checks against schema and catalog), declared-shape completeness (accepted required fields on existing records and keyed entries), and orphan cells (data under a root/shape/member the schema no longer declares, or under a record identity with no node cell), each a typed `IntegrityProblem` with a stable code.

Stamped roots, value reads, and child listings wrap their existing readers in one
`TreeStore::read_snapshot()` guard and return `StampedData<T>`. The stamp keeps
the physical store identity, catalog digest, optional `DataCommitStamp`, and
checked program source digest separate, so callers can mark stale data without
guessing whether a difference came from the store or the editor snapshot.
`marrow data roots|get --format json|jsonl` render this as `store_snapshot`.
Multi-pass commands and lower-level tooling tests still call the un-stamped
reader primitives under their own broader snapshot.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/analysis.rs` | Two-pass IDE analysis core: discover + overlay + parse + check into `AnalysisSnapshot`, enforce module/script/root-owner uniqueness, run the shared checker tail, compute test-resolution suppression, and build the use-site table. |
| `crates/marrow-check/src/program.rs` | Checked-program artifact plus analysis queries for effect closure, per-entry durable footprints, entry cost shape, entry store-open mode, and checked read-only expressions. |
| `crates/marrow-check/src/analysis/cursor.rs` | Cursor `type_at`/`scope_at`: replay the checker's binding primitives to rebuild lexical scope, infer the tightest covering expression; records no diagnostics. |
| `crates/marrow-check/src/evolution/preview.rs` | Schema-only and backup-backed `WitnessFactSet` preview facts for tooling. |
| `crates/marrow-check/src/tooling/mod.rs` | Tooling facade: re-exports the data/integrity API; defines `ToolingError` (Query vs Store). |
| `crates/marrow-check/src/tooling/data/mod.rs` | Data tooling root and shared value types (`DataQuery`, `DataChild`, `DataEntry`, `DataWalkPage`, `DataReadResult`, `DataRecord`, `StampedData`, `DataSnapshotStamp`, `DataCommitStamp`, `KeyMismatch`, `MAX_PREVIEW_ITEMS`). |
| `crates/marrow-check/src/tooling/data/query.rs` | Path resolution: walk wire/source segments into a `StorageDataQuery` with typed `QueryError`; `data_query_under_prefix` containment. |
| `crates/marrow-check/src/tooling/data/query_error.rs` | The `QueryError` enum (client-facing request errors) and `MemberFlavor`, with render-only `Display`. |
| `crates/marrow-check/src/tooling/data/shape.rs` | The single member-tree shape classifier `classify_data_path` and its consumers (walk-cursor value test, integrity orphan detection). |
| `crates/marrow-check/src/tooling/data/record_nav.rs` | Arity-aware record-child navigation for tooling scans, so partial identity prefixes only surface when an exact declared-arity record exists below them. |
| `crates/marrow-check/src/tooling/data/read.rs` | `read_data_query`: resolve one query to its payload and `DataPresence` (Absent/ValueOnly/ChildrenOnly). |
| `crates/marrow-check/src/tooling/data/children.rs` | Child listing: classify a path into roots/record-children/members/key-children/leaf; return typed next segments and page keyed scans with a resume cursor. |
| `crates/marrow-check/src/tooling/data/walk.rs` | `walk_data`: paged, filter-prefixed, cursor-resumable depth-first walk of leaf values; emits `DataWalkPage` with a next cursor. |
| `crates/marrow-check/src/tooling/data/traversal.rs` | Full saved-record traversal: recurse exact-arity identity nodes and member trees, emit a `DataRecord` per stored leaf or a record identity for declared-shape checks; backs counts, roots, and integrity. |
| `crates/marrow-check/src/tooling/data/render.rs` | Path/key rendering helpers (catalog-id to source name, canonical `SavedKey` text). |
| `crates/marrow-check/src/tooling/integrity.rs` | Integrity verdicts: per-value decode/key-type/enum-member checks, identity referent-existence verdicts, required-field completeness for existing records/keyed entries, and orphan classification as typed `IntegrityProblem` with stable codes. |
| `crates/marrow-check/src/test_support.rs` | Feature-gated test support fact-lookup helpers; not in normal or release builds. |

## Key types

- `AnalysisSnapshot` / `AnalyzedFile` (`analysis.rs`) — the IDE view: report + best-effort `CheckedProgram` + every parsed file, error files retained.
- `UseSite` / `UseSiteKind` (`analysis.rs`) — catalog-id references in checked
  bodies, built from lowered expressions rather than source spelling.
- `CheckedReadOnlyExpression` (`program.rs`) — a source-digest-bound checked
  expression handle for runtime point evaluation.
- `WitnessFactSet` (`evolution/preview.rs`) — schema and optional backup cell
  facts for evolution preview tooling, with live-store preview explicitly
  deferred.
- `DataQuery` / `StorageDataQuery` (`tooling/data/mod.rs`, `query.rs`) — a resolved, schema-validated path; public display form vs crate-internal physical store form.
- `QueryError` / `ToolingError` (`query_error.rs`, `tooling/mod.rs`) — typed request malformity vs store faults.
- `DataRecord` / `DataPresence` / `DataWalkPage` / `DataChildrenPage` (`tooling/data/mod.rs`) — the paged data facts carrying truncation and resume cursors.
- `StampedData` / `DataSnapshotStamp` / `DataCommitStamp` / `DataReadResult` (`tooling/data/mod.rs`) — a value read under one store snapshot plus typed store UID, catalog digest, optional commit stamp, and checked-program source digest.
- `IntegrityProblem` / `IntegrityOutcome` (`integrity.rs`) — a typed finding implementing `Diagnose`, tagged stored-value vs structure/orphan findings, with catalog/key identity attached to incomplete data and dangling identity references.

## Entry points

- `analyze_source_project` is crate-internal (`pub(crate)`); the public entry is
  `analyze_project`. Both take the accepted catalog as an
  `Option<&CatalogMetadata>` input the caller supplies. The convenience
  `check_project` binds no accepted catalog (the first-run shape);
  `check_project_with_catalog` takes the committed `marrow.catalog.json`
  artifact. The checker has no store-open fallback.

## Read next

- `analysis.rs` → `analyze_source_project` — two-pass assembly, ownership rules, and the shared checker tail that defines a `CheckedProgram`.
- `tooling/data/query.rs` → `resolve_query_steps` — the single path-resolution authority and origin of every `QueryError`.
- `tooling/data/walk.rs` → `walk_data` — the most intricate fact:
  cursor-resumable leaf walk across identity and member-key levels.
- `tooling/data/shape.rs` → `classify_data_path` — the shared shape owner keeping walk and integrity from diverging.
- `analysis/cursor.rs` → `type_at` / `scope_at` — checker-faithful cursor queries.
