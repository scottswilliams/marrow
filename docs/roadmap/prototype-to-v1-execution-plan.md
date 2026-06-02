# Prototype To V0.1 Execution Plan

This plan is the handoff from the prototype Marrow implementation to the v0.1
architecture described by the accepted decision packet in `marrow-decisions`.
It is not an ADR. It is the implementation contract for replacing prototype
semantics with the production model, deleting duplicate paths, and keeping the
Rust workspace simple enough to reason about.

## Goals

- Build a strong v0.1 foundation before feature breadth: clear ownership,
  durable identity, checked execution, tested storage semantics, and facts that
  tools can trust.
- Implement the accepted ADR packet end to end in this repository.
- Replace syntax-backed and string-backed runtime behavior with checked facts
  and checked executable IR.
- Split resource types, stores, catalog identity, logical tree cells, runtime
  write effects, evolution, backup, and tooling into clear ownership boundaries.
- Delete every prototype-only production path named by the ADR packet; keep
  only explicit debug/admin surfaces and named short-lived bridges needed to
  replace a vertical stack.
- Treat prototype deletion, Rust simplification, and duplicate-path removal as
  implementation work, not cleanup after the fact.
- Keep every lane test-driven, reviewed, file-disjoint where possible, and
  integrated only after the full Rust and docs gates pass.

## Non-Goals

- Do not create ADRs in this repository.
- Do not build a server product, sync system, distributed transaction model,
  permission system, public generated API, or outbox syntax for v0.1.
- Do not add dependencies without an explicit architecture decision.
- Do not preserve old and new production paths after a replacement lane lands.
- Do not make raw saved paths, backend bytes, source-order enum ordinals, or
  source names stable production identity.

## Operating Rules

Each substantial lane uses this loop:

1. Create an isolated worktree outside the repository, with a dedicated
   `CARGO_TARGET_DIR` outside the repository.
2. Assign one build agent to write the failing check first, implement the
   smallest replacement, and leave the worktree dirty for review.
3. Run two read-only senior reviews in parallel:
   - soundness: try to break identity, storage, write, transaction, and data
     compatibility semantics with real repros;
   - idiom/spec: verify minimality, local style, ADR traceability, docs, code
     shape, and no slop. This review must inspect touched Rust for oversized
     dispatcher functions, catch-all semantic passes, duplicate helpers,
     comment-heavy code, and compatibility glue.
4. Fix every finding in the lane, then re-review until both reviews pass.
5. Integrate through the live main worktree only:
   - fetch and record the current `origin/main`;
   - rebase the lane on that exact commit;
   - resolve only obvious mechanical conflicts;
   - run the lane's focused gate;
   - update the integration worktree to the same `origin/main`;
   - cherry-pick the reviewed commit with `git cherry-pick -x`;
   - run the full integration gate with the integration target directory;
   - request a final read-only review of the assembled main diff;
   - fetch again and push only if `origin/main` has not moved.

Stop the line when the same Rust smell appears in more than one lane. The
orchestrator updates the affected lane plans and starter prompts before assigning
new implementation work, then sends dirty lanes back to their build agents for
code-shape repair. A lane with oversized dispatcher functions, comment-heavy
control flow, duplicate semantic classifiers, or new compatibility glue has not
reached review-ready state.

Each lane owns complete cleanup of its area. The build agent must delete
prototype paths, duplicate classifiers, dead APIs, stale fixtures, low-value
comments, and weak module shape in the files it owns. Lane 11 is an audit for
missed residue after the owning lanes land; it is not a parking lot for cleanup
that the current lane already knows it must do. If a prototype bridge has no
current production caller, delete it instead of preserving it as a future
handoff.

Green tests or compile success are not reasons to keep legacy behavior alive.
If an old test, fixture, CLI path, runtime caller, or helper depends on rejected
prototype semantics, the lane must migrate or delete that dependency and make
the v0.1 path pass. Do not add fallback branches, boolean compatibility modes,
test-only production entrypoints, or duplicate semantic models so the old
runtime continues to pass. A production bridge exists only for a named live
production caller that cannot move inside the same file-disjoint lane; test
continuity and compile convenience are not live callers.

