# Analysis & Tooling Facts

The read-only, transport-free surface that editors, the CLI, and backup/restore
consume on top of the checker. It owns no semantics: it walks the parse and
facts the checker already built and the keys the store already holds, so editor
and CLI views cannot drift from the checked program.

Two halves live in `crates/marrow-check/src`:

- **`analysis`** runs the IDE-grade pipeline (discover, overlay-or-disk read, parse, check source roots plus configured tests) into an `AnalysisSnapshot` that retains every parse, error files included, and answers cursor lookups (`type_at`/`scope_at`) by reconstructing the checker's lexical scope without emitting diagnostics.
- **`tooling`** turns a `CheckedProgram` plus, where live data is needed, a `TreeStore` into typed saved-data facts: schema-validated path resolution, schema-declared child facts, paged child/walk traversal, and integrity verdicts.

`CheckedProgram` also exposes the static entry footprint surface built from
checked facts: `effect_closure`, `entry_footprints`, `entry_cost_shapes`, and
`entry_run_facts`. `entry_run_facts` is the single per-entry carrier for the
runtime footprint, cost shape, and store-open mode used by run tooling. These
APIs expand lowered direct callee refs, not source names, and report typed
store/index ids plus the `write_effects_reachable` bit. Store-open
classification also stays write-capable for first-run catalogs, pending catalog
proposals, and reachable transaction blocks.

## Analysis pipeline

`analysis.rs` assembles the snapshot in two passes: pass 1 parses all files and builds the project-wide module set, saved-root owner set, and the single deferred script; pass 2 resolves imports against the full set. Ownership and uniqueness (module name, saved root, at-most-one module-less script) are therefore decided on project-wide counts, not first-seen order. A parse-error file contributes no checked module but stays in the snapshot so editor tooling still works on broken buffers; the program is best-effort, not all-or-nothing.

`cursor.rs` is checker-faithful by construction: it replays the checker's own
binding primitives (`file_prelude`, `for_frame`, `local_binding`, `bind`,
`resolve_type`, `infer_type`, `for_each_child_expr`), so the reconstructed scope
cannot drift from the one the checker builds. A binding at or after the cursor
offset is outside the scope at that offset; the tightest covering expression
wins.

## Analysis API contract

The analysis API contract consumed by `marrow-lsp` is read-only and
snapshot-scoped. `AnalysisIdentity` is the source/config content identity;
catalog-bound views must not treat it as a catalog or stable-ABI cache key. The
API exposes checker facts and checker-faithful derived views; it does not parse
language structure a second time, infer facts from diagnostic prose, or open,
repair, or create stores during ordinary check. These public surfaces recompute
from the checked program or snapshot:

- `AnalysisSnapshot::sites_for(catalog_id)` filters the snapshot's `UseSite`
  table, which is built from two checker-owned sources: lowered catalog-bearing
  expressions in module constants, function bodies, and evolve transform
  bodies, plus checker-resolved enum type annotations from analyzed source
  and configured test files. Use sites are keyed by accepted or proposal catalog
  ids and typed as saved roots, resource members, store indexes, enums, or enum
  members.
- `tooling::identity_type_annotations(snapshot, file)` returns token-tight
  spans for checked `Id(^root)` type annotations in the requested analyzed
  source file. It walks parsed type annotations, resolves them through the
  checker, requires the store root to exist in the checked program, and recurses
  through `sequence[...]` annotations, so editor callers do not classify
  identity type constructors from token spelling alone.
- `tooling::source_type_annotation_cursor_fact_at(snapshot, file, offset)`
  returns the parsed source type-annotation span under `offset`, including
  declaration and checked-body annotations retained in broken-buffer snapshots.
  The fact carries source text and span only; resolved enum/resource/catalog
  identity stays in the use-site, declaration, binding, and hover facts that own
  those semantics.
- `tooling::source_semantic_token_facts(source, lexed, parsed)` returns
  syntax/parse-only source-token facts with token-tight `SourceSpan`,
  Marrow-owned `SourceSemanticTokenRole`, and typed presentation modifiers.
  This stale-buffer-safe API owns syntax baseline roles, parser declaration
  roles, saved-root sigil/name facts, and context-free intrinsic callable
  roles; it accepts no checked analysis or binding facts.
