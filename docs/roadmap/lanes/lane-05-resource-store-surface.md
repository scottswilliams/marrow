# Lane 5: Resource/Store Surface, Schema Split, And Store-Owned Indexes

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> One build agent writes failing tests and implementation, then two read-only
> reviewers attack soundness and idiom/spec before integration.

Goal: make the split resource/store model the compiler and schema foundation,
with store-owned identity, store-owned indexes, and typed `Id(^store)` values.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-05-resource-store`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-05-resource-store`

Status: ready for the first production code orchestrator.

## Parallel Safety

This is the first code lane. It may run beside Lane 7 read-only conformance
planning, Lane 10 protocol audits, and Lane 11 read-only scans. It must not run
beside Lane 6 code because both lanes want `crates/marrow-check/src/facts.rs`,
read typing, saved-place classification, and diagnostics.

Own these files during the code pass:

- `crates/marrow-syntax/src/ast.rs`
- `crates/marrow-syntax/src/parse_decl.rs`
- `crates/marrow-syntax/src/format.rs`
- `crates/marrow-syntax/tests/parse.rs`
- `crates/marrow-syntax/tests/format.rs`
- `crates/marrow-schema/src/lib.rs`
- `crates/marrow-schema/tests/compile_resource.rs`
- `crates/marrow-check/src/facts.rs`
- checker files needed only to bind store declarations into checked facts
- `crates/marrow-check/tests/project.rs`
- `docs/language/resources-and-storage.md`
- `docs/language/grammar.md`
- `docs/language/types.md`
- `docs/language/sample.md`

Do not change runtime execution, catalog file persistence, tree-cell physical
keys, backup/restore protocols, or source-native evolution in this lane.

## Area Cleanup Gate

This lane owns the complete cleanup of the resource/store surface across syntax,
schema, checker facts, diagnostics, docs, fixtures, and tests. It must delete or
rewrite resource-owned store, index, and identity paths in its area instead of
leaving a second semantic model for a later lane. It must not leave
`crates/marrow-check/src/checks.rs` as a larger catch-all pass.

Before handing the lane to review:

- split any expanded statement, loop, call, saved-path, or resource/store
  dispatcher into focused helpers or a focused module;
- migrate or delete tests, fixtures, and callers that depend on legacy
  resource-owned identity, resource-owned indexes, or old store syntax instead
  of keeping fallback branches for them;
- keep helper names carrying the explanation rather than adding branch-by-branch
  comments;
- delete dead resource-owned identity/index/store helpers and stale fixtures
  introduced or exposed by this lane;
- delete comments that narrate what the code does, explain temporary migration
  state, or compensate for an oversized function;
- preserve only comments that explain durable invariants or non-obvious
  soundness constraints;
- ensure the idiom/spec reviewer explicitly checks touched Rust for oversized
  functions, duplicate semantic classifiers, compatibility glue, and comment
  sediment.

## Production Contract

- Split `resource` and `store` declarations are the internal model.
- `store ^books(id: int): Book` owns durable store identity and keys.
- The concise `resource Book at ^books(id: int)` form is parser sugar only if it
  remains; schema and checked facts store the split representation.
- `Id(^store)` is the canonical identity type. `Book::Id` is not automatic
  resource identity; any surviving alias must be explicitly store-declared,
  absent from canonical fixtures by default, and reviewed as compatibility
  surface.
- Indexes belong to stores. A production resource schema does not own indexes.
- Collections are local trees, sequences, and keyed layers, not flat lists.
- Placement or partition source syntax remains future-reserved.
- ADR 0209 typed ephemeral root syntax is future-reserved and rejected in v0.1.
  Source forms such as `cache ~...`, `ensure ~...`, `Id(~...)`, and top-level
  `~` roots must not parse or format as production features.

## Prototype Removal Ledger

Replacement behavior: resource declarations define typed tree shape; store
declarations define durable roots, keys, identity aliases, and indexes.

Delete or isolate:

- `ResourceDecl.store` as the production semantic owner.
- `ResourceMember::Index` as production ownership.
- `ResourceSchema.saved_root` and `ResourceSchema.indexes` as durable owners.
- resource-name-owned identity inference from `Book::Id`.
- checker/runtime index lookup through `resource.indexes`.
- accidental production support for `~` typed ephemeral roots.

Production bridge: none. There are no production callers that need a
runtime-facing merged resource/store view in v0.1; delete merged-view helpers
instead of naming them as a Lane 8 handoff. If another lane finds a real caller,
that caller is a blocking bug to fix there, not a reason to preserve a second
semantic model here.

## TDD Start

Write these failing checks first, through the production parser/schema/checker
pipeline where possible:

- parse and format `store ^books(id: int): Book`;
- compile split resource plus store into separate resource and store schemas;
- compile concise `resource Book at ^books(id: int)` into the same split model;
- prove two stores over one resource have distinct identities;
- reject production indexes on resources except through concise desugaring;
- accept `author: Id(^authors)` as a typed reference without joins, cascade
  delete, or automatic existence checks;
- migrate the shared v0.1 fixture from `Author::Id`/`Book::Id` to
  `Id(^authors)`/`Id(^books)`.
- reject ADR 0209 `~` root forms, including `cache ~...`, `ensure ~...`,
  `Id(~...)`, and top-level `~` roots, without formatter output presenting them
  as v0.1 surface.

Focused commands:

```sh
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-05-resource-store \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-05-resource-store/Cargo.toml \
    -p marrow-syntax

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-05-resource-store \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-05-resource-store/Cargo.toml \
    -p marrow-schema

CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-05-resource-store \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-05-resource-store/Cargo.toml \
    -p marrow-check
```

## Review Lenses

Soundness review attacks cross-module store/resource names, same resource with
multiple stores, alias reuse, typed reference confusion, index uniqueness, absent
index components, and source rename assumptions.

Idiom/spec review checks the grammar matches docs, the implementation is small,
no new dependencies appear, no duplicate store classifiers are introduced, and
Rust modules do not grow crate-root glob preludes, oversized dispatcher
functions, comment sediment, compatibility slop, or lane-local cleanup deferred
to Lane 11.

## Integration Gate

Before integration, run the full gate from the central roadmap with the target
dir above. Add architecture scans for the deleted prototype paths:

```sh
rg -n 'ResourceSchema.*indexes|ResourceSchema.*saved_root|ResourceMember::Index|Book::Id|Author::Id' \
    /Users/scottwilliams/Dev/marrow-lane-05-resource-store/crates \
    /Users/scottwilliams/Dev/marrow-lane-05-resource-store/docs/language

rg -n 'cache\s*~|ensure\s*~|Id\s*\(\s*~|store\s*~|at\s*~' \
    /Users/scottwilliams/Dev/marrow-lane-05-resource-store/crates \
    /Users/scottwilliams/Dev/marrow-lane-05-resource-store/docs
```

Every match must be a rejection test, canonical compatibility docs for an
explicitly store-declared alias, or debug/admin docs. Lane 5 allows no production
bridge; merged-view/prototype helpers with no production callers must be absent.

## Starter Prompt

Continue Marrow v0.1 Lane 5 in `/Users/scottwilliams/Dev/marrow-lane-05-resource-store`.
Create or reuse the branch `lane-05-resource-store`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-05-resource-store`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
Implement the resource/store split with TDD: split `store` declarations,
store-owned indexes, canonical `Id(^store)`, concise-form desugaring if kept,
ADR 0209 `~` source-form reservation/rejection, and migration of v0.1 fixtures
away from resource-owned identity. Do not touch catalog persistence, runtime
execution, tree-cell keys, backup, or evolution. Before review, satisfy the Area
Cleanup Gate: remove resource-owned store/index/identity helpers, delete any
runtime-facing merged resource/store view, split resource/store checker
dispatchers, and make `ResourceSchema.saved_root`, `ResourceSchema.indexes`,
`ResourceMember::Index`, `Book::Id`, and `Author::Id` absent outside allowed
rejection or compatibility docs. Leave the worktree dirty for two read-only
reviews: soundness and idiom/spec.