Use one cargo target directory per lane, for example:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/<lane> \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-<lane>/Cargo.toml ...
```

Never run broad cargo gates in parallel against the same target directory.

## Central Tracking

This document is the durable implementation tracker. Do not create a second
roadmap in chat, memories, scratch files, or ADRs. Per-lane orchestration plans
live under [`lanes/`](lanes/); each lane file is the owner-facing tracker for one
orchestrator. This central file owns the queue, dependency graph, global gates,
and cross-lane collision map. Lane files own file lists, split safety, TDD
starts, review prompts, and deletion ledgers.

Tracking is forward-only:

- keep only the next unintegrated lanes and active blockers;
- delete completed lane history instead of appending diary entries;
- record design decisions in the lane's canonical docs or commit message, not in
  a new ADR;
- keep temporary worktree paths, target directories, review transcripts, and
  throwaway artifacts out of tracked files.

Current queue:

| Order | Lane | Why It Is Next | Must Prove |
| --- | --- | --- | --- |
| 1 | [Lane 5: Resource/Store Surface, Schema Split, And Store-Owned Indexes](lanes/lane-05-resource-store-surface.md) | Aligns the language and schema model with the accepted split resource/store design. | Resource types, stores, indexes, and `Id(^store)` stop sharing the old resource-owned identity foundation. |
| 2 | [Lane 6: Catalog Identity Binding And Presence Ledger](lanes/lane-06-catalog-presence-ledger.md) | Durable identity, the accepted catalog file, and the proof-discharge ledger must exist before tree-cell storage and activation can be production foundations. | Physical data identity no longer depends on source spelling, `@id`, source-order enum ordinals, regenerated IDs, or ad hoc read-presence checks. |
| 3 | [Lane 7: Tree-Cell Store And Engine Profile](lanes/lane-07-tree-cell-store-engine.md) | The store can move to stable-ID key space only after resource/store identity and catalog bindings are named. | Physical keys derive from stable catalog IDs, typed key values, and the reserved placement prefix, not source names. |

## Parallel Orchestrator Split

Assign one lead orchestrator per lane file. Parallelize design scans, review,
and file-disjoint implementation; sequence production code when two lanes would
edit the checker/schema identity surface.

| Track | Lane Plan | Can Start Now | Code Timing | Collision Boundary |
| --- | --- | --- | --- | --- |
| Resource/store | [Lane 5](lanes/lane-05-resource-store-surface.md) | Yes | First production code lane | Owns syntax, schema, and the store-aware checked-facts bridge; no catalog/presence checker edits in parallel. |
| Catalog/presence | [Lane 6](lanes/lane-06-catalog-presence-ledger.md) | Design and review only | Code after Lane 5 store facts integrate | Owns accepted catalog metadata and read-proof ledger; no store physical key edits. |
| Tree-cell store | [Lane 7](lanes/lane-07-tree-cell-store-engine.md) | Read-only planning and engine-substrate checks only | Production key work after Lane 6 | Owns store backend/tree-cell code; no checker/catalog ownership. |
| Runtime | [Lane 8](lanes/lane-08-runtime-checked-execution.md) | Inventory and design only | Code after store facts, presence ledger, and tree-cell address API exist | Owns runtime checked execution; no syntax-body compatibility path survives. |
| Evolution | [Lane 9](lanes/lane-09-evolution-activation.md) | Read-only witness matrix design only | Code after catalog, proof ledger, store, and runtime facts exist | Owns one proof-discharge pipeline with command-specific surfaces. |
| Tooling/protocols | [Lane 10](lanes/lane-10-tooling-backup-protocols.md) | Read-only stale protocol audit only | Code after shared facts, store/runtime facts, and evolution generation facts exist | Owns the typed backup manifest first, then adapters and rendering; no tool-local semantic classifiers. |
| Hardening | [Lane 11](lanes/lane-11-rust-hardening.md) | Read-only scans anytime | Final fixes after owning lanes land, except truly file-disjoint style fixes | Owns deletion proof, not postponed semantic rewrites. |

Lane 5 may split internally into syntax/schema and checked-facts tasks, but one
orchestrator owns both so the project does not grow a compatibility shim between
resource/store identity and checker facts. Lane 6 similarly owns both catalog
binding and the ADR 0210 presence ledger so read totality is not duplicated
across modules.

## Prototype Removal Controls

The pivot from prototype to v0.1 is the central constraint. A lane that adds the
new architecture but leaves the prototype production path quietly reachable has
not succeeded.

Every implementation lane must include a **prototype removal ledger** in its
lane plan:

- replacement behavior: the v0.1 behavior that becomes authoritative;
- old production paths touched by the lane;
- duplicate classifiers or duplicate resolution logic that must disappear;
- production bridge, only when a current production caller cannot be moved in the
  same file-disjoint lane, naming the live caller, isolation boundary, absence
  test, and exact deletion lane;
- architecture absence tests or scans that prove the old path is no longer
  production-reachable;
- docs that must be deleted, folded into canonical references, or marked
  debug/admin only.

Production bridges are exceptional. They are allowed only when a live production
caller cannot be moved in the same file-disjoint lane, and the bridge is named,
isolated, reviewed, covered by an absence test, and assigned to a deletion lane
at creation time. A bridge with no current production caller is deleted, not
handed off. Old tests, obsolete fixtures, compile errors, and runtime green-bar
pressure do not justify a bridge; migrate the caller or delete the obsolete
expectation. A bridge may not create a new semantic owner or let production
callers choose between old and new semantics. The only acceptable
runtime-replacement bridge is a syntax-to-IR adapter for one named live caller;
it may not preserve runtime name resolution as a second checker.

Semantic ownership after v0.1:

- syntax parses source and preserves spans;
- catalog owns stable durable identity;
- checker owns resolved facts, types, durable places, effects, and diagnostics;
- runtime executes checked facts and write plans;
- store persists ordered bytes and exposes engine/tree-cell primitives;
- tools render shared facts and never rediscover Marrow semantics.

Any code that violates that ownership map is a prototype vestige unless a lane
explicitly proves it is debug/admin-only.

Rust cleanup is also lane-scoped:

- when a lane touches a prototype module, it must delete dead branches and
  duplicate helpers in that module instead of wrapping them;
- large semantic files must be split when that is the smallest way to give one
  clear invariant per module;
- broad functions introduced or expanded by the lane must be split before review
  handoff. A passing test suite does not justify leaving a giant statement/type
  dispatcher, a pile of local helper comments, or a second semantic classifier in
  place;
- new crate-root glob preludes are forbidden, and existing glob-prelude patterns
  are deletion targets when a lane rewrites that crate boundary;
- tests must move toward source-driven production fixtures instead of adding
  more catch-all assertions to giant files;
- `#[allow(dead_code)]`, unused public APIs, fallback lookup helpers, and
  "just for compatibility" functions are deleted. Explicit debug/admin product
  surfaces may remain only when excluded from production semantics and reviewed
  as such;
- when tests fail because the old runtime, old schema shape, or old path model
  disappeared, update the tests to the v0.1 contract instead of preserving a
  legacy codepath for them;
- comments added by a lane must explain durable rationale. Comments that narrate
  control flow, summarize obvious branches, or explain temporary migration state
  are deleted or replaced with better names and smaller helpers.

Every lane plan must name the touched Rust hotspots and expected split/deletion
shape in its Area Cleanup Gate. Starter prompts refer to that gate and include
only lane-specific blocking deltas. Reviewers treat "this can wait for Lane 11"
as a failing answer when the lane introduced or expanded the smell.

The hardening lane is not a place to postpone known deletion. It is the final
audit that proves previous lanes deleted what they said they would delete.

## Strong Foundation Rule

For v0.1, weak foundations are blockers, not debt. A lane may not build on top
of a prototype foundation that the accepted ADR packet rejects. When a lane
finds one, it must choose one of these outcomes before integration:

- replace it with the v0.1 foundation and delete the old path;
- limit it to an explicit debug/admin boundary that is part of the v0.1
  product, or to a production bridge with a live caller, isolation boundary,
  absence test, and deletion lane;
- stop and run a design review for the replacement shape.

The following foundations are mandatory before dependent breadth work:

| Foundation | Must Be True Before | Weak Foundation To Remove |
| --- | --- | --- |
| Checked facts and IDs are authoritative | runtime, tools, evolution | runtime/source/tool re-resolution |
| Presence proof ledger exists | activation, evolution, runtime reads, tools | scattered absence/read-totality classifiers |
| Catalog identity exists and owns durable IDs | tree-cell storage, backup, evolution | `@id`, source spelling, regenerated IDs |
| Resource/store split is internal model | runtime writes, indexes, references | fused resource/root/schema ownership |
| Store-owned indexes are explicit facts | index maintenance, durable traversal, backup | resource-owned production indexes |
| Tree-cell storage keys derive from stable IDs | redb layout, backup, restore | source-name encoded physical keys |
| Runtime executes checked IR/facts | production `run`, transactions, tools | AST-body execution and dynamic fallback lookup |
| Evolution has exact witnesses | destructive changes, catalog apply | source-diff inference or migration-script framing |
| Tools consume shared facts | CLI/LSP/serve/data protocols | tool-local semantic classifiers |

If a proposed lane cannot strengthen one of these foundations directly or
remove a weak foundation that blocks it, it is not an implementation lane for
this phase.

## Data Quality Gate

Every lane that touches durable data, checked facts, storage, backup/restore,
or tooling protocols must name its data-quality contract before implementation:

- the source-level fixture that exercises the production pipeline;
- the catalog or checked fact that proves stable identity;
- the write, transaction, backup/restore, and integrity behavior the lane
  changes or intentionally leaves unchanged;
- the stale-data or compatibility fixture that would fail if old source-spelling
  identity, raw path identity, or duplicate classifiers remained authoritative;
- the exact scan that proves a rejected prototype path is absent from production
  code and canonical docs.

Raw byte validity, green unit tests, or a local helper that duplicates semantic
classification are not enough evidence for a data-quality claim. The proof must
come from the same source, schema, checked program, store, and tool path that
users exercise.

## Full Integration Gate

Every integrated code lane must pass fresh output from:

```sh
git diff --check origin/main..HEAD
cargo fmt --manifest-path <worktree>/Cargo.toml --all --check
CARGO_TARGET_DIR=<target> cargo build --manifest-path <worktree>/Cargo.toml --workspace --all-features
CARGO_TARGET_DIR=<target> cargo test --manifest-path <worktree>/Cargo.toml --workspace --all-features
CARGO_TARGET_DIR=<target> cargo clippy --manifest-path <worktree>/Cargo.toml --workspace --all-targets --all-features -- -D warnings
! rg -n 'unsafe\s*(\{|fn|impl|trait|extern)' <worktree>/crates
```

Docs-only lanes must pass `git diff --check`, relevant stale-term scans, link
checks when links change, and senior review. The integration gate still runs the
full Rust workspace before pushing if the docs alter language, data, or tooling
contracts.

Each replacement lane also gets an architecture absence check for the prototype
path it replaces. Examples:

- checked-model lanes fail if executable facts can contain recovery-only
  `Unknown`; any syntax-body bridge must be named non-production scaffolding and
  must be removed by the runtime replacement lane;
- runtime lanes fail if production execution still splits `::` strings, resolves
  saved paths from syntax at runtime, or accepts raw syntax bodies;
- store lanes fail if production physical keys encode source root, field, layer,
  index, or enum-member names;
- enum lanes fail if stored meaning depends on declaration-order ordinal alone;
- tooling lanes fail if raw path/value protocols are still described or exposed
  as stable production APIs.

## ADR Traceability

| Source | Implementation Responsibility | Primary Lanes |
| --- | --- | --- |
| ADR 0000 | Production laws, prototype-only inventory, gate criteria | all lanes, hardening |
| ADR 0001 | Local embedded product target, future boundary | docs, tooling |
| ADR 0002 | Source + catalog + data + engine compile together | checked model, catalog, evolution |
| ADR 0003 | Rust checked execution strategy, no unsafe, no syntax runtime | checked model, runtime replacement |
| ADR 0004 | redb as engine substrate, not semantic owner | tree-cell store, backup |
| ADR 0005 | Production-pipeline testing strategy | shared fixtures, all lanes |
| ADR 0006 | Canonical terminology | docs alignment, diagnostics |
| ADR 0101-0105 | Current prototype inventory and invariants to preserve | rejection, deletion inventory |
| ADR 0200 | user model, colon-canonical resource literals, compile/apply modes, requiredness, write, absence, and reference laws | language surface, checked model, runtime, fixtures |
| ADR 0201 | checked facts, durable places, effects, IR, no executable recovery | checked model, runtime replacement, tooling |
| ADR 0202 | resource/store split, store-aware identity, typed references, typed tree cells | parser/schema/checker, catalog, tree-cell store, fixtures |
| ADR 0203 | source-native evolution, witnesses, compatibility windows, approvals | catalog, evolution, tooling |
| ADR 0204 | engine contract, tree cells, transactions, commit metadata, backup | tree-cell store, runtime, backup |
| ADR 0205 | shared facts, local generations, raw debug only | tools and protocols |
| ADR 0206 | catalog lifecycle, identity binding, and committed accepted catalog file | catalog |
| ADR 0207 | store-owned indexes, index key laws, durable traversal, collections-as-trees, internal range iterators, platform bounded scans, sequence laws | resource/store parser and schema, catalog, tree-cell store, checked model, runtime, backup |
| ADR 0208 | physical key/value encoding, stable-ID key space, reserved placement prefix, enum-member identity encoding, and ordered scalar laws | catalog, tree-cell store, enum storage, backup |
| ADR 0209 | reserved typed ephemeral roots, future checked `~` effect class | Lane 5 parser reservation and rejection, Lane 8 checked-effect/runtime absence |
| ADR 0210 | presence proof sources, discharge ledger, and activation gate | checked model, catalog, data-attached check, runtime, tooling |
| ADR 0303 | Rust style and de-slopification | all Rust lanes, hardening |

## Prototype Inventory And Outcomes