- `tooling::source_semantic_token_facts_for_file(snapshot, binding_index,
  file)` returns snapshot-bound checked source-token facts for one analyzed
  file, or `None` when the file is not in the snapshot. It binds source text,
  parse, file path, checked callable facts, binding-index reference roles,
  module-const readonly roles, and checked `Id(^root)` type-constructor facts
  coherently through `AnalysisSnapshot`. Transport callers map these roles into
  their own token legends; they do not reclassify source tokens locally.
- `AnalysisSnapshot::catalog_declarations()` returns catalog-owned
  declarations keyed by catalog id. Each `CatalogDeclaration` carries the source
  file, exact declaration-name span, catalog id, `CatalogEntryKind`, and source
  name for resources, stores, resource members, store indexes, enums, and enum
  members. `catalog_declaration(catalog_id)` is the direct lookup for editor
  navigation, so LSP callers do not reconstruct catalog paths or proposal ids.
- `tooling::source_symbols(snapshot)` returns flat project/workspace symbol
  facts with Marrow-owned kind, file, span, display name, and container. It joins
  checked functions and constants with catalog-backed resources, stores, indexes,
  members, enums, and enum members so editor callers only translate the typed
  kind to their transport's symbol enum.
- `tooling::source_symbols_matching(snapshot, search_text)` applies
  checker-owned workspace symbol search over those facts, matching names and
  qualified container paths with deterministic ranking.
- `tooling::source_symbol_docs_at(snapshot, binding_index, file, offset)`
  returns docs attached to the source symbol resolved by the binding index.
- `tooling::source_saved_root_cursor_fact_at(snapshot, file, offset)` returns
  the parsed source saved-root cursor span under `offset`, including store
  declarations, surface targets, type annotations, expression roots, and evolve
  targets. The fact carries the root segment and source role, but no catalog
  identity; catalog-backed definition and references continue to use
  `AnalysisSnapshot` use-site and declaration tables when resolution succeeds.
- `tooling::source_callable_hover_fact_at(snapshot, binding_index, file,
  offset)` returns callable hover facts for user functions, parameters, module
  constants, and intrinsic callable leaves. Intrinsic facts carry the canonical
  callable signature after file-context import expansion, so transport callers
  do not classify default-library calls from token spelling.
- `tooling::source_operator_hover_fact_at(snapshot, file, offset)` returns a
  language-operator hover fact only when the checked cursor position is an
  operator expression. Keyword path/declaration positions and other
  non-expression uses return no fact, including in broken-buffer snapshots.
- `tooling::document_symbols(file, source)` returns parsed document-outline
  facts with Marrow-owned kind, detail, full span, selection span, and nested
  children. It accepts a parsed `SourceFile` plus source text, so editor callers
  can keep outline behavior for broken open buffers without rechecking.
- `AnalysisSnapshot::surface_read_operations()` iterates snapshot-bound
  `SurfaceReadOperationAnalysis` views. Each view carries the source file,
  checked `SurfaceFact`, and checked `SurfaceReadOperationFact`, so editor
  consumers can inspect declared surface operations without walking source
  syntax or mistaking source/config identity for a catalog-bound surface
  version. Stable surfaces can also render a checker-owned
  `SurfaceReadOperationDescriptor`; source-only surfaces cannot.
- `AnalysisSnapshot::surface_update_operations()` iterates snapshot-bound
  `SurfaceUpdateOperationAnalysis` views for stable-surface candidates with a
  non-empty `update` list. The descriptor is checker-owned, uses
  `surface.update.v1`, carries `non_empty_patch` semantics, and is suppressed
  for source-only surfaces.
- `AnalysisSnapshot::surface_create_operations()` iterates snapshot-bound
  `SurfaceCreateOperationAnalysis` views for stable-surface candidates with a
  non-empty `create` list. The descriptor is checker-owned, uses
  `surface.create.v1`, carries exact declared-body, identity-policy, and
  reject-existing semantics, and is suppressed for source-only surfaces.
