# Marrow v0.1 Research Synthesis

Date: 2026-06-04

This synthesis consolidates the finalized research reports for Lanes 5 through
10, Lane 14A online evolution foundation research, the holistic language audit,
the architecture red-team audit, the current Marrow docs, and the current ADR
packet in `marrow-decisions`, including the unstaged edits to the foundation and
transaction ADRs.

This is a research and planning artifact. It does not implement code, create a
new ADR, or bless any incomplete lane as complete.

## Evidence State

Local state inspected before synthesis:

- the Marrow repository was on the source-native evolution research branch, with
  untracked research output under `docs/roadmap/research/`;
- local `main` and `origin/main` were at `3f7178a`;
- `marrow-decisions` had then-current edits in the foundations and
  storage-engine ADRs.

Unique finalized reports read and integrated into
`marrow/docs/roadmap/research/`:

- Lane 5 resource/store surface:
  `lane-05-resource-store-surface.md`
- Lane 6 catalog identity, presence ledger, and enum member identity:
  `lane-06-catalog-identity-presence-enum-audit.md`
- Lane 7 typed tree-cell store and engine profile:
  `lane-07-typed-tree-cell-store-engine.md`
- Lane 8 checked runtime execution and write planner:
  `lane-08-checked-runtime-execution-and-write-planner.md`
- Lane 9 source-native evolution and activation:
  `lane-09-source-native-evolution-activation.md`
- Lane 10 tooling, backup, restore, protocols, LSP, and data tools:
  `lane-10-tooling-backup-protocols-audit.md`
- Holistic language ergonomics and idiom:
  `holistic-language-ergonomics-and-idiom.md`
- Architecture red-team:
  `marrow-architecture-red-team.md`
- Lane 14A online evolution foundation:
  `lane-14a-online-evolution-foundation.md`

Lane 5 appeared in multiple worktrees with the same content hash and is
integrated once.

Current docs and ADRs reviewed:

- Canonical Marrow docs under `marrow/docs`, with
  particular attention to language, cost model, CLI, data tools, serve protocol,
  backend contract, implementation, data evolution, and roadmap lane docs.
- Accepted and proposed ADRs under
  `marrow-decisions/adr`.

## First Principles

The reports converge on one strong thesis: Marrow should stay a source-native,
typed tree language with built-in durable data, not drift toward SQL, an ORM, a
document-store wrapper, a query planner, or a private database server.

The load-bearing model is:

- Source declares resource trees, stores, functions, transactions, and evolution
  intent.
- The catalog owns durable identity.
- The compiler checks source, catalog, attached saved data, and engine profile
  together.
- The runtime executes checked IR and explicit write plans.
- The store persists typed tree cells over a private ordered-byte engine.
- Tools render compiler/runtime/store facts; they do not rediscover semantics.
- The access path is source. There is no hidden optimizer and no query plan to
  discover beneath ordinary Marrow code.

This foundation is strong. The main risk is not the absence of SQL. The main
risk is letting prototype residue and tool convenience create a second semantic
model through path strings, raw bytes, stale ADR prose, oversized Rust
dispatchers, or debug surfaces that accidentally harden into product APIs.

## Decisions To Keep

### Keep The Five-Layer Architecture

Keep the source/catalog/compiler/runtime/engine split. Every report found this
more coherent than a SQL table layer, a raw KV surface, or a conventional
migration framework. The recent ADR edits that say "the access path is source"
are correct and should be treated as architectural law.

### Keep Resource/Store Separation

Keep `resource` as reusable typed tree shape and `store ^root(...)` as durable
root/identity owner. This is Marrow-native and avoids table/model coupling.
Store-owned indexes and store-owned identities fit this model.

### Keep `Id(^store)` As The Only Canonical Identity Surface

Keep durable identity as store plus key, exposed as `Id(^store)`. Do not revive
resource-owned identity such as `Book::Id`. A resource shape may have multiple
stores, and the store role is the durable identity boundary.

`Id(^store)` is a type constructor, not a runtime function. It means "the
identity type for this saved store." The spelling is acceptable for v0.1 because
Marrow does not yet have generic type-parameter syntax. If Marrow later gains
type parameters, `Id<^store>` or another explicitly type-level spelling can be
reconsidered. The invariant stays the same: identity belongs to the store, not
the resource shape.

### Keep Invisible Catalog Identity

Keep the accepted generated catalog as the durable ABI. Do not reintroduce
source-visible stable-id annotations such as `@id`, source-name/path-derived
IDs, source-order enum ordinals, or regenerated IDs.

The committed catalog is a good local/source-control-native alternative to a
central schema registry, provided merge UX and digest/identity rules are made
stronger.

### Keep Catalog-Backed Enum Member Storage

Keep enum member identity catalog-backed rather than source-order-based.
Reordering source members must remain non-destructive. Removing or making a
member unselectable over stored data should fail closed.

### Keep Typed Tree Cells Over A Private Ordered-Byte Engine

Keep `TreeStore` as the production store model and redb as the private native
engine for v0.1. This is not "building a database engine from scratch"; it is a
typed semantic layer over an embedded ordered KV engine.

Do not restore public raw backend, raw saved-path, archive replay, or physical
key APIs as production surfaces.

### Keep Checked Runtime Execution And Explicit Write Plans

Keep syntax-free checked runtime artifacts, checked entry calls, checked saved
places, explicit `WritePlan`s, source-structured transactions, and generated
index maintenance derived from checked facts.

Do not reverse into AST execution, raw entry strings, runtime-local saved-path
resolution, or ORM-like lazy loaders.

