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
   - idiom/spec: verify minimality, local style, ADR traceability, docs, and no
     slop.
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

Use one cargo target directory per lane, for example:

```sh
CARGO_TARGET_DIR=/private/tmp/marrow-targets/<lane>
cargo test --manifest-path /private/tmp/marrow-worktrees/<lane>/Cargo.toml ...
```

Never run broad cargo gates in parallel against the same target directory.

## Prototype Removal Controls

The pivot from prototype to v0.1 is the central constraint. A lane that adds the
new architecture but leaves the prototype production path quietly reachable has
not succeeded.

Every implementation lane must include a **prototype removal ledger** in its
lane plan:

- replacement behavior: the v0.1 behavior that becomes authoritative;
- old production paths touched by the lane;
- duplicate classifiers or duplicate resolution logic that must disappear;
- temporary bridge, only when unavoidable, with the exact later lane that
  deletes it;
- architecture absence tests or scans that prove the old path is no longer
  production-reachable;
- docs that must be deleted, folded into canonical references, or marked
  debug/admin only.

Temporary bridges are allowed only when they keep a vertical rewrite shippable
without lying about the destination. A bridge must be named, isolated, reviewed,
and assigned to a deletion lane at creation time. A bridge may not create a new
semantic owner. The only acceptable runtime-replacement bridge is a
syntax-to-IR adapter that keeps the old runtime callable for one named lane, and
it may not preserve runtime name resolution as a second checker.
It is a failed bridge if production callers can choose between old and new
semantics or if the bridge survives past its named deletion lane.

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
- new crate-root glob preludes are forbidden, and existing glob-prelude patterns
  are deletion targets when a lane rewrites that crate boundary;
- tests must move toward source-driven production fixtures instead of adding
  more catch-all assertions to giant files;
- `#[allow(dead_code)]`, unused public APIs, fallback lookup helpers, and
  "just for compatibility" functions are deleted unless a reviewer records the
  current non-production owner and deletion lane.

The hardening lane is not a place to postpone known deletion. It is the final
audit that proves previous lanes deleted what they said they would delete.

## Strong Foundation Rule

For v0.1, weak foundations are blockers, not debt. A lane may not build on top
of a prototype foundation that the accepted ADR packet rejects. When a lane
finds one, it must choose one of these outcomes before integration:

- replace it with the v0.1 foundation and delete the old path;
- limit it to an explicit debug/admin boundary that is part of the v0.1
  product, or to a named temporary bridge with a deletion lane;
- stop and run a design review for the replacement shape.

The following foundations are mandatory before dependent breadth work:

| Foundation | Must Be True Before | Weak Foundation To Remove |
| --- | --- | --- |
| Checked facts and IDs are authoritative | runtime, tools, evolution | runtime/source/tool re-resolution |
| Catalog identity exists and owns durable IDs | tree-cell storage, backup, evolution | `@id`, source spelling, regenerated IDs |
| Resource/store split is internal model | runtime writes, indexes, references | fused resource/root/schema ownership |
| Store-owned indexes are explicit facts | index maintenance, scans, backup | resource-owned production indexes |
| Tree-cell storage keys derive from stable IDs | redb layout, backup, restore | source-name encoded physical keys |
| Runtime executes checked IR/facts | production `run`, transactions, tools | AST-body execution and dynamic fallback lookup |
| Evolution has exact witnesses | destructive changes, catalog apply | source-diff inference or migration-script framing |
| Tools consume shared facts | CLI/LSP/serve/data protocols | tool-local semantic classifiers |

If a proposed lane cannot strengthen one of these foundations directly or
remove a weak foundation that blocks it, it is not an implementation lane for
this phase.

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
| ADR 0200 | user model, compile/apply modes, requiredness, write, absence, and reference laws | language surface, checked model, runtime, fixtures |
| ADR 0201 | checked facts, durable places, effects, IR, no executable recovery | checked model, runtime replacement, tooling |
| ADR 0202 | resource/store split, store-aware identity, typed references, typed tree cells | parser/schema/checker, catalog, tree-cell store, fixtures |
| ADR 0203 | source-native evolution, witnesses, compatibility windows, approvals | catalog, evolution, tooling |
| ADR 0204 | engine contract, tree cells, transactions, commit metadata, backup | tree-cell store, runtime, backup |
| ADR 0205 | shared facts, local generations, raw debug only | tools and protocols |
| ADR 0206 | catalog lifecycle and identity binding | catalog |
| ADR 0207 | store-owned indexes, index key laws, bounded scans, cursors, sequence laws | resource/store parser and schema, catalog, tree-cell store, checked model, runtime, backup |
| ADR 0303 | Rust style and de-slopification | all Rust lanes, hardening |