- `AnalysisSnapshot::surface_delete_operations()` iterates snapshot-bound
  `SurfaceDeleteOperationAnalysis` views for stable-surface candidates with a
  `delete` declaration. The descriptor is checker-owned, uses
  `surface.delete.v1`, carries reject-absent full-subtree semantics, and is
  suppressed for source-only surfaces.
- `AnalysisSnapshot::surface_action_operations()` iterates snapshot-bound
  `SurfaceActionOperationAnalysis` views for declared surface actions. The
  descriptor is checker-owned, reuses `entry.invoke.v1` identity, parameter
  shapes, and return shape from the resolved public function, and is suppressed
  for source-only surfaces.
- `AnalysisSnapshot::surface_computed_read_operations()` iterates
  snapshot-bound `SurfaceComputedReadOperationAnalysis` views for declared
  computed reads. The descriptor is checker-owned, uses
  `surface.computed_read.v1`, reuses shared entry parameter/result shapes,
  carries the computed read's checked cost shape, and is suppressed for
  source-only surfaces.
- `CheckedFacts::store_indices` carries `StoreIndexFact::usage` as a
  `StoreIndexUsageBitmap`; every current index fact reports no observed
  read/write use.
- `CheckedProgram::entry_cost_shapes` reports distinct static store/index
  operation shapes per public entry from the same lowered call graph and direct
  effects as `entry_footprints`. It is a model-audit surface, not a runtime
  multiplicity counter: repeated reads of the same saved member are one point
  read shape, and a counted index branch is one range-scan shape.
- `CheckedProgram::effect_closure`, `entry_footprints`,
  `entry_cost_shapes`, `entry_run_facts`, and `write_effects_reachable` provide
  the transitive checked-fact view for editor and tooling classification.
  `entry_run_facts` carries the canonical footprint, cost shape, and store-open
  mode for one entry. These APIs expand lowered direct callees and carry typed
  `StoreId`/`StoreIndexId`. The CLI JSON projection of entry footprints renders
  those ids as canonical structural paths (`module::^root`,
  `module::^root::index`) via `store_structural_path`/
  `store_index_structural_path`, so footprint identities are freeze-independent
  and join to the catalog by path.
- `BindingIndex::rename_action` returns source edits plus a canonical
  `evolve rename` fragment for saved-data-backed definitions, so editor callers
  do not synthesize catalog paths or formatter output themselves. Imported
  module references remain navigation facts only because imports have no alias
  syntax to edit independently of the module path.
- `BindingIndex::parameter_definition` maps a parameter definition or use back
  to its checked `FunctionFact`, `LocalFact`, and parameter ordinal. Editor
  callers use that identity to join token-tight source spans to signature facts
  without re-parsing parameter declarations.
- `tooling::intrinsic_completion_callables()` returns the bare editor-callable
  intrinsic signatures from the checker-owned builtin and scalar-conversion
  vocabularies plus `Id` and `Error`. It excludes namespace-qualified standard
  library operations and removed builtins.
- `tooling::source_saved_root_completion_fact(program)` returns project-wide
  saved-root completion candidates with root segment, owning module, resource
  name, and declaration docs. Editor callers render protocol items from those
  semantic fields instead of walking module stores.
- `tooling::source_saved_path_completion_fact_at(...)` returns a checked
  saved-path completion context for an active saved receiver, including the
  receiver span, checked root identity, completed key/layer segments, and
  declared child facts. Editor callers do not parse or retain raw saved-path
  strings.
- `tooling::source_namespace_completion_fact(...)` returns editor namespace
  candidates for project modules, enum members, and the standard-library
  namespace. The std root carries first-seen module names from the canonical
  stdlib table; known std modules carry `CallableSignature` values for their
  operations. `source_namespace_completion_file_fact(...)` remains project/file
  namespace only for MCP.
- `tooling::source_completion_fact(...)` returns the `source.completion.v1`
  source-completion profile for one checked file and cursor position. The fact
  carries protocol-free item kinds, labels, detail text, and docs for saved
  paths, namespace members, type positions, typed scope values, module aliases,
  raw `std`, current-module declarations, expected enum members, and intrinsic
  callables. Editor and tool callers map those semantic items into their own
  transport shapes instead of reconstructing completion candidates locally.