### Keep Source-Native Evolution

Keep source-native evolution with explicit `rename`, `default`, `transform`, and
`retire`, exact preview witnesses, fail-closed apply, and no migration DSL or
hidden migration ledger.

The research strongly favors source/catalog/data/engine proof over conventional
migration files for Marrow's language-owned data model.

### Keep Staged Online Activation As The Future Foundation

Keep v0.1 strict and single-epoch, but do not make single-epoch fencing the
long-term OLTP architecture. Future Marrow should grow toward multi-epoch online
activation: reads pin snapshot plus catalog epoch, writes run through checked
epoch facts and runtime generations, background jobs backfill bounded chunks,
and a tiny publish step advances readable catalog state after verification.

Compatibility adapters are allowed only inside explicit, bounded windows. They
are generated from source/evolution facts, named, visible in tooling, and deleted
when the window closes. Old writes are rejected unless the compiler proves they
lower to latest-format write plans and maintain every active or building fact.

For key changes, resource reshapes, layout recompiles, and engine moves, use a
shadow-decant workflow when needed: build a new store/layout in chunks, bridge a
bounded set of writes, verify identity/count/checksum facts, publish a small
binding change, then close and purge. This is an activation job, not a raw store
patch or migration script.

### Keep Typed Backup/Restore

Keep backup/restore as typed Marrow artifacts bound to source digest, accepted
catalog epoch, engine profile, layout/codec facts, checksums, and tree-cell data,
with indexes rebuilt or verified on restore.

Do not make raw engine files or raw path/value dumps the portable production
backup contract.

### Keep Debug/Admin Inspection Only If Clearly Scoped

`marrow data` and `marrow serve` can exist only as typed, read-only,
debug/admin inspection surfaces over checked facts. The `debug_data_*` naming is
good. It must not become a production app protocol, sync protocol, generated API,
or raw-path compatibility surface in v0.1.

This does not reject a future production local API. A later `serve` can be a real
SQLite-like local HTTP/IPC surface for local clients, with resources and public
functions projected as URI-like endpoints. That future surface must be generated
or checked from Marrow facts, typed, versioned, bounded, catalog-epoch aware, and
local-first. It must not harden the current raw debug path/value protocol into
the product contract.

### Keep Sparse Presence As A Saved-Path Fact

Keep sparse saved fields, presence proofs, `exists`, optional chaining, and `??`
as address-presence semantics, not general nullability. Do not introduce
implicit nulls.

### Keep Exact Whole-Resource Assignment

Keep whole-resource assignment as exact replacement. This is normal Marrow
semantics, not a footgun requiring a new warning system by default. Code that
wants to preserve sparse fields or keyed children writes those fields directly,
usually inside a transaction.

### Keep Manual References For v0.1

Keep `Id(^store)` fields as typed values, not foreign keys. Do not smuggle joins,
cascades, restrict rules, or existence checks into the store. If referential
integrity becomes a product requirement, it needs a future source-level design.

Dangling references are allowed by default, but they are compiler-visible because
compiler equals data integrity in Marrow. A field typed `Id(^authors)` must hold
an `^authors` identity, and data-attached compiler/integrity flows should be able
to report when the referenced author is absent. That report is an integrity fact,
not an implicit cascade or unconditional write rejection.

### Keep Explicit User-Mode History

History and audit state are modeled explicitly by users as resource fields and
keyed layers. Marrow v0.1 is not a temporal fact database and does not promise an
automatic database-wide audit log.

## Decisions To Refine

### Reconcile ADR And Doc Drift

The docs and implementation are ahead of some accepted ADR prose. Update
existing ADRs in place, not by creating new ADRs, to remove or clarify:

- `Book::Id` alias claims;
- `edit` as source syntax;
- raw inspection that runs without checked source;
- stale defaulted-field or absence-narrowing syntax not present in the language;
- stale `archive.rs` evidence and old raw archive language;
- any wording that implies a query planner, migration DSL, or source-diff
  identity inference.

Also reconcile the current docs' random catalog stable IDs with the physical
encoding ADR's monotonic per-catalog stable ID wording. Entity stable IDs should
be random or otherwise collision-resistant and branch-merge-safe; catalog epochs
and commit IDs can be monotonic.

### Strengthen Catalog Foundations

Refine the catalog implementation and docs:

- use the collision-resistant SHA-256 digest contract for catalog/source/evolution
  fences;
- keep `reserved` as the alias lifecycle state that blocks silent reuse;
- avoid sentinel empty strings for proposal-only catalog IDs;
- make source-order enum helpers private or visibly non-durable;
- add branch-merge fixtures for parallel catalog additions, aliases, enum
  members, indexes, and conflict diagnostics;
- keep the `cat_<32 lowercase hex>` random ID shape unless a future catalog
  format decision deliberately replaces it.

### Refine Presence Ledger Shape

The presence ledger is the right direction, but it should move closer to the ADR
shape:

- one proof source per admitted read;
- explicit proof identity;
- explicit `Discharged` versus `PendingAttachedData` status;
- consumer tests proving runtime, evolution, CLI, LSP, backup, and restore read
  the ledger instead of rediscovering presence.

### Refine Store API Shape

The store boundary is conceptually right, but the Rust/API shape needs
hardening:

- public payload bytes should become typed wrappers or be consciously frozen as
  `Vec<u8>` with precise docs;
- all-child helper APIs should become bounded pages/cursors or be made
  crate-private test conveniences;
- `tree.rs` should be split by invariant before the store API freezes;
- commit metadata docs should include every implemented field, including source
  digest if it remains in code;
