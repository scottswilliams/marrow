# Lane 5 Resource/Store Surface Architecture Audit

Audit date: 2026-06-04

Scope: Lane 5 resource/store surface for Marrow v0.1. This is research and architecture review, not implementation.

Local state inspected:

- `marrow` was audited at `854cd15` (`docs(roadmap): list lanes 5 and 7 in completed foundations`), matching `main` and `origin/main`. At final verification, the primary `/Users/scottwilliams/Dev/marrow` worktree was checked out on `research/lane-09-source-native-evolution-audit` at that same commit and contained sibling untracked research docs outside this audit's write set, so this report was kept in the isolated `architecture-lane-05-resource-store-audit` worktree.
- `marrow-decisions` main had unstaged edits in `adr/foundations/01-architecture-laws-and-five-layers.md` and `adr/storage-engine/02-transactions-commits-and-recovery.md`; this report treats those edits as part of the current decision state.

## 1. Local Vision Summary

Marrow's current language docs define a resource as a reusable typed tree shape, not a durable table or collection by itself: "The same shape can be used for local values, local keyed trees, or saved data" (`docs/language/resources-and-storage.md:3-5`). A saved root is introduced separately by `store ^books(id: int): Book`, and `^books(id)` is canonically typed as `Id(^books)` (`docs/language/resources-and-storage.md:41-53`).

The identity rule is explicit: identity is owned by the store, and ordinary typed code passes the store identity value rather than the raw key (`docs/language/resources-and-storage.md:120-122`). Composite identities remain store identities (`docs/language/resources-and-storage.md:124-151`). Identity keys live in the store address, not as stored fields, and key/member name collisions are forbidden (`docs/language/resources-and-storage.md:136-140`).

Indexes are likewise store-owned. The docs say `resource ... at ^books(...)` is accepted only as declaration sugar for split `resource` plus `store`, while indexes remain on the generated store (`docs/language/resources-and-storage.md:207-209`). Index entries point back to store identities, not resource-owned IDs (`docs/language/resources-and-storage.md:211-227`). Current v0.1 index args are deliberately narrow: store keys or top-level fields only; nested fields through unkeyed groups and keyed child layers are rejected (`docs/language/resources-and-storage.md:229-232`).

The grammar matches that surface. It parses `resource_decl`, `store_decl`, and `resource_store = "at" saved_root key_params?` separately (`docs/language/grammar.md:79-88`), types store identities as `Id(^root)` (`docs/language/grammar.md:228-235`), and states that indexes inside the concise sugar desugar onto the generated store (`docs/language/grammar.md:501-503`).

Typed references are value-level identities, not foreign-key relationships. A saved field such as `authorId: Id(^authors)` accepts only `Id(^authors)`, rejects raw scalars and other stores, and round-trips as the same identity value (`docs/language/types.md:69-78`). The docs also explicitly defer referential actions: no FK constraint, cascade, join, or existence check is implied (`docs/language/types.md:92-98`).

The updated decision state reinforces the model. The local unstaged foundation ADR edit says the access path is source: store, index, and fields are hand-written rather than optimizer-chosen (`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:56-61`). The local unstaged transactions ADR edit says lowering may only elide provably redundant work and never choose semantically distinct plans by runtime statistics (`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:28-35`).

There is one important ADR conflict. The Lane 5 doc says resource-name identities such as `Book::Id` exist only as rejection fixtures (`docs/roadmap/lanes/lane-05-resource-store-surface.md:15-16`). The language docs say first-surface Marrow has no user-defined type aliases and uses `Id(^store)` for saved identities (`docs/language/types.md:51-53`). But the accepted catalog identity ADR still says a single-store resource auto-exports a `Book::Id` alias (`/Users/scottwilliams/Dev/marrow-decisions/adr/catalog-identity/01-catalog-addressed-resource-trees.md:31-35`), and the typed-reference ADR still illustrates resource identities as `Author::Id` before later repeating the `Book::Id` alias rule (`/Users/scottwilliams/Dev/marrow-decisions/adr/language/05-typed-references-and-enums.md:16-18`, `:32-34`). Current implementation and canonical language docs reject the alias, so the ADR packet is stale on this point.