| Prototype Path | Outcome | Replacement Gate |
| --- | --- | --- |
| resource-member stable-id annotations as durable identity | Delete from source syntax and production identity | catalog lane records stable IDs outside source annotations |
| Textual saved paths as stable IDs | Debug/admin only | tree-cell store and tools expose typed store/catalog identities |
| Source-name physical keys | Delete production use | tree-cell physical keys derive from stable IDs and typed key values |
| Source-order enum ordinals as stored meaning | Delete production use | enum member stable identity is encoded and indexed |
| Whole-resource assignment with hidden clearing | Keep source law, expose destructive effects | checked write plan reports subtree clearing and requires tests |
| No-op or underspecified `lock` | Reject in production and remove from canonical docs | transaction lane defines v0.1 behavior without `lock` as a primitive |
| Saved `inout` or durable reference-like mutation | Reject in production | checked effects forbid saved `inout` writeback |
| Current `merge` surface as broad patch semantics | Reject, then replace with `edit` or checked transform semantics | runtime and evolution lanes define exact write or transform behavior |
| Hidden merge or implicit durable traversal | Reject; explicit durable `for` iteration is the v0.1 surface and platform/tool scans stream bounded chunks | durable-traversal facts lane |
| Runtime execution of syntax bodies | Delete production entry | checked IR runtime lane |
| Runtime string splitting or fallback resolution | Delete production use | checked model carries resolved IDs and saved places |
| Executable `Unknown` or diagnostic recovery | Delete from executable IR | checked model separates recovery from executable facts |
| Raw archive/data/serve protocols as stable APIs | Debug/admin only, typed wrappers for production | tooling and backup lanes |
| Out-of-band migration scripts as primary evolution | Delete from roadmap/product docs | source-native evolution lane |
| Remote clients opening engine files | Future-only, never v0.1 | tooling docs and protocol lane |

## Lane Graph

```text
plan
  -> v0.1 surface decision slice
  -> prototype rejection and docs alignment
  -> shared fixture skeleton
  -> checked model nucleus
  -> resource/store parser, schema, and store-owned indexes
  -> catalog identity binding and presence ledger
  -> tree-cell store and engine profile
  -> runtime checked execution and write planner
  -> source-native evolution
  -> tooling, backup, restore, and protocols
  -> deletion and hardening
```

The checker and runtime are hotspots. Do not run checker-heavy lanes in
parallel with other checker-heavy lanes. Store-only and docs-only lanes may run
beside checker design work if their file sets do not overlap.

Current hotspot map:

- `crates/marrow-check/src/checks.rs`, `infer.rs`, `resolve.rs`, `enums.rs`,
  `binding.rs`, and `program.rs` move together behind the checked-model lane.
- `crates/marrow-schema/src/lib.rs` is the collision point for resource/store,
  enum storage, indexes, stable IDs, and requiredness. Sequence those changes.
- `crates/marrow-check/src/facts.rs` and read typing are shared by resource/store
  facts and the ADR 0210 presence ledger. Do not run Lane 5 checked-facts work
  beside Lane 6 presence-ledger code.
- Lane 8 may add runtime-facing checked-fact APIs only after Lane 6 has
  integrated the catalog and presence-ledger facts. Treat this as a handoff, not
  concurrent checker ownership.
- `crates/marrow-run/src/call.rs`, `exec.rs`, `expr.rs`, `path.rs`,
  `schema_query.rs`, `write.rs`, and `write_dispatch.rs` form one vertical
  runtime replacement lane; do not split them into competing adapters.
- `crates/marrow-store/src/path.rs`, `value.rs`, `backend.rs`, `redb.rs`, and
  `archive.rs` can advance in a store worktree only after the catalog/tree-cell
  address shape is fixed.
- `crates/marrow/src/main.rs` and command modules are integration surfaces.
  Run CLI/tooling lanes after the fact, catalog, and store lanes expose stable
  APIs.

## Non-Authoritative Lane Snapshots

The sections below preserve historical lane context and completion criteria.
They are not executable prompts, file-ownership contracts, or current TDD
starts. The active per-orchestrator lane files under [`lanes/`](lanes/) are the
authority for Lane 5 and later. If a snapshot conflicts with a lane file, the
lane file wins; if the conflict would affect implementation, update or delete
the stale snapshot before assigning work.

### Lane 0: Plan Review And Baseline

Files:

- `docs/roadmap/prototype-to-v1-execution-plan.md`
- `docs/roadmap/README.md`

Acceptance:

- The plan has ADR traceability, lane order, file ownership, deletion targets,
  review strategy, and gates.
- Two senior read-only reviews pass: ADR coverage and implementation
  practicality.
- `git diff --check` passes.

Deletion target:

- None; this lane creates the execution control surface.

### Lane 1: V0.1 Surface Decision Slice

Files:

- `docs/roadmap/prototype-to-v1-execution-plan.md`
- focused docs under `docs/language/` only if the decision is being stated in
  canonical reference form

Production behavior:

- No production code changes.
- Senior reviewers ratify the language choices that would otherwise be invented
  inside rejection or parser/checker lanes:
  - split `store` declarations are canonical; the concise
    `resource Book at ^books(id: int)` form is accepted only as immediate
    parser sugar, and docs, facts, storage, and runtime internals use the split
    model;
  - `edit place` with nested assignment statements is the v0.1 grouped-update
    surface; current broad `merge` is prototype-only, rejected first, and
    deleted as `edit` and typed transforms land;
  - field defaults use field syntax, `name: T = <pure default>`, and participate
    in read totality and data-attached compatibility without forcing storage;
  - there is no broad "production mode" that keeps prototype semantics alive.
    Prototype constructs are rejected unconditionally as their replacements
    land, except for explicit debug/admin commands and production bridges with
    live callers, isolation boundaries, absence tests, and deletion lanes.

Fixture/oracle:

- Review records in the lane summary and plan update, not ADRs.
- The follow-on rejection lane can point to concrete choices instead of making
  them.

Deletion targets:

- None directly. This lane prevents later docs/checker patches from becoming
  design-by-accident.

Review lenses:

- Language/spec reviewer checks the choices are exactly those already allowed
  by the accepted ADR packet.
- Orchestration reviewer checks the choices unblock Lane 2 without widening
  v0.1.

### Lane 2: Prototype Rejection And Docs Alignment

Files:

- `docs/language/*.md`
- `docs/*.md`
- `docs/future/*.md`
- `crates/marrow-syntax/src/ast.rs`
- `crates/marrow-syntax/src/parse_decl.rs`
- `crates/marrow-syntax/src/format.rs`
- `crates/marrow-syntax/tests/*.rs`
- `crates/marrow-schema/src/lib.rs`
- `crates/marrow-schema/tests/*.rs`
- `crates/marrow-check/src/checks.rs`
- `crates/marrow-check/src/infer.rs`
- `crates/marrow-check/src/prototype.rs`
- `crates/marrow-check/tests/project.rs`
- `crates/marrow/tests/check_cli.rs`

Production behavior:

- Canonical docs describe v0.1 resource/store, requiredness, absence, write, and
  transaction laws.
- Prototype-only features are removed from canonical docs and explicitly
  deleted from syntax or rejected while old statement/expression spelling still
  parses during the rewrite.
- `docs/future/` contains only future constraints that are not v0.1 gates.
- `lock`, `merge`, `@id`, raw saved-path APIs, source-order enum ordinals, and
  saved `inout` stop reading like v0.1 commitments.

Fixture/oracle:

- A stale-term scan for `@id`, raw stable paths, enum ordinals, saved `inout`,
  production `lock`, broad `merge`, raw protocols, and migration scripts.
- Checker tests for rejected production constructs.

Deletion targets:

- Any canonical-doc claim that raw paths, `@id`, source-order enum ordinals,
  no-op `lock`, broad `merge`, or raw protocols are stable production
  contracts.

Review lenses:

- Language/spec reviewer verifies the docs now read like one product.
- Soundness reviewer searches for remaining prototype commitments.

### Lane 3: Shared V0.1 Fixture Skeleton

Files:

- `crates/marrow-check/tests/v01_fixtures.rs`
- `crates/marrow/tests/v01_cli.rs`
- test-support modules colocated with the crate that uses them

Production behavior:

- Establish the smallest shared fixture skeleton that later lanes extend.
- The skeleton contains source text and helper wiring for source-driven tests,
  but it does not fake catalog or logical data-slot capabilities that do not
  exist yet.
- Each later lane adds the failing production-pipeline assertion for its own
  capability.

Fixture/oracle:

- A `Book`/`Author` fixture can be loaded by checker now and run through the CLI
  production path now, then runtime, store, LSP, evolution, and backup tests as
  those lanes gain source-driven harnesses and catalog/logical data-slot facts.
- The initial lane proves only that the fixture is source-driven, shared, and
  not a second semantic classifier.
- The fixture names a typed reference field. Until the resource/store surface
  lane lands canonical `Id(^authors)` syntax, the shared source fixture uses the
  current executable spelling, `author: Author::Id`, as a temporary executable
  identity bridge for that relationship. Lane 5 must migrate the fixture to the
  canonical store-addressed spelling when it replaces resource-owned identity
  with store-owned aliases.

Deletion targets:

- Ad hoc giant tests that duplicate semantic classifiers once the shared
  fixture covers the same invariant.

Review lenses:

- Test-architecture reviewer checks that fixtures do not become another
  implementation of the compiler.
- Soundness reviewer checks that the fixture can expose source/catalog/data
  drift.

### Lane 4: Checked Model Nucleus

Files:

- `crates/marrow-check/src/program.rs`
- new focused modules under `crates/marrow-check/src/` for IDs, facts, effects,
  and durable places
- `crates/marrow-check/src/analysis.rs`
- `crates/marrow-check/tests/analysis_api.rs`
- `crates/marrow-check/tests/checked_program.rs`
- `crates/marrow-check/tests/binding_index.rs`

Production behavior:

- `CheckedProgram` carries a first checked-facts nucleus with typed IDs for
  modules, functions, resources, resource members, enums, enum members, and
  parameter locals. Store IDs, durable-place IDs, and full index/traversal facts
  remain follow-up work for the resource/store and runtime-replacement lanes.
- Diagnostics and recovery values do not appear in executable facts. Invalid
  signatures and unresolved member chains fail closed rather than publishing
  partial typed facts.
- Direct saved reads, direct saved writes, transactions, direct output/capability
  effects, and throws are represented as direct checked effects. Transitive
  user-call effects, durable traversal facts, platform bounded-scan needs,
  future checkpoint needs, and index usage remain follow-up work.
- Runtime-facing facts begin as non-production checked facts beside an explicit
  temporary syntax-body bridge. The bridge is named and marked for deletion by
  the runtime replacement lane; Lane 4 must not create a second production
  execution path.
- `~` is reserved for future typed ephemeral roots. The v1 checked-effect model
  leaves room to distinguish future ephemeral reads and writes, but no `~`
  declaration or runtime behavior is implemented in v1.
- The ADR 0210 proof ledger is not scattered into Lane 4 after the fact. Lane 6
  owns the first production ledger and makes runtime, evolution, and tools
  consume it.
- Because Lane 4 has already landed, active follow-up work owns ADR 0209 in two
  places: Lane 5 reserves and rejects `~` source forms, and Lane 8 proves the
  checked-effect/runtime path has no production `~` behavior while retaining a
  named future effect slot.

Fixture/oracle:

- Golden checked facts for resources, resource members, enums, functions, local
  parameters, direct saved reads, direct saved writes, direct effects, optional
  reads, and unresolved-name failures.
- Tests proving unresolved names, invalid signatures, partial member chains, and
  `Unknown` cannot enter executable facts.
- An architecture test proving checked executable facts do not store source
  `Block`, `Statement`, or `Expression` values. Any compatibility test must
  prove the old runtime path is absent from production execution or limited to an
  explicit debug/admin surface.

Deletion targets:

- Executable recovery facts and new runtime fallback paths.
- Production syntax-body execution is a Lane 8 deletion target, not a Lane 4
  acceptance requirement.

Review lenses:

- Soundness reviewer attacks same-named modules, resources, enums, stores, and
  shadowed locals.
- Idiom reviewer checks typed IDs, no glob-prelude expansion, and small modules.

### Lane 5: Resource/Store Surface, Schema Split, And Store-Owned Indexes

Files:

- `crates/marrow-syntax/src/ast.rs`
- `crates/marrow-syntax/src/parse_decl.rs`
- `crates/marrow-syntax/src/format.rs`
- `crates/marrow-syntax/tests/parse.rs`
- `crates/marrow-syntax/tests/format.rs`
- `crates/marrow-schema/src/lib.rs`
- `crates/marrow-schema/tests/compile_resource.rs`
- `crates/marrow-check/src/*.rs` only where store declarations bind into facts
- `docs/language/resources-and-storage.md`
- `docs/language/grammar.md`