- add conformance for pinned snapshot plus open write transaction, or forbid
  that state explicitly;
- reconcile the physical-key ADR namespace with the actual backend contract
  before layout epoch freeze.

### Refine Runtime Hardening

Runtime architecture is good, but hardening should narrow raw-looking internal
helpers:

- rename or narrow `DataAddress::raw`;
- replace string member paths with checked member references where possible;
- move maintenance/evolution/restore capabilities toward typed tokens or
  typestate-like state;
- add operation-class fixtures for the minimal-plan guarantee;
- ensure commit metadata is stamped at the physical durable commit boundary, not
  per statement or per plan inside a multi-write transaction.

### Refine Evolution Operations

Source-native evolution is strong, but the red-team report found a potential
v0.1 soundness blocker:

- proposal-only `evolve default` or `evolve transform` may discharge as
  activatable while runtime apply cannot address the new member before catalog
  acceptance. This needs a production apply fixture and a real fix or explicit
  rejection.

Other evolution refinements:

- add activation receipts as evidence, not executable migration history;
- make activation job-shaped as the long-term foundation: v0.1 may execute the
  job immediately in one exact transaction, but preview/apply should be modeled
  as a compiler-owned activation job so large future rebuilds/backfills do not
  require a migration-system rewrite;
- preserve the future online protocol in the facts even before it exists:
  preview, start, bridge, backfill, verify, publish, and close are separate
  conceptual phases, while v0.1 may collapse them into exact apply;
- make the normal future compatibility window one old epoch to one new epoch,
  with old clients read-only or rejected unless a compiler-generated write
  adapter is proven;
- require shadow decant, not in-place reinterpretation, for key-shape,
  resource-shape, layout, or engine changes that cannot be proven as ordinary
  backfill;
- split the large discharge kernel by invariant;
- design online activation jobs for large index rebuilds, backfills, and
  transforms as the future chunked/resumable form of the same witness, not as a
  separate migration framework;
- define future compatibility windows instead of ad hoc old-binary exceptions;
- keep source-diff tooling as suggestions only, never identity authority.

### Refine Tooling Around Shared Facts

Tooling is the highest risk for reintroducing a second model. Refine toward a
shared, transport-free tooling facts API that owns:

- typed data-query resolution;
- checked path rendering;
- bounded data previews;
- integrity findings;
- explainable checked operations;
- snapshot/catalog-epoch metadata;
- cursor contracts;
- backup/restore/repair status.

Then make CLI, serve, LSP, backup, restore, and future adapters thin renderers.

### Refine Or Rename `marrow explain`

The current name is dangerous because SQL users read `EXPLAIN` as planner
output. Marrow has no query plan to discover.

Valid outcomes:

- rename saved-path behavior under `marrow data explain` or `marrow debug
  explain`;
- keep `marrow explain` only if it renders checked source-owned facts and
  operation classes, not planner choices, costs from statistics, or
  `ANALYZE`-style execution output;
- delete it from v0.1 if its product story is not clear.

### Refine Data/Serve Boundaries

Data and serve should stay read-only debug/admin unless promoted through a
source-visible typed API design. Current risks to refine:

- `data dump` is unbounded as an operator command;
- data/serve path codecs and query resolution may become duplicate semantics;
- serve lacks version/capability negotiation, which is acceptable only while
  debug/admin;
- LSP position encoding needs UTF-16 or negotiated encoding before protocol
  correctness is claimed;
- restore must be compiler-owned and clean relative to source/catalog: restoring
  orphaned managed cells is a data-attached compiler error, and `marrow data
  integrity` should report what to do next.

### Re-Evaluate `out` And `inout`

`inout` for local-place mutation may be defensible. `out` is much less
idiomatic and should be re-justified or removed before language freeze.

Saved-path `inout` stays rejected.

### Refine Dynamic Identity Boundaries

Static typing prevents same-key foreign identities. Once `unknown`, host IO, or
JSON/data import grows, runtime identity values should carry enough store
identity, or typed reentry should validate dynamic identity store roots.

`unknown` is a boundary type, not `any`. It must never become a way to bypass
managed saved schemas or typed identity checks. Dynamic values must be checked
before they can enter typed Marrow code or saved data.

### Refine Rust Shape Before More Features

The code quality issue is architectural, not cosmetic. Oversized semantic
kernels make duplicate classifiers hard to detect. Lane 11 should split or
delete rather than decorate:

- `crates/marrow-check/src/checks.rs`
- `crates/marrow-check/src/evolution/discharge.rs`
- `crates/marrow-check/src/catalog.rs` and binding-heavy areas
- `crates/marrow-store/src/tree.rs`
- thousand-line catch-all test aggregators

Also remove glob preludes, production `use super::*`, clippy suppressions used
to hide poor structure, and comment sediment.

## Decisions To Reverse

### Reverse Resource-Owned Identity Aliases

Remove `Book::Id` and similar resource-owned identity claims from ADRs, docs,
tests, and any implementation affordance. `Id(^store)` is the canonical surface.

If future ergonomics demand aliases, they should be explicit store-owned aliases,
not automatic resource-owned identities.

### Reverse Source-Level `@id`

Keep `@id` dead. It should survive only as rejection tests and diagnostics.
Generated catalog tooling must not reintroduce stable-ID source annotations.

### Reverse `edit` And Patch-Like Surface Syntax

Do not keep `edit` or a dedicated patch/update DSL. Field writes already express
partial updates. Whole-resource assignment is exact replacement. Multi-field
patches are grouped field writes, usually in a transaction.

### Reverse Normal `merge` And `lock` Syntax Paths