## Prototype Inventory And Outcomes

| Prototype Path | Outcome | Replacement Gate |
| --- | --- | --- |
| `@id("...")` as durable identity | Reject and delete as production identity | catalog lane records stable IDs outside source annotations |
| Textual saved paths as stable IDs | Debug/admin only | tree-cell store and tools expose typed store/catalog identities |
| Source-name physical keys | Delete production use | tree-cell physical keys derive from stable IDs and typed key values |
| Source-order enum ordinals as stored meaning | Delete production use | enum member stable identity is encoded and indexed |
| Whole-resource assignment with hidden clearing | Keep source law, expose destructive effects | checked write plan reports subtree clearing and requires tests |
| No-op or underspecified `lock` | Reject in production and remove from canonical docs | transaction lane defines v0.1 behavior without `lock` as a primitive |
| Saved `inout` or durable reference-like mutation | Reject in production | checked effects forbid saved `inout` writeback |
| Current `merge` surface as broad patch semantics | Reject, then replace with `edit` or checked transform semantics | runtime and evolution lanes define exact write or transform behavior |
| Unbounded merge or traversal over durable subtrees | Reject unless bounded or explicitly budgeted | scan/cursor facts lane |
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
  -> catalog identity binding
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
- `crates/marrow-run/src/call.rs`, `exec.rs`, `expr.rs`, `path.rs`,
  `schema_query.rs`, `write.rs`, and `write_dispatch.rs` form one vertical
  runtime replacement lane; do not split them into competing adapters.
- `crates/marrow-store/src/path.rs`, `value.rs`, `backend.rs`, `redb.rs`, and
  `archive.rs` can advance in a store worktree only after the catalog/tree-cell
  address shape is fixed.
- `crates/marrow/src/main.rs` and command modules are integration surfaces.
  Run CLI/tooling lanes after the fact, catalog, and store lanes expose stable
  APIs.

## Lane 0: Plan Review And Baseline

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

## Lane 1: V0.1 Surface Decision Slice

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
    land, except for explicit debug/admin commands and named temporary bridges.

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

## Lane 2: Prototype Rejection And Docs Alignment

Files:

- `docs/language/*.md`
- `docs/*.md`
- `docs/future/*.md`
- `crates/marrow-check/src/checks.rs`
- `crates/marrow-check/tests/project.rs`
- `crates/marrow/tests/check_cli.rs`

Production behavior:

- Canonical docs describe v0.1 resource/store, requiredness, absence, write, and
  transaction laws.
- Prototype-only features are removed from canonical docs and explicitly
  rejected/diagnosed while any old spelling still parses during the rewrite.
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

## Lane 3: Shared V0.1 Fixture Skeleton

Files:

- `crates/marrow-check/tests/v01_fixtures.rs`
- `crates/marrow-run/tests/v01_fixtures.rs`
- `crates/marrow-store/tests/v01_fixtures.rs`
- `crates/marrow/tests/v01_cli.rs`
- test-support modules colocated with the crate that uses them

Production behavior:

- Establish the smallest shared fixture skeleton that later lanes extend.
- The skeleton contains source text, expected catalog/data slots, and
  helper wiring for source-driven tests, but it does not fake capabilities that
  do not exist yet.
- Each later lane adds the failing production-pipeline assertion for its own
  capability.

Fixture/oracle:

- A `Book`/`Author` fixture can be loaded by checker, runtime, store, CLI/LSP,
  evolution, and backup tests as those lanes become real.
- The initial lane proves only that the fixture is source-driven, shared, and
  not a second semantic classifier.
- The fixture names a typed reference field, `author: Id(^authors)`, so later
  checker/runtime/store/backup lanes inherit one reference oracle instead of
  creating incompatible relationship examples.

Deletion targets:

- Ad hoc giant tests that duplicate semantic classifiers once the shared
  fixture covers the same invariant.

Review lenses:

- Test-architecture reviewer checks that fixtures do not become another
  implementation of the compiler.
- Soundness reviewer checks that the fixture can expose source/catalog/data
  drift.

## Lane 4: Checked Model Nucleus

Files:

- `crates/marrow-check/src/program.rs`
- new focused modules under `crates/marrow-check/src/` for IDs, facts, effects,
  and durable places
- `crates/marrow-check/src/analysis.rs`
- `crates/marrow-check/tests/analysis_api.rs`
- `crates/marrow-check/tests/checked_program.rs`
- `crates/marrow-check/tests/binding_index.rs`

Production behavior:

- `CheckedProgram` carries typed IDs for modules, functions, resources, stores,
  fields, layers, indexes, enums, enum members, locals, and durable places.
- Diagnostics and recovery values do not appear in executable facts.
- Saved reads, writes, transactions, host effects, scan bounds, cursor needs,
  and index usage are represented as checked effects.
- Runtime-facing facts begin as non-production checked facts beside an explicit
  temporary syntax-body bridge. The bridge is named and marked for deletion by
  the runtime replacement lane; Lane 4 must not create a second production
  execution path.

Fixture/oracle:

- Golden checked facts for resources, stores, enums, functions, saved reads,
  saved writes, effects, optional reads, and unresolved-name failures.
- Tests proving unresolved names and `Unknown` cannot enter executable facts.
- An architecture test proving checked executable facts do not store source
  `Block`, `Statement`, or `Expression` values. Any compatibility test must
  name the temporary bridge, prove it is not a second production execution path,
  and point to the Lane 8 deletion.

Deletion targets:

- Executable recovery facts and new runtime fallback paths.
- The temporary syntax-body bridge is a named Lane 8 deletion target, not a
  Lane 4 acceptance requirement.

Review lenses:

- Soundness reviewer attacks same-named modules, resources, enums, stores, and
  shadowed locals.
- Idiom reviewer checks typed IDs, no glob-prelude expansion, and small modules.

## Lane 5: Resource/Store Surface, Schema Split, And Store-Owned Indexes

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
- `Id(^store)` is the canonical identity type; aliases like `Book::Id` are
  store-declared aliases, not automatic resource identity.
- Declared indexes belong to stores. Concise-form indexes are desugared into the
  generated store; production resource schemas do not own indexes as resource
  members.
- Unmarked fields are optional by default, `required` is explicit, and defaults
  make reads total without forcing physical storage.

Fixture/oracle:

- Parser/formatter round trips for split and concise forms.
- Checker tests for resource without store, two stores over one resource, and
  distinct `Id(^draftBooks)` vs `Id(^publishedBooks)`.
- Parser/schema tests for `store ^books(id: int): Book` with `index` and
  `unique index`; tests proving indexes are store-owned and source resource
  declarations cannot own production indexes except through concise desugaring.
- Schema tests for indexed component laws: absent indexed components produce no
  entry unless the index declares a default, non-unique indexes include the store
  key tie-breaker, and composite index components preserve declared order.
- A `Book`/`Author` typed-reference fixture proving `author: Id(^authors)`
  checks and lowers as a typed value without implying joins, cascade delete, or
  automatic existence checks.

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

## Lane 6: Catalog Identity Binding

Files:

- new catalog modules under `crates/marrow-check/src/` or
  `crates/marrow-schema/src/`, chosen by the checked-model boundary
- `crates/marrow-project/src/lib.rs` if project catalog metadata enters config
- `crates/marrow-check/tests/project.rs`
- `crates/marrow/tests/check_project_cli.rs`
- `docs/data-evolution.md`
- `docs/project-config.md`

Production behavior:

- Source-only check proposes catalog changes without mutating accepted catalog
  metadata.
- Accepted catalog metadata records stable IDs, aliases, lifecycle state,
  catalog epoch, and digest.
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

Deletion targets:

- Source annotations as required stable ID storage.
- Any code that regenerates IDs to make a diff clean.

Review lenses:

- Soundness reviewer attacks branch conflicts, stale catalog epochs, alias
  reuse, and source rollback.
- Idiom reviewer checks catalog metadata remains compiler/tooling
  infrastructure, not source syntax.

## Lane 7: Tree-Cell Store And Engine Profile

Depends on:

- Lane 5 and Lane 6 producing the logical address, stable ID, catalog epoch, and
  key profile. Before that point, store work is limited to read-only design,
  conformance notes, and tests that do not change production physical keys.

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
  bounded cursors, engine profile, and typed errors.
- Marrow tree-cell layer owns node, leaf, index, sequence, catalog/meta, and
  blob/chunk cells.
