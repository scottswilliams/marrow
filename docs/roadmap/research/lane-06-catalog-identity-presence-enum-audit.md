# Lane 6 Catalog Identity, Presence Ledger, And Enum Member Identity Audit

Research audit only. This is not an ADR and proposes no production-code changes.
References below are repository-relative unless they point into
`marrow-decisions`.

## 1. Local Vision Summary With File And Line References

The audit compared the Marrow repository with the `marrow-decisions` ADR packet,
including then-current edits in the foundations and storage-engine ADRs.

The local vision is coherent and ambitious: Marrow treats saved state as part
of one compiled program, where source declares shape and intent, the catalog
owns durable identity, the compiler checks source/catalog/data/engine together,
runtime executes checked IR, and the engine persists ordered bytes without
knowing Marrow semantics. The unstaged foundations ADR edit makes this sharper:
source owns access paths, and no lower layer may choose a cost-based plan below
the language contract
(`marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:20`,
`:34`, `:56`, `:68`). The unstaged storage ADR edit mirrors that by allowing
only provably redundant-operation elision in write lowering, not runtime
statistics choosing another operation shape
(`marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:23`,
`:28`, `:55`, `:83`).

Lane 6 is explicitly scoped to bind durable identity through a committed
accepted catalog file and one checked-program presence proof ledger, not through
source names, regenerated IDs, source-order enum ordinals, or scattered
maybe-read helpers
(`docs/roadmap/lanes/lane-06-catalog-presence-ledger.md:7`,
`:42`, `:69`, `:109`, `:129`). The execution plan currently marks Lanes 5-9
as completed foundations and names Lane 6 as the catalog-backed enum identity
and presence-ledger foundation; it does not, in this checkout, prove Lane 10 is
fully done, so I did not rely on the prompt's completion report
(`docs/roadmap/prototype-to-v1-execution-plan.md:173`,
`:195`, `:355`, `:451`, `:522`).

The accepted catalog ADR says the catalog is the durable ABI and single
authority for stable IDs, aliases, lifecycle, epoch, and digest; source checks
may propose but not mutate it; and accepted/evolve boundaries advance durable
state explicitly
(`marrow-decisions/adr/catalog-identity/02-catalog-lifecycle-and-identity-binding.md:18`,
`:23`, `:28`, `:33`, `:52`, `:68`). It also states that stable IDs are never
regenerated for branch merges and public aliases should move through
`active`, `deprecated`, or `reserved` states
(`marrow-decisions/adr/catalog-identity/02-catalog-lifecycle-and-identity-binding.md:61`,
`:63`). That alias lifecycle is the v0.1 implementation contract.

Resource and storage docs reject source-spelling identity: stores own identity,
the canonical identity type is `Id(^store)`, and every durable entity receives
an invisible random opaque stable ID recorded in the catalog, not in source
annotations
(`docs/language/resources-and-storage.md:52`,
`:179`, `:207`, `:229`). The data-evolution doc states the same operationally:
renames require `evolve rename`, the catalog records old aliases and the same
stable ID, source-only checks propose but do not write the catalog, and stable
IDs are random `cat_<32 lowercase hex>` values precisely to tolerate branch
parallel allocation
(`docs/data-evolution.md:131`,
`:149`, `:163`, `:178`).

Enum docs now reject source-order meaning: an enum value stores the selected
member's stable catalog identity, reordering source members does not change
stored meaning, removal or unselection is checked against saved data, and member
renames preserve identity only through `evolve rename`
(`docs/language/enums.md:39`,
`:46`, `:53`, `:85`). Grammar docs cover the corresponding language surface:
`evolve` targets catalog-addressable entities through ordinary source forms,
`Id(^root)` is the identity type, `??` is admitted only for maybe-present reads
checked by the checker, and ADR 0209 reserves and rejects `~` roots and
`Id(~scratch)` in v0.1
(`docs/language/grammar.md:193`,
`:228`, `:379`, `:516`).