`merge` and `lock` may remain reserved words, but they should not parse and
format as ordinary v0.1 statements. A reserved word should produce a direct
reserved/prototype diagnostic, not feel like a supported language form that fails
later.

### Reverse Saved-Path `inout`

Saved paths are not first-class mutable reference arguments. Use explicit saved
assignments at the call site.

### Reverse Any Hidden Optimizer Or Query Planner

Do not add a cost/statistics optimizer beneath ordinary Marrow source. Do not
teach users that there is a query plan to discover. If Marrow later adds a query
language, it must be source-visible, checked, bounded, and explicitly scoped.

### Reverse Production Raw Paths And Backend Bytes

Raw saved paths, physical keys, backend bytes, and archive replay are not stable
production APIs. They may exist only as explicit debug/admin surfaces with no
backup/restore, LSP, data-preview, serve, or application-protocol authority.

### Reverse An Accidental Production Data Server In v0.1

`marrow serve` v0.1 is not an app server, sync server, remote DB, or generated
API. Keep current `debug_data_*` behavior as loopback debug/admin only.

A future production local API is allowed, but it must be a new checked-fact
surface over resources/functions/effects, not the current debug path/value
protocol with a product label.

### Reverse Migration Scripts And Source-Diff Identity

No migration DSL, generated migration files, hidden database ledger, or
best-effort rename inference. Source-native `evolve` intent plus exact witnesses
is the model.

### Reverse Monotonic Entity Stable IDs For Git-Branch v0.1

Monotonic entity IDs need a central allocator. They do not fit branch-parallel
local source control. Keep monotonicity for epochs and commits, not catalog
entity IDs.

### Reverse Legacy Survival For Green Tests

Any old test or fixture depending on rejected behavior should be migrated or
deleted. Passing old tests is not a reason to keep a prototype path alive.

### Reverse Orphan-Preserving Production Restore

Restore is a compiler-owned data-integrity operation, not raw archive replay.
Restoring undeclared managed cells under the current source/catalog should fail
as a data-attached compiler error. `marrow data integrity` should report the
orphan and the repair/evolve path.

## Settled Scott Decisions

These decisions are settled for the next follow-up lanes:

1. `Id(^store)` is the v0.1 identity spelling. It is a type constructor, not a
   runtime function. If Marrow gains generic type syntax later, `Id<^store>` can
   be reconsidered, but automatic `Book::Id` remains rejected.
2. `marrow serve` may eventually become a real local API, SQLite-like and local
   rather than client/server. The future surface can expose resources/functions
   through URI-like HTTP/IPC endpoints, but it must be generated or checked from
   Marrow facts. The current v0.1 `debug_data_*` serve protocol stays
   debug/admin loopback only.
3. Restore is clean compiler-owned data integrity. Orphaned managed cells under
   the current source/catalog are a data-attached compiler error, and
   `marrow data integrity` should report what to do.
4. `unknown` is not `any`. It is a boundary type that must be checked before
   typed use, especially before identity values or managed saved writes.
5. Dangling references are allowed by default, but the compiler/data-integrity
   layer should be able to detect and report them.
6. Users model history and audit state explicitly. Marrow v0.1 is not a
   temporal fact database or automatic audit-log system.
7. Whole-resource assignment remains exact replacement with no default warning
   requirement.
8. Activation should be job-shaped for the long term. v0.1 can execute a
   single-transaction activation job, but large future rebuild/backfill/transform
   jobs should be chunked/resumable forms of the same witness model.
9. External engine adapters stay out of v0.1. Keep the typed tree-cell contract
   clean so future adapters implement Marrow, not SQL/ORM semantics.

## Remaining Decisions For Scott

These still need explicit product decisions before v0.1 freeze:

1. Should `out` exist in v0.1, or should the language prefer returned records,
   tuples, booleans-plus-result resources, or `Result`-like values?
2. Should `marrow explain` survive as a top-level command, be renamed under
   `data` or `debug`, or be deleted from v0.1?
3. Should `marrow data dump/get` remain default CLI commands if they expose
   canonical payload bytes, or should raw-byte inspection require a debug/admin
   namespace or flag?
4. Should `data dump` be allowed as an explicitly unbounded operator command, or
   must every durable-data preview be cursor/paging based?
5. Should any remaining non-cryptographic checksum wording be narrowed to
   accidental-corruption detection?

## Proposed Follow-Up Lane Plan

The lanes below are designed to be safer than another broad "cleanup" wave. Each
lane has an owner area, blockers, and a prompt. The lanes should use isolated
sibling worktrees, explicit isolated `CARGO_TARGET_DIR`s, and the lane loop in
AGENTS.md.

### Sequencing

1. Lane 12 starts first because it removes stale ADR/doc authority, but it does
   not block file-disjoint code lanes that already have settled decisions in this
   synthesis.
2. Lanes 14, 15, and 17 can start full TDD implementation immediately. Lane 14
   owns proposal-only evolution and activation-job shape; Lane 15 owns commit
   metadata/snapshot/store contract; Lane 17 owns rejected syntax cleanup except
   unresolved `out`.
3. Lane 13 owns catalog/presence hardening and settles ID size, digest, and
   reserved-alias wording before later stack lanes compose on it.
4. Lane 16 can start full implementation for settled tooling decisions:
   restore/orphan rejection, v0.1 debug/admin serve boundaries, shared tooling
   facts, LSP correctness, and raw production surface deletion. It must isolate
   unresolved `explain` and `data dump/get` product choices rather than blocking
   the whole lane.