Production behavior:

- Split `resource` and `store` declarations are the internal model.
- The concise `resource Book at ^books(id: int)` form desugars to a resource and
  store when kept for ergonomics.
- `Id(^store)` is the canonical identity type. `Book::Id` is not automatic
  resource identity; any surviving alias must be explicitly store-declared,
  absent from canonical fixtures by default, and reviewed as compatibility
  surface.
- Declared indexes belong to stores. Concise-form indexes are desugared into the
  generated store; production resource schemas do not own indexes as resource
  members.
- Unmarked fields are optional by default, `required` is explicit, and defaults
  are schema facts. Presence admission and read-totality proof flow through the
  Lane 6 ledger, not a Lane 5 classifier.
- Collections materialize as local trees, sequences, and keyed layers rather
  than a flat in-memory list type.
- Future placement and partition syntax remains reserved; Lane 5 does not add a
  source-level placement surface.
- ADR 0209 typed ephemeral root syntax is reserved and rejected in v0.1. Lane 5
  rejects source forms such as `cache ~...`, `ensure ~...`, `Id(~...)`, and
  top-level `~` roots instead of parsing them as production features.

Fixture/oracle:

- Parser/formatter round trips for split and concise forms.
- Checker tests for resource without store, two stores over one resource, and
  distinct `Id(^draftBooks)` vs `Id(^publishedBooks)`.
- Parser/schema tests for `store ^books(id: int): Book` with `index` and
  `unique index`; tests proving indexes are store-owned and source resource
  declarations cannot own production indexes except through concise desugaring.
- Parser/checker tests proving ADR 0209 `~` root forms are reserved and rejected,
  with no formatter output that presents them as v0.1 surface.
- Schema tests for indexed component laws: absent indexed components produce no
  entry unless the index declares a default, non-unique indexes include the store
  key tie-breaker, and composite index components preserve declared order.
- A `Book`/`Author` typed-reference fixture proving `author: Id(^authors)`
  checks and lowers as a typed value without implying joins, cascade delete, or
  automatic existence checks.
- Migration of the shared v0.1 fixture from current `Author::Id` spelling to the
  canonical `Id(^authors)` spelling.

Deletion targets:

- Resource-name-owned identity as a production type.
- Schema logic that treats saved roots as an inseparable property of resource
  declarations.
- Production `ResourceMember::Index` ownership and `ResourceSchema.indexes` as
  the durable owner of index identity.
- Relationship docs that recommend scalar-key workarounds where typed
  `Id(^store)` references are supported.

Review lenses:

- Spec reviewer checks syntax is minimal and matches docs.
- Soundness reviewer attacks cross-module resources, aliases, and identity
  confusion.

### Lane 6: Catalog Identity Binding And Presence Ledger

Files:

- new catalog modules under `crates/marrow-check/src/` or
  `crates/marrow-schema/src/`, chosen by the checked-model boundary
- `crates/marrow-check/src/facts.rs` and analysis/check modules for the
  per-read presence proof ledger
- `crates/marrow-project/src/lib.rs` if project catalog metadata enters config
- `crates/marrow-check/tests/project.rs`
- `crates/marrow/tests/check_project_cli.rs`
- `docs/data-evolution.md`
- `docs/project-config.md`

Production behavior:

- Source-only check proposes catalog changes without mutating accepted catalog
  metadata.
- Accepted catalog metadata is a generated file committed in the project source
  tree. It records stable IDs, aliases, lifecycle state, catalog epoch, and
  digest.
- The checked program records, per read, the proof source that admits it:
  declaration, narrowing, or pending attached-data proof.
- Source-only checks discharge declaration and narrowing proofs and leave
  attached-data obligations pending for activation.
- Data-attached checks compare source, accepted catalog, store snapshot, data
  snapshot, and engine profile before activation.
- Renames require source-native intent and preserve stable identity only when
  accepted.

Fixture/oracle:

- First compile proposes IDs.
- Source-only check leaves catalog epoch unchanged.
- Rename without intent fails closed.
- Accepted rename preserves stable identity without moving data cells.
- Fresh clone and source rollback fixtures fail or bind through explicit
  catalog metadata.
- A bare maybe-present read with no read-site resolution produces a source-check
  diagnostic. Positive tests prove `??`, `else`, `if let`, `if exists`, and
  optional chaining flow through the single proof ledger rather than duplicating
  read-totality classifiers. Attached-data obligations remain explicit pending
  facts until the data-attached check discharges them.

Deletion targets:

- Source `@id` annotations and metadata as source identity storage. Canonical
  source rejects or deletes `@id` entirely; allowed matches are rejection tests
  or historical/debug docs only.
- Any code that regenerates IDs to make a diff clean.

Review lenses:

- Soundness reviewer attacks branch conflicts, stale catalog epochs, alias
  reuse, source rollback, and reads whose proof was inferred outside the ledger.
- Idiom reviewer checks catalog metadata remains compiler/tooling
  infrastructure, not source syntax.

### Lane 7: Tree-Cell Store And Engine Profile

Depends on:

- Lane 5 and Lane 6 producing the logical address, stable ID, catalog epoch, and
  key profile. Before that point, store work is limited to read-only design,
  conformance notes, and engine-substrate checks that do not encode Marrow
  identity: ordered bytes, snapshots, one-writer behavior, rollback, typed
  engine errors, and read-only opens.

Files:

- `crates/marrow-store/src/backend.rs`
- `crates/marrow-store/src/path.rs`
- `crates/marrow-store/src/value.rs`
- `crates/marrow-store/src/mem.rs`
- `crates/marrow-store/src/redb.rs`
- `crates/marrow-store/src/archive.rs`
- `crates/marrow-store/src/conformance.rs`
- `crates/marrow-store/tests/*.rs`
- `docs/backend-contract.md`

Production behavior:

- Engine contract is ordered bytes, snapshots, one writer, transactions,
  internal range iterators, engine profile, and typed errors.
- Marrow tree-cell layer owns node, leaf, index, sequence, catalog/meta, and
  blob/chunk cells.