- `CheckedProgram::checked_read_only_expression` parses and checks an injected
  expression against one checked module, rejects writes, host effects, and
  unindexed saved collection lookups with source-level diagnostic codes, and
  returns a `CheckedReadOnlyExpression` handle. Runtime evaluation uses
  `marrow_run::evaluate_checked_read_only_expression`, which reuses the checked
  lowered expression and the production evaluator.
- `AnalysisSnapshot::checked_debug_expression` adds a checked source location
  and source-digest binding for debugger watches and conditionals. Its
  `CheckedDebugExpression::data_access` fact reports whether the admitted
  expression is local-only or requires saved data, using the same direct and
  transitive read-only effects that enforce the debug-expression diagnostics.
- `CheckedRuntimeProgram::stop_points()` returns snapshot-scoped
  `RuntimeStopPoint` facts for the checked statement spans the evaluator can
  report through `StepHook::before_statement`. Each point carries a `FileId` and
  `SourceSpan`; callers map the file id back through the runtime program rather
  than treating rendered paths as semantic identity. Nested statement bodies are
  included. Source headers that are not separate checked statements are not
  independent stop points.
- `evolution::evolution_preview(snapshot, backup)` returns a `WitnessFactSet`.
  With no backup it is schema-only and marks the live-store path deferred; with
  a backup path it streams archive cells to add bounded count and sample facts.
  It never opens a live store.
- Ordinary `marrow check` reads each source file through the analysis pipeline
  and binds the live store snapshot when one is present and readable, falling back
  to the committed `marrow.lock` projection for first-run adoption. The read is
  read-only: it does not open the store for repair or create one.

Catalog navigation spans are owned upstream of `analysis.rs`. Syntax carries
token-tight spans for declaration names, name-expression segments, field
segments, saved roots, and match-arm member path segments. Lowering copies those
spans into checked saved places, layers, terminals, and enum-member references;
checked facts copy declaration name spans. The analysis use-site and declaration
tables consume those exact spans for lowered expressions, saved paths, match
arms, and declarations. Type annotations currently carry a whole-annotation span,
so enum annotation use-sites recover the resolved enum leaf inside that bounded
annotation text. No use-site falls back to whole expressions, calls, match arms,
layers, or broad declaration spans.

## Tooling facts

Path resolution is the single chokepoint: `resolve_data_path_steps` validates source-text or wire segments against a checked place's identity keys and member tree into a `StorageDataPath` (physical store `CatalogId`, identity keys, data path), emitting typed `DataPathError` on malformity. The schema-only declared-child surface reuses the same checked saved-place/member ownership and accepts either concrete saved-data segments or source-shape segments with key slots; complete record or layer-entry paths return declared field/layer facts, while partial key prefixes return no schema members because their next children are data keys. Source receiver completions parse the receiver expression at a file/span scope and reuse the checker-owned saved-root address predicate, so scalar keys and composite `Id(^root)` arguments share the same identity semantics; malformed, untypable, partial, or foreign-identity receivers return no schema members. `ToolingError` keeps request malformity (`Path`) distinct from store faults (`Store`); a missing or malformed checked catalog id stays `StoreError::Corruption` on purpose. Callers match variants, never prose.

`shape.rs::classify_data_path` is the one member-tree shape owner, so the walk cursor's value-position test and integrity orphan detection share a single definition of "declared value path." Every walk and child listing pages with explicit limits, resume cursors, and truncated flags; counts use `checked_add` into `StoreError::LimitExceeded`. Integrity separates declared values (decode, key-type, enum-membership, and canonical identity referent checks against schema and catalog), declared-shape completeness (accepted required fields on existing records and keyed entries), and orphan cells (data under a root/shape/member the schema no longer declares, or under a record identity with no node cell), each a typed `IntegrityProblem` with a stable code. `count_orphan_cells` exposes just the orphan count over the same `visit_orphans_in_places` reader, so the source-driven inspection commands (`data stats`, `data dump`) can warn that their schema-view traversal omits cells under undeclared members rather than under-reporting them silently.