5. Lane 19 is a short product-decision closure lane for the remaining product
   questions. It should run early and unblock the `out`, `explain`, `data dump`,
   compatibility-window, default, and re-key pieces.
6. Lane 18 runs last as final Rust hardening, after semantic owners have landed.

### Lane 12: ADR And Canonical Docs Reconciliation

Goal: make accepted ADRs and canonical docs agree with the v0.1 architecture
without creating new ADRs.

Owned files:

- `marrow-decisions/adr/**`
- `marrow/docs/**`
- `marrow/docs/roadmap/**`

Do not edit Rust except for absence scans if needed.

Prompt:

```text
You are Lane 12: ADR And Canonical Docs Reconciliation.

Work in a fresh isolated sibling worktree from current main. Do not create new ADRs. Update accepted ADRs and canonical docs in place so they match the implemented v0.1 vision and the finalized research synthesis at marrow/docs/roadmap/research/synthesis.md.

First inspect git state for the Marrow checkout and `marrow-decisions`, including dirty files. Read every integrated research report in marrow/docs/roadmap/research, the current docs/language set, docs/cli.md, docs/data-tools.md, docs/serve-protocol.md, docs/backend-contract.md, docs/data-evolution.md, and the full ADR packet.

Fix doc drift only. Remove or rewrite stale claims about Book::Id, @id, edit, raw inspection without checked source, raw archive replay as production, monotonic entity stable IDs, merge/lock as supported syntax, source-diff identity, migration scripts, query plans, and stale lane status. Keep Id(^store) as the v0.1 store-identity type constructor, random or collision-resistant catalog identity, source-is-access-path, typed backup/restore, checked facts, clean orphan-rejecting restore, unknown-not-any, dangling references as compiler-visible integrity facts, explicit user-mode history, exact whole-resource replacement, activation as a compiler-owned job shape, deferred external adapters, and current serve as debug/admin loopback only while preserving the future local API path.

Do not preserve completed-history sediment or temp worktree paths. Do not create new roadmap umbrellas. Every changed paragraph must be current product truth.

Verification:
- git diff --check in both repos if both are touched.
- Markdown scan for Book::Id, @id, edit, raw inspection, migration script, query plan, optimizer, merge, lock, archive, explain, unknown, orphan, serve, dangling, audit, history, activation job, and external adapter; every remaining match must be current product truth, accepted, reserved/rejected, future-only, or debug/admin with a clear verdict.

Before claiming done, run read-only review with two lenses: consistency against synthesis and anti-sediment. Return changed files, exact scans, and reviewer verdicts.
```

### Lane 13: Catalog Identity And Presence Hardening

Goal: remove catalog identity ambiguity and make presence facts harder to misuse.

Owned areas:

- `crates/marrow-project/src/**`
- `crates/marrow-check/src/catalog.rs`
- `crates/marrow-check/src/facts.rs` only for catalog/presence identity shape
- `crates/marrow-check/src/presence/**`
- catalog/presence tests
- catalog/presence docs touched by Lane 12 decisions

Prompt:

```text
You are Lane 13: Catalog Identity And Presence Hardening.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md. Read the synthesis, Lane 6 report, Lane 5 report, language docs, and catalog/model ADRs before editing.

Mission: harden catalog identity and presence proofs without changing language semantics. Refresh all evidence from current HEAD before acting.

Targets:
- Resolve random-vs-monotonic stable ID law in implementation/docs using the synthesis as current product truth; rebase and align if Lane 12 lands ADR wording while you work.
- Preserve the reserved alias lifecycle and prove retired/reserved spellings cannot
  be silently reused.
- Preserve typed optional catalog IDs; do not regress proposal-only identity into
  empty-string sentinels.
- Make enum source-order helpers private to lowering/traversal or clearly typed as non-durable.
- Strengthen catalog branch-merge fixtures for parallel additions, aliases, enum members, indexes, and conflict diagnostics.
- Preserve explicit proof identity and Discharged/PendingAttachedData status in
  presence proofs.
- Prove runtime/tooling/evolution consumers cannot rediscover maybe-present semantics outside the presence owner.
- Ensure dynamic identity reentry cannot treat unknown as any or accept same-shaped foreign identities without a typed store-root check.

Do not reintroduce @id, Book::Id, source-order stored enum meaning, source-name identity, or compatibility glue. Do not broaden checker dispatchers; split touched code by invariant.

Start with failing tests or architecture absence tests and proceed to implementation. Focused gates first, then fmt, clippy -D warnings, and workspace tests with explicit CARGO_TARGET_DIR.

Before claiming done, run soundness and idiom/spec review. Soundness must attack branch merge, alias reuse, enum reorder/remove, and maybe-present read proofs. Idiom/spec must reject sentinel strings, duplicate classifiers, oversized helpers, low-value comments, and stale tests.
```

### Lane 14: Evolution Soundness And Activation Receipts

Goal: close the proposal-only apply hole and make activation job-shaped without
creating migration history.

Owned areas:

- `crates/marrow-check/src/evolution/**`
- `crates/marrow-run/src/evolution/**`
- `crates/marrow/src/cmd_evolve/**`
- evolution apply/discharge/CLI tests
- data-evolution docs touched by the lane

Prompt:

