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

## Completion Claim Discipline

Recent lane failures exposed a recurring prompt problem: agents fixed the first
review finding, ran green gates, and reported "done" while sibling surfaces,
roadmap status, or code shape still contradicted the claim. Treat completion as
an evidence packet, not a feeling.

Every lane handoff must use exactly one status label:

- **audit complete**: read-only inventory or verdict matrix is finished, but
  production code has not started or dependencies are missing;
- **code ready for review**: failing tests were written, implementation is
  present, focused gates pass, and the worktree is intentionally dirty for
  read-only review;
- **blocked**: a named dependency, missing semantic fact, or owning-lane file
  conflict prevents meaningful progress;
- **lane complete**: all implementation, docs, tests, sibling scans, focused
  gates, full gates, review findings, and lane-status updates are finished.

"Done" and "perfect" mean **lane complete**. A lane is not complete if its
tracked lane doc still says active, repair, blocked, read-only only, or lists
unresolved owning-lane blockers. A lane that stops after producing a verdict
matrix reports **audit complete**, not done.

Every lane-complete claim must include:

- exact base/head commits, branch, worktree, and clean/dirty status;
- changed files, including deleted files;
- focused red/green evidence and full verification output with an explicit
  isolated `CARGO_TARGET_DIR`;
- reviewer verdicts and how every finding was fixed;
- the lane doc status after the edit;
- feature-surface verdicts for every suspect surface in the lane area;
- a sibling-surface scan proving the fixed weak foundation is gone across the
  lane's owned APIs, not only at the cited review line;
- code-shape evidence for touched Rust hotspots: broad dispatchers split,
  low-value comments removed, duplicate helpers deleted, and test aggregators
  not enlarged when a focused fixture would do.

When a review finding names a family of smells, the next build pass must inspect
the family. Examples:

- after fixing one unbounded saved-data loop, scan `keys`, `values`, `entries`,
  `count`, `exists`, data previews, and protocol child/walk APIs;
- after replacing one raw saved-path parser, scan CLI, serve, backup, LSP, docs,
  and tests for the same raw path identity model;
- after splitting one broad dispatcher, scan the sibling module for new
  all-in-one helpers and comment blocks that only explain branch structure;
- after demoting one prototype feature, scan docs and tests so the feature is no
  longer presented as a normal v0.1 production surface.

Use one cargo target directory per lane, for example:

```sh
CARGO_TARGET_DIR=<outside-repo-target-dir> \
    cargo test --manifest-path <lane-worktree>/Cargo.toml ...
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

Current status:

Completed foundations:

- [Lane 5](lanes/lane-05-resource-store-surface.md) supplies the v0.1
  resource/store surface: resources and stores are distinct, and saved roots are
  declared by `store ^root(...)`.
- [Lane 6](lanes/lane-06-catalog-presence-ledger.md) supplies catalog-backed
  enum identity and the presence ledger consumed by runtime and tooling.
- [Lane 7](lanes/lane-07-tree-cell-store-engine.md) supplies the typed tree-cell
  store keyed by stable catalog ids, with typed key values, sequence state,
  index cells, commit metadata, and an explicit engine profile.
- [Lane 8](lanes/lane-08-runtime-checked-execution.md) supplies checked runtime
  execution, checked durable traversal, write planning, and enum/index runtime
  value handling. Runtime no longer has a syntax-body production path or a
  string-backed checked entry call.
- [Lane 9](lanes/lane-09-evolution-activation.md) supplies source-native
  evolution and an invisible, content-independent catalog identity: one
  fail-closed, identity-aware proof-discharge pipeline (`check`, `check --data`,
  `evolve preview`/`apply`) over an exact witness, with stale-writer fencing on
  catalog epoch, engine profile, and schema digest.
- [Lane 10](lanes/lane-10-tooling-backup-protocols.md) supplies the typed
  backup/restore artifact, checked read-only data inspection, loopback
  `debug_data_*` serve inspection, and the tooling boundary that keeps raw/path
  surfaces out of production semantics.

Active follow-up work is tracked here and in the current lane files. The
research synthesis is archived evidence, not a second roadmap or prompt
tracker. The next docs/ADR step is to keep accepted ADRs and canonical docs
aligned with the settled v0.1 architecture. Remaining implementation work is
limited to explicit follow-up lanes such as future online activation,
language-surface simplification, product-decision closure, and final Rust
hardening.

## Active Quality Intervention

The active quality intervention is no longer a pre-Lane-10 audit. The current
intervention is consistency: accepted ADRs, canonical docs, lane files, and
future implementation prompts must agree that source is the access path,
identity is `Id(^store)`, restore is compiler-owned and orphan-rejecting,
`unknown` is not `any`, `merge`/`lock` are reserved not supported syntax, and
data/serve rawness is debug/admin only. They must also agree that v0.1
activation is exact-epoch and fail-closed, while future online activation is
compiler-mediated through bounded jobs, generated/deleted adapters, finite
compatibility windows, and shadow decant for major reshapes.

Every active lane must classify suspect surfaces in its area as keep production,
debug/admin only, rename/rescope, delete, or product-decision pending, then turn
the verdict into code, docs, tests, or an owning-lane blocker before review.

Every active orchestrator must include these items in the next handoff:

- the exact weak foundation being removed, not just the feature being added;
- the feature-surface verdicts in the lane's area, including deletes and
  debug/admin demotions;
- the code-shape cleanup performed before review;
- the absence scan proving no production caller can choose the old model;
- any old test or fixture deleted or migrated because it depended on prototype
  behavior;
- a statement that no roadmap wording was narrowed to hide unfinished deletion.

## Scott-Pending Product Decisions

These questions block narrow parts of later lanes. They are tracked here so
archived research documents do not become a second active backlog:

- whether `out` exists in v0.1 or whether returned values cover the use case;
- future compatibility-window defaults for server mode, including one-old-epoch
  policy and old-write admission;
- required-field `default` meaning: temporary activation fill or durable read
  default;
- re-key identity semantics: new store plus explicit transform/decant by
  default, or narrowly proven identity-preserving cases.

## Parallel Orchestrator Split

Assign one lead orchestrator per lane file. Parallelize design scans, review,
and file-disjoint implementation; sequence production code when two lanes would
edit the checker/schema identity surface.

| Track | Lane Plan | Can Start Now | Code Timing | Collision Boundary |
| --- | --- | --- | --- | --- |
| Catalog/presence corrective | [Lane 6](lanes/lane-06-catalog-presence-ledger.md) | Complete | Integrated; future edits are regressions or Lane 11 cleanup findings | Owns checker/schema enum identity and presence classifier cleanup; no runtime/store physical-key edits. |
| Runtime | [Lane 8](lanes/lane-08-runtime-checked-execution.md) | Complete | Integrated; future edits are regressions or Lane 11 cleanup findings | Owns runtime checked execution and enum value/index conversion; no syntax-body compatibility path survives. |
| Evolution | [Lane 9](lanes/lane-09-evolution-activation.md) | Complete | Integrated; future edits are regressions, Lane 14 online-activation implementation work, or Lane 11 cleanup findings | Owns one v0.1 proof-discharge pipeline with command-specific surfaces; future windows/adapters/decant remain compiler-owned job facts. |
| Tooling/protocols | [Lane 10](lanes/lane-10-tooling-backup-protocols.md) | Integrated foundation | Follow-up work owns future activation rendering, future local API shape, and hardening that keeps diagnostic/admin commands fenced | No unsupported commands, raw public protocols, restore that imports orphaned managed cells, execution-strategy claims, migration-ledger claims, or tool-local semantic classifiers. |
| Hardening | [Lane 11](lanes/lane-11-rust-hardening.md) | Read-only scans anytime | Final fixes after owning lanes land, except truly file-disjoint style fixes | Owns deletion proof, not postponed semantic rewrites. |

Lane 8 owns the runtime as one vertical replacement so the project does not grow
a compatibility shim between syntax execution and checked execution.

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
| Current `merge` surface as broad partial-update semantics | Reject as v0.1 syntax; field writes express partial changes, and transforms handle source-native evolution | runtime and evolution lanes define exact write or checked transform behavior |
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
- Lane 8 has consumed the catalog, presence-ledger, and tree-cell facts for
  runtime-facing checked execution. Future runtime changes should preserve that
  single checked-fact boundary rather than reintroducing tool-local classifiers.
- `crates/marrow-run/src/call.rs`, `expr.rs`, `path.rs`, `read.rs`,
  `write.rs`, and `write_dispatch.rs` remain one runtime boundary; do not split
  them into competing semantic adapters.
- Runtime and tooling lanes consume the integrated store profile; they do not
  mutate tree-cell physical keys or re-promote raw saved paths to production
  semantics.
- `crates/marrow/src/main.rs` and command modules are integration surfaces.
  Run CLI/tooling lanes after the fact, catalog, and store lanes expose stable
  APIs.

## Open Design Review Points

These points require senior review before their lane edits production code, but
they do not need new ADRs:

- The minimum checked IR shape that deletes syntax-body execution without
  inventing a low-level bytecode.
- The future local API and diagnostic-command hardening points that keep
  diagnostics from becoming execution-strategy or raw-path production APIs.
- The future online-activation facts surface: activation job status, chunk
  progress, verification findings, publish readiness, compatibility-window
  admission, adapter names, and close conditions without creating a migration
  ledger.
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
