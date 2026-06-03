# Lane 6: Catalog Identity Binding And Presence Ledger

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> This lane is checker/catalog critical and consumes Lane 5's stable
> resource/store facts.

Goal: bind durable identity through a committed accepted catalog file and record
one checked-program presence proof ledger that source checks, activation,
runtime, evolution, and tools consume.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-06-perfect`

Target dir: `/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect`

Status: complete/integrated for the catalog member identity and presence-ledger
foundation. Lane 8 has consumed these facts for runtime enum-value conversion
and index maintenance; future runtime issues are regressions or owning-lane
blockers, not pending Lane 6 work.

## Parallel Safety

This lane consumes Lane 5's stable resource, store, identity, and index facts.
Do not edit store physical key code, runtime execution, or source-native
evolution here.

Own these files during the code pass:

- `crates/marrow-check/src/facts.rs`
- checker catalog modules under `crates/marrow-check/src/` or schema modules if
  the boundary review chooses schema ownership
- `crates/marrow-check/src/analysis.rs`
- `crates/marrow-check/src/checks.rs`
- `crates/marrow-project/src/lib.rs` if project catalog metadata enters config
- `crates/marrow-check/tests/project.rs`
- `crates/marrow/tests/check_project_cli.rs`
- `docs/project-config.md`
- `docs/data-evolution.md`
- `docs/error-codes.md`

## Feature-Surface Audit Gate

Lane 6 owns the verdicts for language-source identity, catalog identity, enum
member identity, and read-presence proof surfaces. Before review, classify each
suspect surface as keep production, debug/admin only, rename/rescope, or delete.

Known catalog/presence suspects:

- `@id` source annotations: delete as accepted source identity; allowed only in
  rejection tests, diagnostics, or historical/debug context.
- Source spelling, raw saved paths, regenerated IDs, and source-name identity:
  delete as durable identity. Accepted catalog metadata is the only stable
  identity owner.
- Enum declaration order or pre-order ordinal as stored meaning or index-key
  meaning: delete. Declaration order may survive only as source traversal order.
- Maybe-present/read-totality helpers: keep only through the checked proof
  ledger; delete scattered helper predicates that independently decide read
  admission.
- Hidden catalog state outside committed project metadata or engine metadata:
  delete or reject as a production surface.
- User, role, permission, and principal/request-context identity: future-reserved
  only; do not let catalog facts imply a v0.1 permissions surface.

If the verdict needs runtime enum value conversion, store physical keys,
tooling/protocol rendering, or evolution apply behavior, return a blocker to the
owning lane instead of patching around it.

## Area Cleanup Gate

This lane owns the complete cleanup of catalog identity and read-presence
admission across checker facts, catalog metadata, project loading, diagnostics,
docs, fixtures, and tests. It must delete source-owned stable identity and ad
hoc read-presence paths in its area instead of leaving a second proof model for a
later lane.

Before handing the lane to review:

- complete the catalog/presence feature-surface verdicts and turn them into
  tests, docs updates, deletions, or owning-lane blockers;
- migrate enum stored meaning and enum index-key meaning away from
  declaration-order ordinals to catalog member identity, with source reorder
  fixtures proving meaning survives;
- keep declaration order only as a source traversal index, never as durable
  stored meaning;
- delete or consolidate duplicated saved-path, builtin-call, read-target, and
  proof-source classifiers in facts and presence modules;
- split broad presence AST walkers when the same semantic facts are already
  collected elsewhere, or make one pass the canonical owner and delete the
  duplicate pass;
- split catalog file handling, identity binding, epoch/digest validation,
  read-presence proof recording, and diagnostics into focused helpers or modules
  with one invariant each;
- migrate or delete tests, fixtures, and callers that depend on regenerated IDs,
  source-order enum ordinals, `@id`, or ad hoc maybe-read behavior instead of
  keeping legacy compatibility paths for them;
- keep proof-source classification in one ledger path, not scattered helper
  predicates;
- delete dead `@id`, regenerated-ID, read-totality, and maybe-present helpers
  introduced or exposed by this lane;
- delete comments that narrate branch structure, explain temporary migration
  state, or compensate for oversized functions;
- preserve only comments that explain durable identity, epoch, digest, or
  soundness constraints;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  functions, duplicate proof classifiers, compatibility glue, comment sediment,
  and lane-local cleanup deferred to Lane 11.

## Production Contract

- Source-only check proposes catalog changes without mutating accepted catalog
  metadata.
- Accepted catalog metadata is a generated file committed in the project source
  tree. It records stable IDs, aliases, lifecycle state, catalog epoch, and
  digest.
- The checked program records, per read, the proof source: declaration,
  narrowing, or pending attached-data proof.
- Source-only checks discharge declaration and narrowing proofs and leave
  attached-data obligations pending.
- Data-attached checks compare source, accepted catalog, store snapshot, data
  snapshot, and engine profile before activation.
- Renames require source-native intent and preserve stable identity only when
  accepted.
- The checked-effect model leaves space for future principal/request-context
  effect classes, but v0.1 does not implement users, roles, or permissions.
- Catalog and presence surfaces without an ADR-backed user story are deleted or
  rejected before other lanes can build on them.

## Prototype Removal Ledger

Replacement behavior: catalog metadata, not source annotations or source names,
owns stable durable identity; the proof ledger, not scattered helper checks,
owns read-presence admission.

Delete or reject:

- source `@id` annotations and metadata entirely from canonical source identity;
  allowed matches are rejection tests or historical/debug docs only;
- regenerated IDs that make a diff clean;
- ad hoc read-totality classifiers outside the checked ledger;
- source-order enum ordinals as stored meaning or index-key meaning;
- catalog state hidden outside the source tree or engine metadata;
- any tool or runtime read proof inferred without a ledger entry.

Production bridge: none for stable identity. A pending attached-data proof is an
obligation, not an executable success path.

## TDD Start

Write failing production-pipeline checks first:

- first compile proposes stable IDs without changing the accepted catalog file;
- source-only check leaves catalog epoch unchanged;
- accepted catalog file round-trips stable IDs, aliases, lifecycle state, epoch,
  and digest;
- source rename without intent fails closed;
- accepted rename preserves stable identity without moving data cells;
- enum value storage and index keys survive member reordering through catalog
  member identity, not declaration-order ordinal;
- fresh clone and source rollback bind only through accepted catalog metadata;
- bare maybe-present read with no read-site resolution emits a diagnostic;
- positive reads using `??`, `if exists`, and optional
  chaining flow through the single ledger;
- declaration, narrowing, and attached-data proof sources are recorded in
  checked facts; read-site resolutions (`??`, `if exists`, and optional
  chaining) flow through the same ledger without adding a fourth
  proof source;
- attached-data obligations remain pending in source-only check output.
- feature-surface checks prove `@id`, source-spelling identity, regenerated IDs,
  raw path identity, and source-order enum stored meaning are rejected or absent
  from production facts.

Focused commands:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-check --test catalog_presence

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-check --test presence_architecture

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-schema --test compile_enum

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-schema --test resolve_type

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-schema --test compile_resource

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-06-perfect/Cargo.toml \
    -p marrow-run
```