The presence ADR is the conceptual center of the maybe-present story: every read
must have exactly one proof source, activation succeeds only when every
obligation is discharged, and runtime, CLI, LSP, evolution, backup, and restore
should read one ledger rather than rediscovering presence
(`marrow-decisions/adr/model-ir/02-presence-proof-and-activation.md:14`,
`:20`, `:33`, `:49`, `:59`). Lane 13 gives presence proofs explicit proof
identity and discharged/pending-attached-data status, which keeps consumers from
rediscovering maybe-present semantics.

One local vision conflict must be fixed: the proposed physical encoding ADR says
stable IDs are a single per-catalog monotonic space
(`marrow-decisions/adr/storage-engine/04-physical-key-and-value-encoding.md:21`),
while the accepted catalog/data-evolution docs and implementation use random,
content-independent IDs to avoid branch-merge coordination
(`docs/data-evolution.md:178`,
`crates/marrow-check/src/catalog.rs:1000`).
The catalog epoch can be monotonic; entity IDs should not be monotonic without a
central allocator.

## 2. Implementation Summary With Crate And Module References

The accepted catalog is represented in `marrow-project`. `CatalogMetadata`
stores `epoch`, `digest`, and entries, validates non-empty paths and stable IDs,
unique stable IDs, and unique canonical/alias paths, and recomputes the digest
on read
(`crates/marrow-project/src/lib.rs:69`,
`:89`, `:107`). Entries cover resources, stores, store indexes, resource
members, enums, and enum members
(`crates/marrow-project/src/lib.rs:145`,
`:196`). The lifecycle enum has `Active`, `Deprecated`, and `Reserved`
(`crates/marrow-project/src/lib.rs:207`),
matching the accepted v0.1 reserved-spelling model. The digest is the canonical
`sha256:<64 lowercase hex>` digest over JSON payload
(`crates/marrow-project/src/lib.rs:583`,
`:590`).

Catalog binding lives in `marrow-check/src/catalog.rs`. It reads the accepted
catalog, computes current source catalog entries, binds accepted IDs onto facts,
records store key shapes and resource-member structural signatures, and leaves a
proposal for accept/evolve flows rather than writing from source-only check
(`crates/marrow-check/src/catalog.rs:39`,
`:169`, `:236`, `:280`). Accepted active paths bind by `(kind, path)`;
renames are explicit, injective, and fail closed when source/target/source-still
declared conditions are unsound; a rename preserves the entry stable ID and
records the old path as an alias
(`crates/marrow-check/src/catalog.rs:414`,
`:451`, `:499`). Non-active entries and aliases do not bind live source facts
(`crates/marrow-check/src/catalog.rs:706`).

New catalog entries are proposal-only until accepted. The allocator emits
random opaque `cat_<32 lowercase hex>` IDs from operating-system entropy,
rerolling against all existing IDs from every lifecycle state. The code comment
explicitly rejects a monotonic counter for branch-parallel work
(`crates/marrow-check/src/catalog.rs:982`,
`:1000`, `:1022`, `:1033`). Tests assert proposal IDs do not collide with
accepted IDs and do not derive identity from kind/path text
(`crates/marrow-check/tests/catalog_presence.rs:303`,
`:2272`).

Checked facts carry catalog IDs for resources, stores, members, enums, and enum
members, plus `presence_proofs`
(`crates/marrow-check/src/facts.rs:47`,
`:213`, `:1044`, `:1053`, `:1161`). Enum member source-order helpers still
exist in `CheckedFacts`
(`crates/marrow-check/src/facts.rs:164`,
`:171`), but runtime storage and index-key meaning no longer use source-order
ordinals. Their remaining role appears to be source traversal and checked
expression lowering; they should be made harder to misuse.

Runtime enum storage is catalog-member based. `EnumValue` carries enum and
member catalog IDs; writing an enum stores a `TreeEnumMember` with the enum
catalog ID and member catalog ID, and the index key is the member catalog ID
(`crates/marrow-run/src/value.rs:53`,
`:226`, `:273`). Decoding checks the stored enum catalog ID matches the current
enum fact and then finds the member by catalog ID
(`crates/marrow-run/src/value.rs:298`,
`:312`). The store encodes enum/member catalog IDs in value bytes
(`crates/marrow-store/src/tree.rs:118`,
`:1424`). Index maintenance reuses the same key derivation path through
`LeafValue::as_key` and `StoredValueMeaning::stored_key`, so stored and rebuilt
index entries share the same member-ID meaning
(`crates/marrow-check/src/facts.rs:1104`).