- Physical keys derive from stable IDs and typed key values, not source names.
- The key profile reserves the empty placement prefix from ADR 0208 without
  exposing placement syntax in v0.1.
- Commit metadata records commit id, catalog epoch, layout epoch, engine profile
  digest, changed roots, and changed indexes.
- Read-only opens are actually read-only and cannot accidentally acquire writer
  capabilities.

Fixture/oracle:

- Early engine-substrate conformance covers snapshots, one-writer behavior,
  rollback, typed engine errors, and read-only opens without constructing stable
  IDs, typed references, source-renamed keys, index cells, catalog epochs, or
  tree-cell physical addresses.
- Post-Lane-6 store conformance covers commit metadata, node-cell existence,
  leaf absence, source-rename-stable physical keys, enum reorder,
  range-iterator traversal, and sequence state.
- Post-Lane-6 index conformance covers absent components, non-unique
  tie-breakers, composite key ordering, binary string ordering,
  enum-reorder-stable meaning, unique duplicate rollback, duplicate build
  failure before publish, index build invisibility before verify, and data/index
  atomicity in one transaction.
- Post-Lane-6 typed-reference encoding proves an `Id(^authors)` value stores
  store identity plus key, not a raw scalar key, and remains stable across
  source renames.
- Crash/repair fixtures for clean commit, rollback, missing commit metadata,
  ambiguous commit refusal or read-only repair, and corrupt catalog/meta cells.

Deletion targets:

- Production path encoding that uses source root/member names as identity.
- Raw archive as portable backup contract.

Review lenses:

- Soundness reviewer attacks rollback, ambiguous commit, corrupt metadata,
  source rename, enum reorder, and index atomicity.
- Idiom reviewer checks redb stays an engine substrate and no semantic logic
  leaks into backend code.

### Lane 8: Runtime Checked Execution And Write Planner

Files:

- `crates/marrow-run/src/*.rs`
- `crates/marrow-run/tests/eval.rs` or split focused runtime fixtures
- `crates/marrow-check/src/` facts needed by runtime
- `crates/marrow/tests/run_cli.rs`
- `docs/language/control-flow-and-effects.md`
- `docs/language/resources-and-storage.md`

Production behavior:

- Runtime entry accepts checked executable facts or IR, not syntax bodies.
- Saved reads and writes use checked durable places.
- Runtime consumes the checked-program presence ledger for read totality and
  activation status; it does not recompute presence from syntax or store shape.
- The checked-effect model retains a named future slot for ADR 0209 ephemeral
  reads and writes, but runtime exposes no production `~` root behavior.
- Assignments, `edit`, `delete`, and assertions lower to write plans with
  explicit planner effects.
- Root assignment is exact and exposes subtree clearing effects.
- Field/path assignment and `edit` preserve omitted data and update indexes.
- Irreversible host effects are forbidden inside rollback-sensitive
  transactions.
- Local runtime materialization preserves tree, sequence, and keyed-layer
  shapes; no production runtime path invents a flat list collection model.
- The checked-effect model leaves a named future slot for principal or request
  context effects, but v0.1 does not implement users, roles, or permissions.
- `lock` is not a source-level production primitive unless a later reviewed lane
  defines semantics beyond the one-writer transaction model.

Fixture/oracle:

- Architecture test proving production runtime cannot execute raw syntax and no
  production `CheckedFunction.body: Block` bridge remains.
- Architecture test proving accidental `cache ~`, `ensure ~`, `Id(~...)`, or
  production `~` root behavior is absent from runtime execution.
- Runtime fixtures for exact root assignment, field assignment, `edit`, delete,
  existence assertions, transactions, nested rollback, optional/default reads,
  missing required production data, index maintenance, unique-index duplicate
  rollback, absent-index-component removal, typed-reference reads/writes without
  implicit joins/cascade/existence checks, and host effects.

Deletion targets:

- AST-body execution path for production runs.
- The temporary syntax-body bridge introduced during checked-model migration.
- Runtime string splitting for saved paths, function names, enum members, or
  resource identities.
- Saved `inout` writeback.
- Runtime schema and path classifiers that duplicate checked-model durable
  place facts.

Review lenses:

- Soundness reviewer mutates future loop elements, transaction branches, index
  entries, optional fields, and host effects.
- Idiom reviewer checks the runtime consumes facts and keeps planner
  classifications out of source syntax.

### Lane 9: Source-Native Evolution

Files:

- new evolution modules under `crates/marrow-check/src/` and/or
  `crates/marrow-run/src/`
- `crates/marrow/src/main.rs`
- new CLI modules for `catalog` and `evolve` commands
- `crates/marrow/tests/*evolve*.rs`
- `docs/data-evolution.md`
- `docs/cli.md`

Production behavior:

- `marrow check`, data-attached check, `catalog preview`, `catalog accept`,
  `evolve preview`, `evolve apply`, and repair are command surfaces over one
  proof-discharge pipeline. The commands differ in authority and side effects,
  but they consume the same catalog, proof-ledger, witness, snapshot, and engine
  classifications.
- Preview is read-only and produces an exact witness.
- Apply consumes only the exact witness and aborts on source, catalog, snapshot,
  engine, affected-ID, or count drift.
- V0.1 compatibility lenses are limited to rename/default compatibility and
  defaulting a newly required field.
- Catalog/runtime metadata declares compatibility windows explicitly; old and
  new binaries activate only inside those windows, and stale writers fail closed.

Fixture/oracle:

- Optional field add needs no rewrite.
- Required field with default reads old data.
- Rename requires source-native intent.
- Destructive approval missing/present/drift cases.
- Online index build is not visible to production queries before verify, and a
  failed build cannot publish partial index data.
- Split/merge transform is rejected as transform required unless the lane
  implements the checked transform; no migration-script or runtime shim stands
  in for it.
- Failed apply resumes or rolls back.
- Old-binary, new-binary, expired-window, and stale-writer fixtures prove
  compatibility windows are enforced.

Deletion targets:

- Migration-script framing as primary workflow.
- Silent source-diff identity preservation.

Review lenses:

- Soundness reviewer attacks witness drift, destructive approval scope, branch
  rollback, stale engine metadata, and backfill idempotence.