## 2. Implementation Summary

The parser keeps the split real. `ResourceDecl` and `StoreDecl` are separate AST nodes (`crates/marrow-syntax/src/ast.rs:303-320`). `resource Name at ^root(...)` is parsed into a `ResourceDecl` plus an optional generated `StoreDecl` (`crates/marrow-syntax/src/parse_decl.rs:991-1023`), and the resource header parser only accepts the `Name [at ^root(...)]` form (`crates/marrow-syntax/src/parse_decl.rs:1934-1969`).

The schema layer owns the store identity surface. `StoreSchema` carries `root`, `resource`, `identity_keys`, and `indexes` (`crates/marrow-schema/src/lib.rs:261-269`), and `identity_type()` returns `Type::Identity(self.root.clone())` (`crates/marrow-schema/src/lib.rs:271-274`). Store compilation checks identity keys for concrete orderable scalar types and key/member collisions (`crates/marrow-schema/src/lib.rs:667-731`).

Index checking is store-centered and intentionally narrow. `check_store_index_args` accepts only identity keys or top-level fields, emits `schema.nested_index_arg` for nested unkeyed fields, and requires non-unique indexes to end with identity keys (`crates/marrow-schema/src/lib.rs:1076-1121`). `store_index_arg_type` rejects dotted args before looking up identity keys or `resource.field_type(&[arg])` (`crates/marrow-schema/src/lib.rs:1124-1132`). Tests assert top-level field/key acceptance and nested/keyed/map/decimal rejection through production schema APIs (`crates/marrow-schema/tests/compile_resource.rs:119-122`, `:669-735`, `:761-825`).

The checker models identity nominally by store root. `type_compatible` accepts `MarrowType::Identity(a)` only against the same root (`crates/marrow-check/src/typerules.rs:116-123`), and diagnostics render identity as `Id(^root)` (`crates/marrow-check/src/typerules.rs:246-255`). Tests verify same-shape resources in different stores have distinct identities and that `Book::Id` plus legacy constructors are unresolved (`crates/marrow-check/tests/project.rs:4195-4244`, `:4285-4310`). The focused contract test repeats that resource-named identity is rejection-only (`crates/marrow-check/tests/resource_store_contract.rs:107-130`).

The name resolver is module-aware for resources and project-wide only for saved roots. Bare resource names resolve in the referencing module only; cross-module scans enrich diagnostics but do not first-match a foreign resource (`crates/marrow-check/src/resolve.rs:79-163`). Saved roots use `resolve_store_by_root` (`crates/marrow-check/src/resolve.rs:188-208`). `marrow explain` calls the same resolver for names and `resolve_store_by_root` for saved paths (`crates/marrow/src/cmd_explain.rs:64-83`, `:249-263`).

One dead/public residue remains: `resolve_resource_by_name_any` is still a public function in `crates/marrow-check/src/resolve.rs:210-221`. Current search found no production caller, and `cmd_explain` no longer uses it, but its public existence contradicts the Lane 5 doc statement that no production helper performs project-wide resource-name search (`docs/roadmap/lanes/lane-05-resource-store-surface.md:25-27`). This is not enough to reverse the model, but it is enough to keep "no duplicate semantic owner" from being fully proven.

Runtime write planning consumes checked saved-place facts. Whole-resource writes validate store identity, write materialized plain fields, clear the record body, and stage index rewrites (`crates/marrow-run/src/write.rs:81-117`). Field writes stage index rewrites for affected indexes (`crates/marrow-run/src/write.rs:154-185`). Index maintenance derives keys from identity keys or checked resource-member sources and writes/deletes index entries atomically with data plans (`crates/marrow-run/src/index_maintenance.rs:33-57`, `:136-172`, `:197-220`).