```text
You are Lane 14: Evolution Soundness And Activation Receipts.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md. Read the synthesis, Lane 9 report, Lane 14A online evolution foundation report, architecture red-team report, catalog ADRs, evolution ADRs, docs/data-evolution.md, and docs/language/resources-and-storage.md.

Mission: make source-native evolution sound before v0.1 freeze. Compiler equals data integrity: preview/apply must prove source, catalog, attached data, and engine facts together. Do not add migration scripts, source-diff identity, hidden ledgers, host migration shims, or compatibility glue.

Start with the red-team blocker. Write a production runtime/CLI apply fixture where the accepted catalog lacks a newly required member, current source adds it with evolve default or transform, old records exist, preview is activatable, and apply must backfill or fail closed correctly before accepting the proposal. Either executable places must carry proposal data-cell identities, or preview must mark the change non-activatable until accepted. Do not paper over this by accepting the catalog first in the test.

Then make activation job-shaped:
- Model apply as a compiler-owned activation job created from the exact preview witness. The v0.1 job may execute immediately in one transaction, but the type/API shape should leave a clean path to future chunked/resumable jobs.
- Preserve the conceptual future protocol in names and facts where it naturally fits: preview, start, bridge, backfill, verify, publish, and close. Do not implement future online execution unless it is required to fix v0.1 soundness.
- The catalog epoch must not publish until the activation job verifies and commits.
- Generated indexes, required-field backfills, and transforms must not become half-visible product state. A crash or drift resumes from typed job evidence or fails closed.
- Job facts/receipts are derived evidence, not executable migration history. They may include epoch, source digest, previous and next catalog digests, engine profile, store commit pin, affected stable IDs, verdicts, counts, approvals, and final commit id.
- Tie backup/restore and CLI rendering to job/receipt facts only as evidence, not as a second source of semantics.
- Record compatibility-window policy as future facts only: v0.1 remains exact epoch/schema equality; future server mode normally supports one old epoch, old clients default to read-only or rejected, and old writes require a checked adapter proof.
- Treat key-shape, resource-shape, layout, and engine changes that cannot be proven as ordinary backfill as future shadow-decant work, not raw store patching or identity-preserving reinterpretation.

Split the evolution discharge kernel by invariant if touched: proposal ID resolution, structural compatibility, data scans, default/transform obligations, index obligations, repair diagnostics, and witness accumulation. Do not grow another broad dispatcher.

Focused gates:
- evolution discharge tests for proposal-only default/transform
- runtime evolution apply tests for exact witness apply
- evolve CLI tests for preview/apply/job/receipt output if output changes
- crash/drift tests proving activation jobs do not publish partial catalog/data visibility

Before claiming done, run soundness review that tries to drift source/catalog/store between preview and apply, and idiom/spec review that rejects oversized discharge code, duplicate classifiers, migration-language residue, and comment sediment.
```

### Lane 15: Transaction, Commit Metadata, And Store Contract Hardening

Goal: make the store/runtime durability boundary precise enough for backup,
snapshots, and future activation.

Owned areas:

- `crates/marrow-store/src/**`
- `crates/marrow-run/src/write_plan.rs`
- `crates/marrow-run/src/env.rs`
- `crates/marrow-run/src/transaction.rs`
- store conformance tests
- runtime transaction/write-plan tests
- `docs/backend-contract.md`

Prompt:

```text
You are Lane 15: Transaction, Commit Metadata, And Store Contract Hardening.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md. Read the synthesis, Lane 7 report, Lane 8 report, Lane 14A online evolution foundation report, architecture red-team report, backend contract docs, storage ADRs, and transaction ADRs including current unstaged decision edits if still present.

Mission: harden the physical durable boundary without exposing raw engine semantics.

Targets:
- Move or prove commit metadata stamping at the physical durable commit boundary. Add a transaction test with multiple managed writes and generated index updates, then assert changed roots/indexes and commit id describe the whole transaction, not the last plan.
- Leave the commit metadata shape ready for future online activation evidence: runtime generation, optional activation job id, source/catalog digest, layout epoch, changed roots/indexes for the whole commit, and adapter/window evidence. Do not implement unused online machinery; preserve the typed metadata seam.
- Add store conformance for pinned snapshot plus open write transaction, or explicitly reject/prohibit that state so memory and redb cannot diverge.
- Reconcile backend-contract metadata docs with implementation, including source digest if still present.
- Replace or quarantine unbounded child-key helpers with bounded pages/cursors or crate-private test conveniences.
- Decide and implement typed payload wrappers for canonical leaf/index/sequence/identity bytes, or explicitly freeze Vec<u8> as the stable payload contract with tests and docs.
- Split tree.rs by invariant if touched: facade, metadata, data cells, index cells, child scans, reference values, enum values, commit codec.

Do not restore public backend/path/archive APIs. Do not add compatibility branches for old raw paths. Do not expose physical key bytes as production API.

Use TDD with focused store conformance/runtime transaction tests first. Then run marrow-store tests, marrow-run focused tests, fmt, clippy -D warnings, and workspace tests with explicit CARGO_TARGET_DIR.

Before claiming done, run soundness review for rollback, commit visibility, snapshot isolation, metadata correctness, cursor validity, and read-only opens. Run idiom/spec review for raw leakage, oversized modules, unbounded materialization, low-value comments, and duplicate key classifiers.
```

### Lane 16: Tooling Facts, Debug Surface, And Explain Rescope

Goal: prevent CLI/LSP/data/serve/backup tools from becoming a second semantic
model.

Owned areas:

- `crates/marrow/src/cmd_data/**`
- `crates/marrow/src/cmd_explain.rs`
- `crates/marrow/src/serve/**`
- `crates/marrow/src/lsp.rs`
- `crates/marrow/src/backup/**` only for rendering/manifest integration
- CLI/data/serve/LSP/backup tests
- `docs/cli.md`, `docs/data-tools.md`, `docs/serve-protocol.md`, `docs/lsp.md`

Prompt:

```text
You are Lane 16: Tooling Facts, Debug Surface, And Explain Rescope.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md. Read the synthesis, Lane 10 report, Lane 8 report, Lane 14A online evolution foundation report, architecture red-team report, docs/cli.md, docs/data-tools.md, docs/serve-protocol.md, docs/lsp.md, tooling ADRs, and storage backup ADRs.

Mission: make tools render shared facts and stop raw/path/debug surfaces from becoming production semantics. Compiler equals data integrity: tools report compiler/store facts; they do not become a second database model.

Start with TDD for settled decisions:
- restore rejects orphaned managed cells under the current source/catalog before commit;
- data integrity reports orphaned managed cells with actionable compiler/data-integrity guidance;
- serve remains loopback debug/admin only in v0.1, while docs preserve the future local checked API direction;
- unknown/raw payload/debug surfaces cannot become typed production APIs;
- LSP position encoding is corrected before protocol correctness is claimed.

Also produce a refreshed feature-surface verdict matrix for: marrow explain, marrow data roots/stats/dump/get/integrity, marrow serve debug_data_*, run --trace, test --trace, run --dry-run, --maintenance, backup/restore, LSP, future adapters, raw saved paths, raw payload bytes, cursors, unbounded scans, and generated API/server/sync language. This matrix drives implementation; it is not a stop point.

Then implement only surfaces that have a clear verdict:
- Keep production only if ADR-backed, typed, bounded, snapshot/epoch bound, and rendered from shared facts.
- Keep debug/admin only if explicitly named or flagged and excluded from production protocol semantics.
- Rename/rescope if names imply query planning, SQL EXPLAIN, production server behavior, or stable raw paths.
- Delete unsupported surfaces and their docs/tests/help output.

Specific focus:
- Isolate unresolved Scott decisions for marrow explain and raw data dump/get. Do not block restore, serve, LSP, backup, or shared-facts cleanup on those choices. Once Scott decides, implement delete, rename under data/debug, or rebuild as checked fact/operation rendering. It must not expose a query plan or planner choices.
- Extract or create a transport-free tooling facts API for typed data-query resolution, checked path rendering, bounded previews, integrity findings, explain facts, snapshot/catalog metadata, and cursor contracts. CLI and serve become adapters.
- Add future activation rendering to the shared-facts backlog: preview outcome, activation job status, chunk progress, verification findings, publish readiness, compatibility-window admission, adapter names, and close conditions. The v0.1 CLI may only render exact apply/receipt evidence, but the facts API should not force a migration-ledger model later.
- Keep data previews consistently bounded where they are previews. If unbounded dump remains before Scott's decision, explicitly classify it as an operator/admin command, not a production preview API.
- Keep serve loopback debug/admin only for v0.1. Preserve the future local API direction as a separate checked-fact surface; do not rename debug_data_* into product operations.
- Fix LSP position encoding before protocol correctness is claimed.

Do not patch missing semantic facts locally in tools. Send missing facts back to the owning checker/runtime/store lane.

Before claiming done, run soundness review for raw leakage, stale epoch/snapshot, cursor forgery/replay, unbounded previews, and restore/tool mismatch. Run idiom/spec review for thin adapters, no tool-local classifiers, split protocol modules, no comment sediment, and docs that clearly mark debug/admin boundaries.
```

### Lane 17: Language Surface Simplification

Goal: make the v0.1 language surface honest, small, and non-SQL.

Owned areas:

- `crates/marrow-syntax/src/**`
- syntax/parser/formatter tests
- checker prototype rejection tests only as needed
- `docs/language/**`
- language examples and sample docs

Prompt:

```text
You are Lane 17: Language Surface Simplification.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md. Read the synthesis, holistic language audit, architecture red-team report, docs/language, and language ADRs.

Mission: make the v0.1 language surface match the vision. Do not implement new features. Delete or make rejection-only any prototype syntax that is not v0.1.

Targets:
- Stop merge and lock from remaining normal parser/formatter round-trip statements. Keep the words reserved and produce direct reserved/prototype diagnostics, or remove the AST paths if that is the cleanest v0.1 cut.
- Keep @id rejected. No source stable-id annotations.
- Keep saved-path inout rejected.
- Remove edit/patch/update-style wording from language docs and samples. Field writes express partial updates; transactions group them.
- Do not block on the unresolved out-parameter decision. Exclude out from this lane unless Scott decides before you finish; if a decision lands, either delete out syntax/docs/tests and migrate examples, or document why it earns its place and ensure it cannot become saved-reference mutation.
- Add or update a "What Marrow Is Not" language doc section if Lane 12 did not already do it: not SQL, not ORM, not query optimizer, not migration DSL, not temporal fact DB, not raw KV.
- Make sequence/map docs consistently describe saved tree-layer conveniences, not general local list/map abstractions.
- Keep exact whole-resource assignment as normal replacement semantics. Do not add a warning system by default.

Use TDD: first update parser/formatter/checker tests to express rejection-only behavior, watch failures, then change syntax/checker/docs. Do not move rejection to parser if it would break deliberate reserved-word diagnostics without a test.

Before claiming done, run soundness review for surviving prototype syntax and idiom/spec review for language simplicity, stale docs, examples, overgrown parser branches, and low-value comments.
```

### Lane 18: Final Rust De-Slopification And Release Hardening

Goal: prove the semantic lanes cleaned their areas and finish file-disjoint Rust
quality cleanup.

Owned areas:

- all crates and docs only after semantic owners have landed;
- no active semantic bug may be hidden as style cleanup.

Prompt:

```text
You are Lane 18: Final Rust De-Slopification And Release Hardening.

Work from current main in a fresh isolated sibling worktree with an isolated CARGO_TARGET_DIR. Follow AGENTS.md and the synthesis. This lane verifies cleanup happened; it is not where active semantic lanes dump unresolved design work.

Start read-only. Inspect current main, worktrees, branches, dirty files, and unresolved conflicts. Refresh every scan from current HEAD; stale synthesis line numbers or chat memory are not evidence.

Required scans:
- unsafe
- use super::* and pub use glob preludes
- clippy allow/expect suppressions
- Unknown/fallback/sentinel strings in production paths
- raw path/backend bytes/archive/debug_admin production leakage
- @id, Book::Id, Author::Id, merge, lock, saved inout, edit, patch, query plan, optimizer, migration script
- unknown used as any, dynamic identity without store-root reentry checks, orphan-preserving restore, accidental production serve APIs, and non-job-shaped activation state
- duplicate classifiers in checker/runtime/schema/tools
- unbounded materialization APIs
- oversized functions/modules in semantic hotspots
- comment sediment
- stale docs/future docs/tests that preserve rejected surfaces

For every finding, assign an owning verdict: fix in Lane 18 only if file-disjoint and semantic ownership is settled; otherwise return it to the semantic owner and report blocked for that surface.

When editing:
- split or delete touched production paths in focused batches;
- remove duplicate helpers, compatibility glue, low-value comments, and stale tests;
- migrate catch-all tests into source-driven invariant fixtures where touched;
- do not add broad cleanup commits or new abstractions without a clear invariant;
- do not create new ADRs.

Completion requires a clean status, exact base/head, changed-file list, focused and full gate output, reviewer verdicts, updated lane status, and absence/sibling scans proving old patterns are gone across the owned area.
```

### Lane 19: Remaining Product Decision Closure

Goal: close the remaining product decisions that block small parts of Lanes 16
and 17.

Owned areas:

- research docs only;
- optional updates to `docs/roadmap/research/synthesis.md` if Scott asks.

Prompt:

```text
You are Lane 19: Remaining Product Decision Closure.

This lane closes the remaining product decisions listed in the synthesis. It does not implement Rust. It may update docs/roadmap/research/synthesis.md only if Scott asks for the decisions to be recorded there.

For each remaining question, produce:
- the decision to make;
- why it matters before v0.1;
- options with tradeoffs;
- the recommended choice;
- what lanes are blocked by the choice;
- what docs/tests/code would change if Scott accepts the recommendation.

The remaining questions are:
- out parameters;
- marrow explain;
- marrow data dump/get debug/admin scope;
- unbounded data dump versus cursor/page contract;
- compatibility-window defaults for future server mode: one-old-epoch rule and
  old-write admission policy;
- required-field `default` meaning: temporary activation fill only versus a
  durable read default;
- re-key identity semantics: always new store plus explicit transform/decant, or
  some identity-preserving cases with proof.

Use the settled Scott decisions as constraints: Id(^store) is v0.1 spelling, serve v0.1 is debug/admin loopback with a future local checked API path, restore rejects orphans, unknown is not any, dangling references are allowed but compiler-visible, history is user-mode, whole-resource assignment is exact replacement, activation is job-shaped, v0.1 stays strict exact-epoch, future online activation is multi-epoch and compiler-mediated, compatibility adapters are bounded/generated/deleted, shadow decant is the major-reshape path, and external adapters are deferred.

Be skeptical. Do not keep a feature because it exists. Default to delete or defer unless the v0.1 language/database vision clearly needs it.
```

## Recommended Immediate Order

1. Start Lane 12 immediately to reconcile docs/ADRs with the settled decisions.
2. In parallel, start Lane 14 and Lane 15 with failing tests for the red-team
   blockers: proposal-only evolution apply, job-shaped activation, commit
   metadata boundary, and snapshot conformance.
3. Start Lane 17 immediately for ready language cleanup: `merge`, `lock`, `@id`,
   saved `inout`, `edit`/patch wording, "What Marrow Is Not", and collection
   wording. Exclude `out` until Lane 19 closes it.
4. Start Lane 16 immediately for settled tooling work: orphan-rejecting restore,
   data-integrity guidance, debug/admin serve boundary, shared tooling facts,
   LSP position correctness, and raw production surface deletion. Isolate
   unresolved `explain` and `data dump/get` choices.
5. Run Lane 19 early as the short decision closure lane for the remaining
   product choices.
6. Run Lane 18 last as final hardening, not as a substitute for lane-local
   cleanup.

## Completion Bar For The Follow-Up Program

The follow-up program is not complete when tests are green. It is complete only
when:

- stale ADR/doc authority is removed;
- every surviving feature surface has a keep/refine/reverse verdict;
- proposal-only evolution apply is proven sound or rejected;
- commit metadata describes the durable commit boundary;
- activation is represented as a compiler-owned job shape, with v0.1
  single-transaction apply as the simple case;
- restore rejects orphaned managed cells under the current source/catalog;
- raw paths/bytes are absent from production protocols;
- `merge`, `lock`, `@id`, saved `inout`, `edit`, and `Book::Id` survive only as
  rejected/reserved/future references;
- `unknown` remains a boundary type, not `any`;
- dangling references are compiler-visible integrity facts but not implicit FKs;
- tool adapters render shared facts instead of owning semantics;
- broad Rust semantic kernels are split or justified by current evidence;
- comments explain durable rationale only;
- final scans and full gates pass from current main with isolated target dirs;
- soundness and idiom/spec reviews have no blocking findings.