- Idiom reviewer checks user-facing terms are `rename`, `default`, `prove`,
  `transform`, `retire`, `rebuild`, and `repair`, not internal lens jargon.

### Lane 10: Tooling, Backup, Restore, And Protocols

Files:

- `crates/marrow/src/cmd_check.rs`
- `crates/marrow/src/cmd_data.rs`
- `crates/marrow/src/cmd_backup.rs`
- `crates/marrow/src/lsp.rs`
- `crates/marrow/src/serve/protocol.rs`
- focused backup, data, serve, LSP, and protocol tests under
  `crates/marrow/tests/`; do not claim `check_project_cli.rs`, `run_cli.rs`, or
  `*evolve*.rs`
- `docs/cli.md`
- `docs/lsp.md`
- `docs/serve-protocol.md`
- `docs/data-tools.md`

Production behavior:

- CLI, LSP, data tools, serve, backup, restore, and future adapters render
  shared compiler/runtime facts.
- Diagnostic and activation details render the shared presence ledger instead
  of rediscovering read totality in each tool.
- Raw physical keys and backend bytes are debug/admin only and disabled as
  stable production APIs.
- Data previews stream bounded chunks, preserve tree/sequence/keyed-layer
  shapes, and are snapshot-bound.
- Internal/tool continuations are catalog-epoch and snapshot bound; source
  language cursor/window values are not v0.1 surface.
- Backup is a typed Marrow artifact that validates source, catalog, data,
  engine profile, checksums, layout, codecs, indexes, and sequence state before
  activation.
- Lane 10 owns the minimum typed backup manifest and production backup/restore
  API as its first phase. Later adapter work consumes that manifest instead of
  defining backup semantics in CLI, serve, or LSP code.
- Runtime generation and stale-writer facts exist for local activation.
- Raw data inspection, raw serve operations, and raw archive import/export are
  explicit debug/admin surfaces, not the default production contract.

Fixture/oracle:

- CLI and LSP render the same diagnostic from the shared fixture.
- Raw debug protocols are opt-in.
- Stale epoch/snapshot/generation produce typed errors.
- Backup during concurrent read uses a stable snapshot.
- Backup/restore round-trips typed references as store identity plus key, not raw
  scalar keys.
- Restore rejects catalog/data/store mismatch and corrupt chunks.
- Restore verifies or rebuilds derived index data before exposing it, and
  preserves or safely repairs per-store sequence state.

Deletion targets:

- Raw data/serve protocol claims as stable production APIs.
- Tool-local re-resolution of source names or saved paths.
- Portable backup implemented as raw path/value dump.

Review lenses:

- Soundness reviewer attacks stale platform tokens, stale generations, restore
  mismatch, raw debug exposure, and unbounded previews.
- Idiom reviewer checks adapters stay thin and transport-specific.

### Lane 11: Rust De-Slopification And Hardening

Files:

- all Rust crates, sequenced by previous ownership boundaries
- `docs/future/` and stale docs

Production behavior:

- No duplicate production paths remain.
- No crate-root glob prelude grows as a replacement for explicit imports.
- Large tests are split only when the split removes real duplication and keeps
  production-pipeline fixtures clear.
- Prototype docs are deleted or folded into canonical references.

Fixture/oracle:

- Full workspace gate.
- `rg` scans for prototype terms, `unsafe`, raw stable path claims, executable
  `Unknown`, and old migration language.
- Diff review proving each old path is deleted, replaced, or limited to an
  explicit debug/admin surface, with no production bridges remaining.

Deletion targets:

- AST runtime production path.
- Source-name physical key production path.
- Raw archive production backup path.
- Runtime fallback resolution.
- Duplicate semantic classifiers in checker/runtime/schema/tools.
- Stale `docs/future` content whose constraints have moved into canonical docs.

Review lenses:

- Soundness reviewer checks removed paths are not still reachable.
- Idiom reviewer checks code is smaller, clearer, and aligned with Rust style.

## Next Implementation Lanes

Start in this order:

1. **Resource/store surface and schema split.** This moves identity and indexes
   onto stores and migrates typed references to `Id(^store)`.
2. **Catalog identity binding and presence ledger.** This introduces the
   committed accepted catalog file and the ADR 0210 proof-discharge facts that
   activation, runtime, tools, and evolution share.
3. **Tree-cell store and engine profile.** This moves physical data identity to
   stable catalog IDs, typed key values, and the reserved placement prefix.

Store conformance planning can happen in parallel after the fixture skeleton,
but early executable checks are limited to engine-substrate behavior that does
not construct Marrow identity. Store code or tests that encode physical keys,
typed references, index cells, catalog epochs, or tree-cell addresses wait until
the catalog and tree-cell address shape is fixed.

## Open Design Review Points

These points require senior review before their lane edits production code, but
they do not need new ADRs:

- The default project path and on-disk format for accepted catalog metadata.
- The minimal source-check proof ledger shape for declaration, narrowing, and
  pending attached-data obligations.
- The minimum checked IR shape that deletes syntax-body execution without
  inventing a low-level bytecode.
- The exact tree-cell physical key version and layout profile boundaries.
- The Lane 10 minimum typed backup manifest that is v0.1 portable without
  implementing engine recompile.
- The local runtime-generation state machine needed for stale-writer fencing
  without prematurely building a server.

Each lane plan must resolve its own point before code changes and record the
decision in the lane commit message or durable docs, not in a new ADR.

## Completion Criteria

This plan is complete only when:

- all lanes above are implemented or intentionally narrowed by a reviewed update
  to this plan;
- every prototype-only production path is deleted or limited to an explicit
  debug/admin surface;
- source, accepted catalog file, data snapshot, and redb engine profile compile
  together;
- resources and stores are distinct in the model;
- durable identity survives source rename/reorder through catalog decisions;
- presence proofs are recorded in one checked-program ledger and activation
  fails until every proof obligation is discharged;
- runtime executes checked facts/IR only;
- transactions, snapshots, rollback, backup, restore, durable traversal, and
  internal bounded-scan continuations are covered by production-pipeline
  fixtures;
- evolution preview/apply/verify and destructive approvals are implemented;
- CLI and LSP consume shared facts;
- full Rust/docs gates pass with no `unsafe`, no duplicate production
  semantics, and no documentation sediment.