Stamped roots, raw value reads, bounded value previews, child listings, and
bounded integrity problem samples wrap their existing readers in one
`TreeStore::read_snapshot()` guard and return `StampedData<T>`. The stamp keeps
the physical store identity, catalog digest, optional `DataCommitStamp`, and
checked program source digest separate, so callers can mark stale data without
guessing whether a difference came from the store or the editor snapshot.
`marrow-json` owns the public integrity sample DTO over those stamped facts,
including the diagnostic envelope and typed incomplete-record or
dangling-reference payloads.
`marrow data roots|get --format json|jsonl` render the stamp as
`store_snapshot`. Multi-pass commands and lower-level tooling tests still call
the un-stamped reader primitives under their own broader snapshot.

Raw/admin reads and preview reads are intentionally separate. `read_data_path`
uses `TreeStore::read_data_value` and returns a full `DebugDataPayload` for
debug/admin byte inspection. `preview_data_path` uses
`TreeStore::read_data_value_prefix` after clamping the requested budget, so
preview callers do not materialize a whole saved cell before applying their
budget. Both reads share the same `DataPresence` decision.

`DataValuePreview` is the Marrow-owned bounded display value for saved-data
tooling. Its limit is a pre-marker byte budget for the rendered text, clamped to
`MAX_VALUE_PREVIEW_LIMIT` before any store prefix read. Whenever rendering stops
because the text budget or stored-byte prefix was truncated, the preview appends
the literal marker `...`, sets `truncated: true`, and the text may therefore be
up to three bytes longer than the effective limit. When `truncated` is false the
marker is absent. DTO field `value_truncated` carries the same contract.