Presence is centralized under `marrow-check/src/presence`. `target.rs` owns
saved-path and unique-index target classification, `proofs.rs` owns proof-source
classification and ledger recording, and `walk.rs` delegates to those owners
(`crates/marrow-check/src/presence/target.rs:28`,
`:50`, `:105`, `:130`;
`crates/marrow-check/src/presence/proofs.rs:17`,
`:43`;
`crates/marrow-check/src/presence/walk.rs:21`,
`:67`). Architecture tests guard against moving duplicate classifiers back into
facts, effects, or walkers
(`crates/marrow-check/tests/presence_architecture.rs:1`,
`:36`). The remaining weakness is shape, not obvious duplicate semantics: broad
walkers still have to be kept in sync as syntax grows.

`@id` is rejected as source identity, not kept as a legacy bridge. The syntax
test rejects a resource member annotation written as `@id("book.title")`
(`crates/marrow-syntax/tests/parse.rs:922`).
Other prototype-only constructs are checked through `marrow-check/src/prototype.rs`
and reported as `check.prototype_only`
(`crates/marrow-check/src/prototype.rs:12`,
`:74`, `:140`, `:180`, `:287`).

## 3. External Precedents And Counter-Precedents

- [Protocol Buffers proto3 language guide](https://protobuf.dev/programming-guides/proto3/)
  and [Proto Best Practices](https://protobuf.dev/best-practices/dos-donts/)
  strongly support the stable-ID premise: field numbers are the durable wire
  meaning, should not be changed after use, and deleted numbers/names should be
  reserved. Counter-precedent: Protobuf makes IDs source-visible field numbers,
  which is operationally clear but noisy and easy to bikeshed. Marrow's invisible
  accepted catalog is a reasonable UX improvement if merge tooling is strong.

- [Cap'n Proto schema language](https://capnproto.org/language.html) is even
  closer: files and types have unique 64-bit IDs, fields/enumerants have ordinal
  numbers, symbolic names may change when IDs/numbers stay fixed, and source
  order can change while numbers remain durable. Counter-precedent: Cap'n Proto
  exposes IDs and ordinals in source (`@0`, `@0x...`). Marrow rejects that source
  clutter, so the generated catalog must compensate with excellent review and
  conflict diagnostics.

- [Apache Avro aliases](https://avro.apache.org/docs/1.12.0/specification/#aliases)
  support explicit rename compatibility through alias-driven reader/writer schema
  resolution. Its [Parsing Canonical Form and fingerprints](https://avro.apache.org/docs/1.12.0/specification/#parsing-canonical-form-for-schemas)
  also support canonicalized digests instead of ad hoc text hashes. Counter:
  aliases are optional reader behavior in Avro, whereas Marrow's alias lifecycle
  is part of durable identity and should be stricter.

- [Confluent Schema Registry schema evolution](https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html)
  shows the central-registry alternative: schemas get IDs and versions, and new
  versions are compatibility-checked before acceptance. Counter: a registry
  service gives clean global coordination but is heavier than v0.1 local/offline
  Marrow. Marrow's committed catalog is a local, source-control-native registry;
  it must still learn from registry UX around compatibility errors and version
  acceptance.

- PostgreSQL catalogs are a strong precedent for internal identity. PostgreSQL
  [OIDs](https://www.postgresql.org/docs/current/datatype-oid.html) are internal
  primary keys for system catalogs and `regclass` constants early-bind across
  rename. PostgreSQL [pg_attribute](https://www.postgresql.org/docs/current/catalog-pg-attribute.html)
  keeps column numbers and dropped-column state, and
  [pg_enum](https://www.postgresql.org/docs/current/catalog-pg-enum.html) stores
  enum values as catalog rows with OIDs plus a distinct sort order. Counter:
  PostgreSQL's database instance is the catalog authority; source branches are
  not trying to mint independent durable IDs and merge them in Git.

- [Datomic schema data](https://docs.datomic.com/schema/schema-reference.html)
  is a precedent for schema-as-data: attributes and specs are data entities with
  idents and transactions, not only source text. Counter: Datomic is
  database-authoritative; Marrow is trying to keep source-only checks, fresh
  clones, and code review meaningful, so committing generated catalog data is a
  better fit than a hidden database-only schema.

- [TypeScript narrowing](https://www.typescriptlang.org/docs/handbook/2/narrowing.html)
  and Rust's [exhaustive match](https://doc.rust-lang.org/book/ch06-02-match.html)
  support Marrow's presence-ledger direction: compilers can prove facts from
  flow and force explicit handling of absent cases. Counter: TypeScript and Rust
  mostly reason over program values at compile time, not persistent data
  snapshots; Marrow's ledger needs data-attached pending/discharged status, not
  just local control-flow narrowing.

- Migration-history tools are the strongest counter-precedent. [Liquibase](https://www.liquibase.org/get-started/core-usage/liquibase-core-concepts-author-database-changes)
  uses source-controlled changelogs plus a `DATABASECHANGELOG` table; [Flyway](https://documentation.red-gate.com/flyway/flyway-concepts/migrations/flyway-schema-history-table)
  tracks applied migrations, checksums, failed/future/out-of-order states, and
  repair flows. Marrow intentionally rejects migration-file ledgers, but it still
  needs equally mature status reporting for accepted catalog epoch, digest,
  branch conflicts, source rollback, and data-attached obligations.

## 4. Alternatives Considered

1. Source-visible stable annotations or field numbers, such as `@id`.
   This has strong precedents in Protobuf and Cap'n Proto, but it makes durable
   identity part of everyday syntax and invites manual churn. Marrow's rejection
   is sound for v0.1 as long as the generated catalog remains reviewable and
   source-native `evolve` intent stays explicit.

2. Source-name identity or path-derived IDs.
   This is the weakest option. It collapses spelling and durable meaning, makes
   renames destructive or heuristic, and fails the roadmap's explicit delete list.
   The current implementation correctly rejects the old path-hash derivation and
   fail-closes bare source rename.

3. Regenerating IDs from source order, source path, or content.
   This should remain reversed. It would make branch merges and reorders
   semantically dangerous. The current enum runtime and index-key path correctly
   store catalog member IDs, not enum declaration order.

4. A central schema registry or database-owned catalog.
   This is attractive for long-lived teams and distributed deployments because it
   gives one allocator and one acceptance point. It is too heavy for v0.1's
   source-local design and breaks source-only checks unless mirrored back into
   source control. It is a possible future deployment mode, not the v0.1
   foundation.

5. Committed generated catalog with random per-entity IDs, epoch, digest,
   lifecycle, aliases, and source-native evolve intent.
   This is the current design and is the best v0.1 foundation. It combines
   reviewability, fresh-clone behavior, offline work, explicit identity moves, and
   storage/runtime indirection. Its long-term success depends on making generated
   file conflicts intelligible rather than treating JSON merge pain as user error.

6. Monotonic stable IDs.
   Monotonic IDs are reasonable only with one allocator, such as a central
   registry or database transaction. They are a poor fit for Git branches that
   mint identities independently. Keep monotonicity for catalog epochs and commit
   IDs, not entity stable IDs.

7. Collision-resistant random IDs plus deterministic validation and merge tools.
   The v0.1 contract uses a 128-bit random shape and fail-closed validation.
   Branch-merge tooling remains useful for human conflict resolution, but stable
   identity is no longer waiting on a larger ID space.

## 5. Verdict: Refine

Keep the committed generated catalog as Marrow v0.1's durable ABI mechanism.
Reverse neither to source-name identity nor to source-visible `@id` annotations.
The current architecture is better than the usual migration-script-only model
for Marrow because durable state is compiled with source, catalog, data, and
engine profile together, and because enum member storage/index meaning is now
catalog identity rather than source order.

The Lane 13 hardening pass resolved the monotonic-vs-random ID conflict,
strengthened digest semantics, and implemented the `reserved` alias lifecycle.
Branch merge UX is still a foundation risk, not polish.

## 6. Long-Term Risks

- Branch merge behavior is the largest product risk. Random IDs avoid automatic
  numeric collisions from parallel branches, but users will still see generated
  JSON conflicts around epoch, digest, entry order, aliases, and structural
  signatures. Without a catalog-aware merge/check tool, the right foundation may
  feel worse than source-visible field numbers.

- The ID strategy is inconsistent in docs. Accepted/local docs and code use
  random content-independent IDs, while the proposed physical-key ADR still says
  single per-catalog monotonic space. A future implementer could "clean up" the
  conflict in the wrong direction.

- Catalog, source, and evolution digests use the SHA-256 contract. Keep any
  remaining non-cryptographic checksum language scoped to accidental-corruption
  detection or narrow engine-profile stamps, not durable compatibility fences.

- Alias lifecycle is no longer a missing enum state. Future work should focus on
  review/merge UX for reserved public spellings.

- Checked facts now use typed optional catalog IDs instead of empty-string
  sentinels for proposal-only entries. Keep that shape from regressing into
  stringly identity.

- Source-order enum helpers remain exposed enough to be tempting. They appear to
  serve traversal/lowering only, while runtime storage and index keys use catalog
  member identity. Long term, a `SourceEnumOrdinal` newtype or a more private
  lowering boundary would keep source-order from drifting back into durable
  meaning.

- The presence ledger records proof identity, proof source, place, keys, read
  kind, discharged/pending-attached-data status, and span. Future consumers
  should keep reading this ledger rather than rediscovering maybe-present
  semantics.

- Presence AST walkers are cleaner than before, but still broad. The architecture
  tests prevent a few duplicate helper names from returning; they do not prove
  future syntax will update direct effects, write invalidation, target
  classification, and proof recording together.

- Generated catalog UX is not yet visible enough. The language docs say there is
  no user-facing catalog command, but ADR 0206 says explicit accept or evolve
  advances the epoch. In practice, run/evolve may write transparently. That can be
  acceptable, but users need inspection, review, and conflict-resolution affordances
  even if the catalog remains infrastructure rather than source syntax.

## 7. Concrete Follow-Up Recommendations, Ordered By Foundation Risk

1. Resolve the ID law in docs and ADRs: entity stable IDs are random or otherwise
   collision-resistant and content-independent; catalog epochs and commit IDs are
   monotonic. Update the proposed physical-key ADR or explicitly mark monotonic
   entity IDs as rejected for Git-branch v0.1.

2. Add catalog branch-merge fixtures and UX. Simulate two branches adding
   different resources, members, enum values, indexes, and aliases; require the
   merged catalog to preserve both IDs, recompute epoch/digest deterministically,
   and diagnose real alias/stable-ID conflicts without regeneration.

3. Make enum source-order helpers private to lowering/traversal or rename them to
   expose their non-durable nature. Add an architecture test that runtime,
   storage, index maintenance, evolution discharge, and saved-key decoding cannot
   call ordinal helpers for stored meaning.

4. Add consumer-side tests proving runtime/tooling/evolution reads the presence
   ledger rather than rediscovering maybe-present semantics.

5. Keep `@id` dead with feature-surface tests across parser, checker, fixtures,
   docs, and CLI output. It is good that syntax rejects it now; future generated
   catalog tooling should not reintroduce an annotation escape hatch.

6. Give generated catalog review a first-class command or report even if the
   source language has no catalog syntax. A command such as `marrow catalog diff`
   or a structured check diagnostic should explain new IDs, renames, aliases,
   retired/reserved spellings, epoch/digest changes, and branch conflicts.

7. Keep presence walkers under code-shape review. When new language constructs
    land, require a sibling scan over `presence::direct`, `presence::walk`,
    `presence::effects`, `presence::target`, and `presence::proofs` so one
    construct cannot gain duplicate semantics in two walkers.
