# Lane 6: Catalog Identity Binding And Presence Ledger

Status: integrated foundation. Future changes in this area are regressions,
hardening work, or explicit follow-up lanes; this file is a historical contract
reference.

## Contract

Lane 6 supplies the accepted-catalog and presence-ledger foundation consumed by
runtime, evolution, storage, and tooling:

- accepted catalog metadata, not source annotations or source names, owns stable
  durable identity;
- source-only checks propose catalog changes without mutating accepted catalog
  metadata;
- accepted catalog metadata is generated project metadata committed with source;
- catalog metadata records stable IDs, aliases, lifecycle state, catalog epoch,
  and digest;
- enum stored meaning and enum index-key meaning use catalog member identity,
  not declaration-order ordinals;
- checked programs record read-presence proof sources;
- source-only checks discharge declaration and narrowing proofs and leave
  attached-data obligations pending;
- data-attached checks compare source, accepted catalog, store snapshot, data
  snapshot, and engine profile before activation.

## Rejection Ledger

Rejected as durable identity or read-presence authority:

- source `@id` annotations;
- source spelling, source paths, source names, or regenerated IDs;
- enum declaration order as stored meaning or index-key meaning;
- hidden catalog state outside committed project metadata or engine metadata;
- maybe-present or read-totality helpers that bypass the checked proof ledger;
- tool or runtime read proof inferred without a ledger entry;
- v0.1 users, roles, permissions, or principal identity inferred from catalog
  facts.

Matches for these terms are acceptable only in rejection tests, diagnostics,
debug/historical context, or docs that state the rejection.

## Ownership

Catalog identity and presence proof changes belong to the checker/catalog owner.
Runtime enum conversion, store physical keys, tooling rendering, and evolution
apply behavior consume these facts; they do not rebuild identity or presence
semantics locally.

If this surface is reopened, review must check branch conflicts, stale epochs,
alias reuse, enum reorder/remove, maybe-present reads, and any proof inferred
outside the ledger.