`marrow-json::saved_data` owns the serde DTOs for current saved-data transport
shapes: path segments, keys, child pages, preview read requests/results,
presence, preview budget, and typed path/store errors. `DataReadRequestJson`
accepts an optional `preview_limit` and clamps it to Marrow's maximum when
callers ask for the effective budget. Downstream editor or tool wrappers should
add only transport availability and request-envelope concerns around those DTOs.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/analysis.rs` | Two-pass IDE analysis core: discover + overlay + parse + check into `AnalysisSnapshot`, enforce module/script/root-owner uniqueness, run the shared checker tail, compute test-resolution suppression, and build snapshot-bound use-site and surface-operation views. |
| `crates/marrow-check/src/program.rs` | Checked-program artifact plus analysis APIs for effect closure, per-entry run facts, durable footprints, cost shapes, store-open mode, checked read-only expressions, and runtime statement stop points. |
| `crates/marrow-check/src/analysis/cursor.rs` | Cursor `type_at`/`scope_at`: replay the checker's binding primitives to rebuild lexical scope, infer the tightest covering expression; records no diagnostics. |
| `crates/marrow-check/src/evolution/preview.rs` | Schema-only and backup-backed `WitnessFactSet` preview facts for tooling. |
| `crates/marrow-check/src/tooling/mod.rs` | Tooling facade: re-exports completion, semantic-token, signature, symbol, data, and integrity facts; defines `ToolingError` (Path vs Store). |
| `crates/marrow-check/src/tooling/completion.rs` | Source completion facts for type candidates, project namespace members, standard-library namespace/module members, and saved-root candidates. |
| `crates/marrow-check/src/tooling/semantic_tokens/mod.rs` | Source semantic-token facade: public syntax-only and snapshot-bound checked APIs, fact/role/modifier DTOs, and fact precedence orchestration. |
| `crates/marrow-check/src/tooling/semantic_tokens/syntax.rs` | Syntax baseline roles for lexer tokens and shared token-span/path-segment helpers. |
| `crates/marrow-check/src/tooling/semantic_tokens/declarations.rs` | Parser declaration roles for modules, uses, constants, functions, resources, stores, surfaces, enums, evolve steps, saved roots, resource members, indexes, parameters, and key parameters. |
| `crates/marrow-check/src/tooling/semantic_tokens/callables.rs` | Intrinsic and standard-library callable roles, including snapshot-context import expansion for checked requests. |
| `crates/marrow-check/src/tooling/semantic_tokens/references.rs` | Binding-index reference roles for checked source occurrences and qualified namespace prefixes. |
| `crates/marrow-check/src/tooling/semantic_tokens/identity_annotations.rs` | Checked `Id(^root)` type-constructor roles from identity type annotation facts. |
| `crates/marrow-check/src/tooling/signatures.rs` | Editor callable facts and renderable signature inputs: active/batch callee context re-exports, intrinsic callable signatures, and resource constructors. |
| `crates/marrow-check/src/tooling/symbols.rs` | Source-symbol facts for editor outlines and workspace search: parsed document-outline DTOs plus checked functions/constants and catalog-backed declarations with Marrow-owned kind, display name, file/span, and container ownership. |
| `crates/marrow-check/src/tooling/data/mod.rs` | Data tooling root and shared value types (`ResolvedDataPath`, `DataChild`, `DeclaredDataChild`, `SourceDataPathSegment`, `DataEntry`, `DataWalkPage`, `DataReadResult`, `DataRecord`, `StampedData`, `DataSnapshotStamp`, `DataCommitStamp`, `KeyMismatch`, `MAX_PREVIEW_ITEMS`, `DEFAULT_VALUE_PREVIEW_LIMIT`, `MAX_VALUE_PREVIEW_LIMIT`). |
| `crates/marrow-check/src/tooling/data/declared.rs` | Schema-only declared child lookup for saved source paths and concrete data paths through the shared checked path walk; opens no store. |
| `crates/marrow-check/src/tooling/data/path.rs` | Shared checked saved-path walk plus `StorageDataPath` conversion for wire/source segments, with typed `DataPathError`; `data_path_under_prefix` containment. `inspection_root_place` retypes leaf members to the accepted-catalog leaf so inspection renders by the epoch data was written under, not drifted source. |
| `crates/marrow-check/src/tooling/data/path_error.rs` | The `DataPathError` enum (client-facing request errors) and `MemberFlavor`, with render-only `Display`. `resolution_code` separates the schema-resolution variants (unknown saved root or member) — reported as a typed `data.unknown_path` diagnostic — from the malformed/misused variants the boundary reports as a usage error. |
| `crates/marrow-check/src/tooling/data/shape.rs` | The single member-tree shape classifier `classify_data_path` and its consumers (walk-cursor value test, integrity orphan detection). |
| `crates/marrow-check/src/tooling/data/record_nav.rs` | Arity-aware record-child navigation for tooling scans, so partial identity prefixes only surface when an exact declared-arity record exists below them. |
| `crates/marrow-check/src/tooling/data/read.rs` | `read_data_path`: resolve one path to its full raw payload and `DataPresence` (Absent/Exists/ValueOnly/ChildrenOnly); `preview_data_path`: resolve the same path to a bounded `DataValuePreview` through a store prefix read. |
| `crates/marrow-check/src/tooling/data/children.rs` | Child listing: classify a path into roots/record-children/members/key-children/leaf; return typed next segments and page keyed scans with a resume cursor. |
| `crates/marrow-check/src/tooling/data/walk.rs` | `walk_data`: paged, filter-prefixed, cursor-resumable depth-first walk of leaf values; emits `DataWalkPage` with a next cursor. |
| `crates/marrow-check/src/tooling/data/traversal.rs` | Full saved-record traversal: recurse exact-arity identity nodes and member trees, emit a `DataRecord` per stored leaf or a record identity for declared-shape checks; backs counts, roots, and integrity. |
| `crates/marrow-check/src/tooling/data/render.rs` | Path/key rendering helpers (catalog-id to source name, canonical `SavedKey` text). |
| `crates/marrow-check/src/tooling/integrity.rs` | Integrity verdicts: per-value decode/key-type/enum-member checks, identity referent-existence verdicts, required-field completeness for existing records/keyed entries, orphan classification as typed `IntegrityProblem` with stable codes, and the store-completeness gate (`verify_store_completeness`) that runs the store's per-root structural-digest cross-check and the index-completeness cross-check (the entries each declared index owes derived from the data records), failing closed when a family enumerates a state its oracle disagrees with. The committed-lock root witness (`verify_store_roots_against_lock`, taking a present live store and rejecting one presenting fewer roots than the lock records) is the bare half: callers route through the single owner `verify_present_store_lock_roots`, which treats an absent store body as the disposable-store case the write paths seed, never inside the completeness gate. |
| `crates/marrow-check/src/test_support.rs` | Feature-gated test support fact-lookup helpers; not in normal or release builds. |

## Key types

- `AnalysisSnapshot` / `AnalyzedFile` (`analysis.rs`) — the IDE view: report + best-effort `CheckedProgram` + every parsed file, error files retained.
- `UseSite` / `UseSiteKind` (`analysis/catalog_nav.rs`, re-exported from
  `analysis.rs`) — catalog-id references in checked bodies and enum type
  annotations, built from checker-owned facts and token-tight syntax spans
  rather than source spelling.
- `CatalogDeclaration` (`analysis/catalog_nav.rs`, re-exported from
  `analysis.rs`) — catalog-id declarations for editor navigation, keyed with
  `CatalogEntryKind` and exact declaration-name spans.
- `SurfaceReadOperationAnalysis` / `SurfaceComputedReadOperationAnalysis` /
  `SurfaceUpdateOperationAnalysis` (`analysis.rs`) — snapshot-bound views over
  checked surface operations plus their source files, with
  `stable_descriptor()` for accepted-catalog read, computed-read, and
  sparse-update descriptors when the surface is stable.
- `CheckedReadOnlyExpression` (`program.rs`) — a source-digest-bound checked
  expression handle for runtime point evaluation.
- `WitnessFactSet` (`evolution/preview.rs`) — schema and optional backup cell
  facts for evolution preview tooling, with live-store preview explicitly
  deferred.
- `ResolvedDataPath` / `StorageDataPath` (`tooling/data/mod.rs`, `path.rs`) — a resolved, schema-validated path; public display form vs crate-internal physical store form.
- `DataPathError` / `ToolingError` (`path_error.rs`, `tooling/mod.rs`) — typed request malformity vs store faults.
- `DataRecord` / `DataPresence` / `DataWalkPage` / `DataChildrenPage` / `DataValuePreview` (`tooling/data/mod.rs`) — the paged data facts carrying truncation and resume cursors, plus bounded saved-value display text for tooling.
- `StampedData` / `DataSnapshotStamp` / `DataCommitStamp` / `DataReadResult` / `DataPreviewReadResult` (`tooling/data/mod.rs`) — raw and preview value reads under one store snapshot plus typed store UID, catalog digest, optional commit stamp, and checked-program source digest.
- `IntegrityProblem` / `IntegrityOutcome` / `IntegrityProblemSample`
  (`integrity.rs`) — typed findings implementing `Diagnose`, full-report
  outcomes tagged stored-value vs structure/orphan findings, and bounded problem
  samples carrying inspected-item counts plus truncation.

## Entry points

- `analyze_source_project` is crate-internal (`pub(crate)`); the public entry is
  `analyze_project`. Both take the accepted reference as two caller-supplied
  inputs: an `Option<&CatalogMetadata>` store snapshot and an
  `Option<&CatalogLock>` committed projection for first-run adoption. The
  convenience `check_project` binds neither (the first-run shape with no committed
  lock); `check_project_with_catalog` threads a store snapshot. The CLI owns
  store/lock selection and passes the result here; the checker has no store-open
  fallback of its own.

## Read next

- `analysis.rs` → `analyze_source_project` — two-pass assembly, ownership rules, and the shared checker tail that defines a `CheckedProgram`.
- `tooling/data/path.rs` → `resolve_data_path_steps` — the single path-resolution authority and origin of every `DataPathError`.
- `tooling/data/walk.rs` → `walk_data` — the most intricate fact:
  cursor-resumable leaf walk across identity and member-key levels.
- `tooling/data/shape.rs` → `classify_data_path` — the shared shape owner keeping walk and integrity from diverging.
- `analysis/cursor.rs` → `type_at` / `scope_at` — checker-faithful cursor lookups.