## Review Lenses

Soundness review attacks branch conflicts, stale epochs, alias reuse, source
rollback, catalog file deletion, maybe-present reads, and any proof inferred
outside the ledger.

Idiom/spec review checks ADR 0206 and ADR 0210 coverage, compact Rust modules,
no dependency additions, no source syntax for stable IDs, and docs that describe
the accepted catalog file as generated project metadata. It also rejects
oversized checker/catalog dispatchers, duplicate proof classifiers, comment
sediment, and lane-local cleanup deferred to Lane 11.

## Integration Gate

Run the full central gate with the current checked runtime and add scans for
forbidden identity and proof paths:

```sh
rg -n '@id|regenerat.*id|source.*identity|source.*name|raw.*path|read.*total|maybe.*present|presence|ordinal|permission|role|principal' \
    /Users/scottwilliams/Dev/marrow-lane-06-perfect/crates \
    /Users/scottwilliams/Dev/marrow-lane-06-perfect/docs
```

`@id` matches are allowed only in rejection diagnostics/tests or
historical/debug docs. Presence and read-totality matches may appear in canonical
catalog/proof-ledger docs or tests.

## Starter Prompt

Continue Marrow v0.1 Lane 6 in `/Users/scottwilliams/Dev/marrow-lane-06-perfect`.
Use branch `lane-06-perfect`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.cargo-targets/lane-06-perfect` on
every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.

This is a narrow catalog identity and presence-ledger corrective pass, not a
general checker cleanup. Consume Lane 5's stable resource/store facts. Complete
the catalog/presence feature-surface audit for `@id`, source-spelling identity,
raw path identity, regenerated IDs, enum declaration-order stored meaning,
maybe-present/read-totality helpers, hidden catalog state, and future
principal/role/permission surfaces. Keep production behavior only when it is
ADR-backed and represented by accepted catalog metadata or the checked proof
ledger; otherwise reject/delete it or return an owning-lane blocker.

No legacy survival for green tests: migrate/delete tests, fixtures, and callers
that depend on source-owned stable identity, source-order enum stored meaning,
or ad hoc read-presence helpers. Before review, satisfy the Area Cleanup Gate:
split catalog file handling, identity binding, epoch/digest validation,
read-presence proof recording, and diagnostics; delete duplicate proof
classifiers, `@id` metadata, regenerated-ID helpers, maybe-present helpers, and
comment sediment. Leave the worktree dirty for soundness and idiom/spec review.