- Physical keys derive from stable IDs and typed key values, not source names.
- Commit metadata records commit id, catalog epoch, layout epoch, engine profile
  digest, changed roots, and changed indexes.
- Read-only opens are actually read-only and cannot accidentally acquire writer
  capabilities.

Fixture/oracle:

- Store conformance for snapshots, one-writer behavior, rollback, commit
  metadata, node-cell existence, leaf absence, source-rename-stable physical
  keys, enum reorder, bounded scans, and sequence state.
- Index conformance for absent components, non-unique tie-breakers, composite key
  ordering, binary string ordering, enum-reorder-stable meaning, unique duplicate
  rollback, duplicate build failure before publish, index build invisibility
  before verify, and data/index atomicity in one transaction.
- Typed-reference encoding proves an `Id(^authors)` value stores store identity
  plus key, not a raw scalar key, and remains stable across source renames.
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

## Lane 8: Runtime Checked Execution And Write Planner

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
- Assignments, `edit`, `delete`, and assertions lower to write plans with
  explicit planner effects.
- Root assignment is exact and exposes subtree clearing effects.
- Field/path assignment and `edit` preserve omitted data and update indexes.
- Irreversible host effects are forbidden inside rollback-sensitive
  transactions.
- `lock` is not a source-level production primitive unless a later reviewed lane
  defines semantics beyond the one-writer transaction model.

Fixture/oracle:

- Architecture test proving production runtime cannot execute raw syntax and no
  production `CheckedFunction.body: Block` bridge remains.
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

## Lane 9: Source-Native Evolution

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
  `evolve preview`, `evolve apply`, and repair semantics are distinct.
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

## Lane 10: Tooling, Backup, Restore, And Protocols

Files:

- `crates/marrow/src/cmd_check.rs`
- `crates/marrow/src/cmd_data.rs`
- `crates/marrow/src/cmd_backup.rs`
- `crates/marrow/src/lsp.rs`
- `crates/marrow/src/serve/protocol.rs`
- `crates/marrow/tests/*`
- `docs/cli.md`
- `docs/lsp.md`
- `docs/serve-protocol.md`
- `docs/data-tools.md`

Production behavior:

- CLI, LSP, data tools, serve, backup, restore, and future adapters render
  shared compiler/runtime facts.
- Raw physical keys and backend bytes are debug/admin only and disabled as
  stable production APIs.
- Data previews are bounded and snapshot-bound.
- Cursors are catalog-epoch and snapshot bound.
- Backup is a typed Marrow artifact that validates source, catalog, data,
  engine profile, checksums, layout, codecs, indexes, and sequence state before
  activation.
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

- Soundness reviewer attacks stale cursors, stale generations, restore mismatch,
  raw debug exposure, and unbounded previews.
- Idiom reviewer checks adapters stay thin and transport-specific.

## Lane 11: Rust De-Slopification And Hardening

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
  explicit debug/admin surface, with no temporary bridges remaining.

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

## First Four Implementation Lanes

Start in this order:

1. **V0.1 surface decision slice.** This resolves the exact language choices
   that rejection must encode.
2. **Prototype rejection and docs alignment.** This fixes the public contract
   before code begins to move and gives later lanes a stable target.
3. **Shared v0.1 fixture skeleton.** This prevents the rewrite from growing
   disconnected test replicas without faking future capabilities.
4. **Checked model nucleus.** This unlocks resource/store, runtime, tooling,
   and evolution replacement lanes.

Store conformance planning can happen in parallel after the fixture skeleton,
but store code that changes physical key or address shape waits until the
catalog and tree-cell address shape is fixed.

## Open Design Review Points

These points require senior review before their lane edits production code, but
they do not need new ADRs:

- The project file location and format for accepted catalog metadata.
- The minimum checked IR shape that deletes syntax-body execution without
  inventing a low-level bytecode.
- The exact tree-cell physical key version and layout profile boundaries.
- The minimum typed backup manifest that is v0.1 portable without implementing
  engine recompile.
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
- source, catalog, data snapshot, and redb engine profile compile together;
- resources and stores are distinct in the model;
- durable identity survives source rename/reorder through catalog decisions;
- runtime executes checked facts/IR only;
- transactions, snapshots, rollback, backup, restore, and bounded scans are
  covered by production-pipeline fixtures;
- evolution preview/apply/verify and destructive approvals are implemented;
- CLI and LSP consume shared facts;
- full Rust/docs gates pass with no `unsafe`, no duplicate production
  semantics, and no documentation sediment.