## 3. External Precedents And Counter-Precedents

SQL tables combine shape, durable collection, primary identity, constraints, indexes, and FK targets into one table object. PostgreSQL primary keys are unique, not-null row identifiers and automatically create a unique B-tree index; FKs require values to match a referenced row and preserve referential integrity. See [PostgreSQL constraints](https://www.postgresql.org/docs/current/ddl-constraints.html). This is a counter-precedent to Marrow's split: SQL makes "table identity" natural, but a table is less reusable than Marrow's local/saved resource shape.

Document databases usually attach identity to documents in collections. MongoDB requires a unique `_id` field for standard collection documents, creates a unique `_id` index, supports manual references by storing another document's `_id`, and does not resolve DBRefs automatically. See [MongoDB documents](https://www.mongodb.com/docs/manual/core/document/), [MongoDB references](https://www.mongodb.com/docs/v8.0/reference/database-references/), and [MongoDB indexes](https://www.mongodb.com/docs/manual/indexes/). Marrow agrees with manual references being application-level values, but adds a store-nominal type boundary that MongoDB lacks.

Gel/EdgeDB is a strong counter-precedent. Object types are schema primaries analogous to tables/models, every object type has a global UUID `id`, and links are first-class relationships with cardinality and constraints. See [Gel object types](https://docs.geldata.com/reference/datamodel/objects) and [Gel links](https://docs.geldata.com/reference/datamodel/links). Gel is more relationship-rich and object-centric than Marrow. Marrow's split is stronger if resource shape reuse and compiler-visible access paths matter more than object graph navigation.

Prisma maps identity to models: one ID per model, single or composite for relational databases, with MongoDB requiring a single `_id`-mapped ID. It also maps referential actions to database FK behavior. See [Prisma models](https://www.prisma.io/docs/orm/prisma-schema/data-model/models) and [Prisma referential actions](https://docs.prisma.io/docs/v6/orm/prisma-schema/data-model/relations/referential-actions). Prisma is more familiar but would push Marrow toward model/table identity and migration-style thinking, the thing Marrow is intentionally avoiding.

Drizzle draws a useful line between soft application-level relations and database-level FKs. Its docs say relations do not affect schema or create FKs implicitly, while FKs are enforced database constraints. See [Drizzle relations](https://orm.drizzle.team/docs/relations). Marrow's typed references are closer to Drizzle's soft relation side, but safer because `Id(^store)` is nominal and checked.

Ent/entgo models entities with fields, edges, indexes, and edge schemas. It supports indexes over fields and edges, but not expression index parts in the schema API, and edge schemas can expose relationships as public API entities with composite primary keys. See [Ent indexes](https://entgo.io/docs/schema-indexes/) and [Ent edges](https://entgo.io/docs/schema-edges/). Ent supports a richer relationship graph than Marrow v0.1. Marrow should learn from edge schemas for future relationship resources, but not import them into Lane 5's identity surface.

Datomic is the closest precedent. Every datom has a database-unique entity id; idents are for programmatic names and should not be ordinary domain ids; domain identity is modeled by unique identity attributes, including composite tuple identities. See [Datomic identity and uniqueness](https://docs.datomic.com/schema/identity.html), [Datomic defining schema](https://docs.datomic.com/schema/defining-schema.html), and [Datomic schema reference](https://docs.datomic.com/schema/schema-reference.html). Marrow differs by making store root plus key the developer-visible identity type instead of a universal entity id. That is a reasonable divergence because Marrow's access path is source-visible and root-specific.

DDD aggregate roots support Marrow's owned-child-layer boundary. Microsoft's DDD introduction describes aggregate roots as the directly referenced entities and warns against references to every sub-entity; repositories conventionally dispense aggregate roots. See [Microsoft DDD aggregate roots](https://learn.microsoft.com/en-us/archive/msdn-magazine/2009/february/best-practice-an-introduction-to-domain-driven-design). Marrow's separate saved resource when a child has its own lifecycle mirrors this: keyed child layers are owned, while `Id(^store)` references target roots.

Typed API modeling supports `Id(^store)` over `Book::Id`. Rust API guidelines recommend newtypes to statically distinguish different interpretations of the same underlying type ([Rust API Guidelines](https://rust-lang.github.io/api-guidelines/type-safety.html)); Kotlin value classes distinguish wrappers from aliases while preserving efficient representations ([Kotlin inline value classes](https://kotlinlang.org/docs/inline-classes.html)); Swift API guidelines prioritize clarity at use sites and role-based names over type-name repetition ([Swift API Design Guidelines](https://www.swift.org/documentation/api-design-guidelines/)). `Id(^books)` is role-specific: it names the durable store role, not merely the resource shape.

## 4. Alternatives Considered

Keep the current split model: `resource` is shape, `store ^root(...)` is durable identity, indexes are store-owned, references are typed identity values. This keeps local and saved resource shape unified and makes access paths compiler-visible.

Use resource-owned identity, such as `Book::Id`. This is familiar to ORM users and short when a resource has one store, but it becomes misleading the moment `Book` is stored in `^books` and `^archivedBooks`. The current tests already prove those identities must remain distinct. Reviving `Book::Id` would either be a compatibility alias with surprising absence in multi-store cases or a second semantic model.

Collapse resource and store into `table`/`model`. This would align with SQL, Prisma, Ent, and Gel, and make indexes/PKs easier to explain. It would also lose Marrow's local/saved shape reuse and encourage table-style migrations, FKs, optimizer expectations, and storage-schema coupling.

Adopt a document collection model with `_id` inside each resource. This would be intuitive for MongoDB users, but it would conflate identity key fields with stored fields and weaken the current rule that keys live in the address, not in the resource body.

Adopt a universal entity id plus attribute identity like Datomic. This would make cross-store references uniform and give every resource instance one global entity handle. It would also make the visible access path less store-specific, which conflicts with Marrow's "source is the plan" rule and its per-root sequence/index contracts.

Make relationships first-class now. Gel, Prisma, and Ent all show value in first-class links/edges/FKs. For Marrow v0.1, this is too much: referential actions, cascade, restrict, existence checks, inverse indexes, and relationship evolution are a separate semantic layer. Typed references without FK semantics are acceptable as a narrow foundation.

Allow nested index args now. MongoDB and Prisma support nested-field indexes in some contexts; Ent can index fields/edges; Datomic can index any indexed attribute. Marrow currently rejects nested fields because the write planner maintains indexes by flat top-level member names. Allowing nested indexes before a production owner exists would create exactly the duplicate/hidden maintenance path Lane 5 is trying to eliminate.

## 5. Verdict

Verdict: refine, do not reverse.

The long-term model is strong: `Id(^store)` is idiomatic for Marrow because Marrow's identity boundary is a durable root plus key, not a resource shape. Store-owned indexes are the correct pairing with store-owned identity. Typed references without FK semantics are the right v0.1 minimum. `resource ... at ^root(...)` sugar is acceptable because implementation desugars it immediately and does not let resources own identity.

The main refinements are not conceptual reversals. They are cleanup and coherence fixes:

- delete or make private the dead `resolve_resource_by_name_any` surface;
- update ADR text that still advertises `Book::Id`;
- clarify the internal encoding distinction for identity reference values so docs, ADRs, and comments agree on whether value bytes include store identity or whether the store identity is supplied by field schema/catalog context;
- keep top-level-only index args for v0.1, but write a future design only after the write planner has one production owner for nested/child index maintenance.

## 6. Long-Term Risks

Duplicate semantics: the live implementation has no production caller for project-wide resource first-match resolution, but the public `resolve_resource_by_name_any` helper remains. Even dead public helpers are architecture hazards in a checker crate because tooling, LSP, or future CLI code can import them and silently re-open the old path.

Stale canonical/ADR conflict: language docs, tests, and Lane 5 docs reject `Book::Id`, while accepted ADRs still describe it as store-derived alias sugar. This is not harmless prose: identity surface is one of the highest-leverage API choices in Marrow.

Encoding ambiguity: docs/ADRs say identity values encode store identity plus key (`docs/language/types.md:76-77`, `/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/04-physical-key-and-value-encoding.md:66-67`), while checker comments warn that a stored identity carries only keys and relies on the typed field context to distinguish stores (`crates/marrow-check/src/typerules.rs:160-170`). That may be an internal leaf-value-vs-index-key distinction, but it must be named precisely before backup/restore and corruption checks harden.

Weak Rust shape: no clippy allow/expect suppressions were found in the searched Lane 5 semantic surfaces, but `crates/marrow-check/src/checks.rs` is still 3,521 lines and remains a broad semantic dispatcher. This does not invalidate Lane 5's public contract, but it raises the cost of proving there is only one semantic owner per invariant.

Roadmap sediment: Lane 5 is durable and clean, but sibling lane docs still contain temporary worktree/target paths even where status says complete or integrated (`docs/roadmap/lanes/lane-06-catalog-presence-ledger.md:11-15`, `docs/roadmap/lanes/lane-07-tree-cell-store-engine.md:11-15`, `docs/roadmap/lanes/lane-09-evolution-activation.md:11-15`). Lane 10 is also locally not reported complete despite the prompt's claim (`docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:10-14`). Do not use chat memory as status authority.

Unidiomatic permanent narrowness risk: top-level-only index args are acceptable v0.1, but if permanent they will feel weaker than document, ORM, and graph/object systems. The constraint is justified only if Marrow keeps an explicit future path for bounded owned-child or nested-group index maintenance through one planner-owned API.

Hidden compatibility glue risk: typed references without FK semantics are clean only while docs and APIs keep saying "typed value, not relationship." If tools later render `Id(^authors)` as a relation, or if backup/restore validates dangling references implicitly, Marrow will get FK semantics through a side door.

## 7. Follow-Up Recommendations

1. Delete or privatize `resolve_resource_by_name_any` in `crates/marrow-check/src/resolve.rs`, then add an absence scan or focused test proving no public production API can project-wide first-match a resource name.

2. Update the ADR packet to remove `Book::Id` alias claims from catalog identity and typed-reference ADRs. Do not add a new ADR; amend the existing accepted records so they match canonical language docs and implementation.

3. Clarify identity value encoding in one owner. Decide whether saved identity leaf payloads physically include store ID or are key-only under a field schema that names the store. Then align `docs/language/types.md`, storage ADR 04, `typerules.rs` comments, backup/restore expectations, and corruption diagnostics.

4. Keep `Id(^store)` as the canonical and only accepted identity surface. If ergonomics later demand aliases, require explicit store-owned aliases such as a future `store ^books(...) as BookId`, never resource-owned `Book::Id`, and only after type-alias support is part of the language design.

5. Keep store-owned indexes and top-level-only args for v0.1. File nested/child index design as future work only after specifying the write-planner owner, rebuild/verification path, catalog identity facts, and data-attached discharge fixtures.

6. Split `crates/marrow-check/src/checks.rs` by semantic invariant during Lane 11 or the next checker-owning lane. Prioritize type annotations/identity types, saved access/key checks, call resolution, collection traversal, and write/read admission. The goal is not aesthetics; it is making duplicate semantic owners easier to prove absent.

7. Remove roadmap execution sediment from completed sibling lane docs before using them as architecture state. Keep worktree and target-dir instructions in prompts or orchestrator notes, not durable lane contracts.

8. Preserve typed references as non-FK values. Add any future referential actions as an explicit relationship/resource layer with its own source syntax, planner effects, evolution obligations, and tests; do not grow them out of `Id(^store)` fields implicitly.
