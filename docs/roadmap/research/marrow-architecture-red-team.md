# Marrow Architecture Red-Team

Scope: architecture research audit, not implementation. I inspected the current git state for both Marrow repositories before trusting any docs: `/Users/scottwilliams/Dev/marrow` is on `research/lane-09-source-native-evolution-audit` at `f0623c9287f3f2483c6ee5996f13f4f16b324a84` with untracked `docs/roadmap/research/`; this report was written in the fresh worktree `/Users/scottwilliams/Dev/marrow-architecture-red-team-audit` on `research/architecture-red-team-audit` at `3f7178a3d6712f2e715259574fc446a552f34302`. `/Users/scottwilliams/Dev/marrow-decisions` is on `main` at `7ce51f17f8037201747baa3cffdf0fdbc214aaae` with unstaged edits in `adr/foundations/01-architecture-laws-and-five-layers.md` and `adr/storage-engine/02-transactions-commits-and-recovery.md`; those edits are included below.

Skeptical thesis: Marrow is trying to own a language, compiler, catalog, runtime, store, evolution system, backup format, CLI, LSP, and data protocol at once. That is exactly the kind of ambition that can turn a clean language into a private database engine with duplicate semantics and accidental compatibility promises. The burden of proof is on Marrow to show that these pieces are one idiomatic tree/database language, not a pile of machinery that SQLite, Postgres, MongoDB, CouchDB, or a migration framework already solved better.

## 1. Local Vision Summary

The strongest local vision is not SQL-shaped. Marrow says a resource is a typed tree, a saved resource is a typed tree under `^root`, an index is a generated lookup tree, the store persists typed tree cells over ordered bytes, and Marrow owns the meaning of durable data (`docs/implementation.md:16-22`). The backend is intentionally demoted: it must not parse `.mw`, understand fields, maintain indexes, plan evolution, or expose backend-specific application APIs (`docs/implementation.md:24-31`). The kernel is source, schema, runtime, store, and tools, with field names, types, indexes, history, evolution, and repair all above the private ordered-byte substrate (`docs/implementation.md:35-47`).

The language docs make the same point in source-native terms. Saved data is logical, while Marrow decides physical storage for roots, keyed layers, fields, and indexes (`docs/language/resources-and-storage.md:13-15`). `^books(id)` is a saved `Book`, with identity canonically modeled as `Id(^books)` (`docs/language/resources-and-storage.md:52-56`). Durable identity is owned by an invisible catalog, not by source spelling; stable IDs are random, opaque, and advanced by durable flows while `check` stays read-only (`docs/language/resources-and-storage.md:179-187`, `docs/data-evolution.md:149-184`). Managed writes update indexed data coherently and reject raw untyped writes into managed roots (`docs/language/resources-and-storage.md:324-350`, `609-617`). Backup is supposed to be a typed manifest plus canonical tree-cell data stream, not a raw engine copy (`docs/language/resources-and-storage.md:541-555`, `docs/data-evolution.md:298-309`).

The most non-SQL part is the cost model. Marrow explicitly says it has no query optimizer: the source names the store, index, and fields; the access path is the source (`docs/language/cost-model.md:3-5`). Cost is counted in point reads, scans, writes, index touches, and commits as properties of the checked program, not runtime statistics (`docs/language/cost-model.md:11-16`). Hidden traversal is a compile error, while explicit traversal is valid (`docs/language/cost-model.md:43-49`). The planner may elide redundant work but does not choose between semantically distinct plans from statistics (`docs/language/cost-model.md:51-62`).

The unstaged foundation ADR edits sharpen this into laws: one program is source files, one catalog, attached data, and a target engine compiled together; source declares, catalog preserves identity, compiler checks source/catalog/data/engine together, runtime executes checked IR, and engine stores ordered bytes (`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:20-30`). The new law 15 says access path is source and there is no optimizer below the language (`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:56-61`). The unstaged transaction ADR likewise says lowering is a property of checked source, not a cost-based optimizer, and commit metadata should address every committed write (`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:28-35`, `55-62`).

Important doc drift changes the audit posture. The roadmap README claims the parser/checker/runtime, resources, redb storage, managed writes, transactions, CLI, inspection tools, LSP, and data server exist today (`docs/roadmap/README.md:3-9`), while the execution plan still queues Lane 10 as next and lists Lane 10/Lane 11 as remaining work (`docs/roadmap/prototype-to-v1-execution-plan.md:166-171`). The Lane 10 doc itself says early status is read-only audit and first code phase begins with backup APIs (`docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:14-18`), but the actual code contains backup/restore and `debug_data_*` serve operations. Lane 11 still cites `crates/marrow-store/src/archive.rs` as evidence (`docs/roadmap/lanes/lane-11-rust-hardening.md:151-166`), but that file does not exist in the current worktree. Also, accepted ADR text still advertises `Book::Id` alias sugar (`/Users/scottwilliams/Dev/marrow-decisions/adr/catalog-identity/01-catalog-addressed-resource-trees.md:31-35`), while the lane and language docs say `Id(^store)` is the canonical surface and resource-name identities are rejection fixtures (`docs/roadmap/lanes/lane-05-resource-store-surface.md:15-16`). Treat roadmap docs as hypotheses, not truth.

## 2. Implementation Summary

The actual Rust proves more of the architecture than a pure red-team prompt would expect. The workspace is split into seven crates and forbids `unsafe` at the workspace lint level (`Cargo.toml:1-10`, `21-22`). `marrow-store` has only optional `redb` as the native dependency (`crates/marrow-store/Cargo.toml:11-17`). That is evidence against the claim that Marrow is currently building a full page-store database engine: it is layering a typed tree-cell contract over an embedded ordered key-value engine.

The store/backend split is real. The private `Backend` trait is ordered bytes: read, write, prefix delete, bounded scans, cursor-resumed scans, begin/commit/rollback, and snapshots (`crates/marrow-store/src/backend.rs:66-88`). `TreeStore` is the public typed facade over that backend (`crates/marrow-store/src/tree.rs:146-190`). It exposes typed node/leaf/sequence/data/index/meta operations (`crates/marrow-store/src/tree.rs:220-250`), and backup traversal streams the data family only (`crates/marrow-store/src/tree.rs:641-645`). Backend conformance covers ordering, scans, nested transactions, read-your-writes, and snapshots (`crates/marrow-store/src/conformance.rs:5-20`). Redb is the native backend, with one active write transaction, undo journals, and pinned read views (`crates/marrow-store/src/redb.rs:42-52`). The redb dependency itself documents multiple concurrent reads, reads concurrent with writes, and a single writer at a time (https://docs.rs/redb/latest/redb/struct.Database.html).

The checker pipeline is also real, but heavy. Source analysis discovers modules, applies source overlays, parses, checks, rebuilds facts, binds catalog, and lowers runtime/evolution bodies (`crates/marrow-check/src/analysis.rs:120-145`, `318-336`). Catalog binding separates accepted IDs from proposal-only IDs used for defaults and transforms (`crates/marrow-check/src/catalog.rs:28-64`). Evolution preview discharges obligations read-only and produces a witness, then runtime apply re-runs preview and requires witness equality before staging writes (`crates/marrow-check/src/evolution/discharge.rs:68-82`, `crates/marrow-run/src/evolution/apply.rs:84-90`, `crates/marrow-run/src/evolution/validate.rs:9-22`). This is a credible source-native foundation, not a doc-only fantasy.

The runtime/write path is much less fake than a prototype. `WritePlan` has explicit `PlanStep`s for data, index, subtree, and metadata work (`crates/marrow-run/src/write_plan.rs:9-42`). A standalone plan opens a store transaction; a plan inside a source transaction applies steps into the already-open transaction (`crates/marrow-run/src/write_plan.rs:79-88`). Backup creation pins a snapshot and streams twice for count/checksum and then bytes (`crates/marrow/src/backup/create.rs:20-60`). Restore validates engine profile, source digest, and catalog epoch; replays into an empty store inside one transaction; rebuilds generated indexes; stamps metadata; and verifies before commit (`crates/marrow/src/backup/restore.rs:21-50`, `55-88`, `91-142`). That is a real typed restore path.

The red-team findings are also concrete:

- Proposal-only evolution identity appears mismatched between checker and runtime. Discharge explicitly handles brand-new proposal IDs (`crates/marrow-check/src/evolution/discharge.rs:518-522`, `1070-1074`), and tests call a brand-new required member with `evolve default` activatable (`crates/marrow-check/tests/evolution_discharge.rs:3915-3919`). But checked executable places fill member catalog IDs from accepted facts and default to empty when no accepted member ID exists (`crates/marrow-check/src/executable/place.rs:203-211`). Runtime default and transform apply locate targets by the `CheckedSavedPlace` member catalog ID (`crates/marrow-run/src/evolution/backfill.rs:36-50`, `crates/marrow-run/src/evolution/transform.rs:64-76`). Existing runtime apply tests avoid the hard case by accepting the expanded schema first (`crates/marrow-run/tests/evolution_apply.rs:218-223`). This is a v0.1 blocker if source-native add-required/default apply is in scope.
- Commit metadata at the physical durable boundary was a confirmed risk before
  Lane 15. The current runtime aggregates managed writes and generated index
  writes into the outer durable transaction, and tests assert that commit
  metadata covers that whole transaction.
- Snapshot plus write state was a confirmed risk before Lane 15. The current
  store contract prohibits overlapping read snapshots and write transactions on
  one handle, rejects same-handle writes while a snapshot is pinned, and covers
  the rule in memory/redb conformance.
- The path/query surface is duplicated. `data get` parses source path text, then collapses field, child-layer, and index segments into `SourceMember` (`crates/marrow/src/cmd_data/get.rs:16-30`, `80-96`). Serve has a distinct JSON segment codec with explicit field/layer/key cases (`crates/marrow/src/serve/protocol/codec.rs:25-38`, `80-86`). `data dump` emits human path plus base64 bytes (`crates/marrow/src/cmd_data.rs:221-244`, `315-321`). This is already a semantic split waiting to become compatibility glue.
- Debug/admin protocols are intentionally demoted, but still tempting. Serve only dispatches `debug_data_*`, and tests reject non-debug `data_*` operations (`crates/marrow/src/serve/protocol.rs:77-100`, `crates/marrow/src/serve/protocol/tests.rs:93-100`). That is good. It also means Marrow does not yet have a production data API.
- Integrity is schema-aware, and restore rejects orphan backup cells before
  commit. `data integrity` still reports orphan cells already present in a local
  store, and orphan classification is catalog-ID membership rather than a full
  nesting proof (`crates/marrow/src/cmd_data/orphan.rs:81-88`). That remaining
  classification depth is a tooling follow-up, not a restore import policy.
- There is weak Rust shape in high-risk areas. `checks.rs` is 3521 lines, `evolution/discharge.rs` is 2572 lines, `tree.rs` is 2175 lines, `lib.rs` in `marrow-check` is 1556 lines, and `binding.rs` is 1542 lines. `evolution/discharge.rs` holds top-level discharge, proposal scans, structural backstops, index repair, text ownership parsing, and accumulator state (`crates/marrow-check/src/evolution/discharge.rs:68-82`, `2048-2052`, `2186-2194`). Prototype syntax is fenced by checks, not removed from the grammar and walkers (`crates/marrow-check/src/prototype.rs:12-34`; see reserved `merge`/`lock` in `docs/language/syntax.md:371-386`).

## 3. External Precedents And Counter-Precedents

These comparisons should not force Marrow to become SQL. The best precedents are systems that own document/tree semantics over lower-level storage, or that make access patterns explicit. SQL systems are useful counter-precedents for what Marrow is deliberately not doing.

- CouchDB is a strong document-system precedent for Marrow's tree-first instincts. Its docs make documents the primary data unit, with database-maintained metadata, optimistic document updates, MVCC snapshots, and incrementally maintained view indexes (https://docs.couchdb.org/en/stable/intro/overview.html). The important lesson is not "copy CouchDB"; it is that an elegant data language can make document shape and view/index maintenance central rather than treating everything as rows.
- MongoDB is a useful NoSQL counterweight to SQL normalization. Its modeling docs say related data can be embedded or referenced depending on access shape, and warn that production schema changes can be hard even with flexible schema (https://www.mongodb.com/docs/manual/data-modeling/best-practices/#link-related-data). Its schema validation docs frame validation as optional and flexible once an application schema is established (https://www.mongodb.com/docs/manual/core/schema-validation/). Marrow's stricter typed trees are differentiated if they keep the document/tree ergonomics while avoiding MongoDB's "schema lives partly in app convention" problem.
- Datomic is a strong precedent for separating identity from user spelling. It assigns stable database entity IDs, uses idents for programmatic schema/enumeration names, and models unique domain identities separately (https://docs.datomic.com/schema/identity.html). Marrow's catalog IDs and `Id(^store)` are in the same design family: identity is durable and semantic, not a table/field spelling.
- FoundationDB Record Layer is the closest architectural precedent for "semantics above ordered bytes." It is a structured record layer on FoundationDB with fields, types, schema evolution, primary and secondary indexes, nested data, and declarative queries (https://foundationdb.github.io/fdb-record-layer/index.html). Its FAQ explicitly avoids features with unbounded resource use unless constrained by primary-key order or indexes (https://foundationdb.github.io/fdb-record-layer/FAQ.html). This supports Marrow owning a tree-cell layer over redb, but it also strongly indicts unbounded materialization and broad traversal helpers.
- Redb is a reasonable local substrate for v0.1 because it supplies durable ordered key-value transactions, multiple readers, and one writer, while staying below Marrow semantics (https://docs.rs/redb/latest/redb/struct.Database.html). This argues against "Marrow is building a database engine from scratch"; the current implementation is closer to a semantic layer over an embedded KV store.
- SQLite is the strongest alternative for "do not own storage." SQLite's own docs say a SQLite file with a defined schema often makes an excellent application file format, with single-file documents, atomic transactions, cross-platform stability, and many language bindings (https://www.sqlite.org/appfileformat.html). But SQLite's query planner is an optimizer over SQL statements and indexes (https://www.sqlite.org/queryplanner.html), and transactions are SQL statement/connection semantics (https://sqlite.org/lang_transaction.html). Choosing SQLite would be a valid product pivot only if Marrow wants a SQL-ish table substrate or is willing to hide a table-mapping layer under the language. That hidden layer is exactly the compatibility glue Marrow is trying to avoid.
- PostgreSQL is a poor target for v0.1 local Marrow, unless the product changes. Its planner examines possible execution plans and join strategies, using costs and heuristics when exhaustive planning is too expensive (https://www.postgresql.org/docs/current/planner-optimizer.html). Its cost constants and statistics tuning are server-operational concepts (https://www.postgresql.org/docs/current/runtime-config-query.html). Marrow's "source is access path" model is not inferior because it is not SQL; it is a different, more explicit modeling contract. The danger is only if Marrow grows hidden optimizers while still claiming source-visible cost.
- Gel/EdgeDB is a useful hybrid precedent: declarative schema files are edited, a migration plan is generated, user clarification may be required, and both schema files and migration files are checked in (https://docs.geldata.com/reference/datamodel/migrations). This is friendlier than raw SQL migrations but still retains migration files and server-generated plans. Marrow is more novel because it wants one-off source `evolve` intent rather than durable migration history.
- Prisma Migrate and Alembic are migration failure-mode precedents. Prisma uses a shadow database to detect drift and potential data loss before generating migrations (https://www.prisma.io/docs/orm/prisma-migrate/understanding-prisma-migrate/shadow-database). Alembic explicitly says autogeneration is not perfect and generated migrations require human review (https://alembic.sqlalchemy.org/en/latest/autogenerate.html). Marrow's source-native approach is attractive because it makes rename/default/transform intent explicit, but the implementation must be even more exact because it rejects the safety net of a migration ledger.

## 4. Alternatives Considered

Target SQLite directly. This is the simplest credible reversal: compile resources to SQLite tables, use SQLite transactions, use SQLite backup tooling, and let users inspect the file with ordinary tools. The cost is semantic leakage. Marrow would need to map trees, stable catalog IDs, keyed child layers, generated indexes, typed references, evolution witnesses, and source-visible cost onto tables. If that mapping is public, Marrow becomes an ORM/schema DSL over SQLite. If it is private, it becomes hidden compatibility glue with SQL semantics underneath. This is acceptable only if the product target pivots to "SQLite with a nicer language," not if the target remains source-native typed trees.

Target PostgreSQL. This is not a v0.1 fit. It would buy concurrency, server ops, indexes, and mature query planning, but it would force Marrow into a remote DB/server product and pressure it toward SQL optimizer semantics. It also contradicts the local embedded target in the foundation ADRs and roadmap (`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/02-product-target-and-v1-scope.md:19-28`, `33-43`). A Postgres backend might be a later adapter only if Marrow's tree-cell contract stays authoritative.

Use MongoDB/CouchDB-style documents without a compiler-owned catalog. This would be more NoSQL-idiomatic at first: store documents, validate at boundaries, and let shape evolve gradually. It loses Marrow's main differentiation: stable durable identity across renames, typed references as `Id(^store)`, fail-closed enum/member storage, source-visible access cost, and exact evolution witnesses.

Use a separate migration DSL or generated migration files. This is conventional and easier to explain. Gel, Prisma, Rails, and Alembic show that this is mature. It also creates a second authority for durable intent, which Marrow explicitly rejects (`docs/data-evolution.md:229-239`). The Marrow approach is better only if the one-off `evolve` blocks, catalog proposal, and runtime apply agree exactly. Right now that agreement is not fully proven for proposal-only members.

Keep the current redb-backed tree-cell layer. This is the best v0.1 foundation if narrowed. Redb owns ordered durable bytes and single-writer/multi-reader behavior; Marrow owns typed tree cells, catalog identity, indexes, evolution, and backup. The refinement is to delete or bless every unused `TreeStore` facade surface, centralize data path semantics, add missing conformance laws, and avoid pretending debug protocols are product APIs.

Build a general Marrow database server/query engine. Reverse this before v0.1. The code does not have a production semantic protocol; it has debug data operations, a diagnostics-only LSP, and CLI inspectors. Shipping a server/query promise now would turn current path strings, JSON segment codecs, cursor shapes, and dump bytes into accidental API.

## 5. Verdict

Verdict: refine, with a ranked reverse-before-v0.1 list.

Do not reverse the whole direction into SQLite/Postgres/SQL. The differentiated foundation is real: a typed tree language with store-owned identity, compiler-owned catalog IDs, source-visible access paths, managed write/index maintenance, data-attached evolution witnesses, and typed backup/restore. These are not things SQLite, Postgres, MongoDB, or redb give directly. Marrow owning a typed tree-cell layer over redb is acceptable and probably optimal for v0.1's local embedded target.

Do reverse any drift toward becoming a general database engine, public data server, hidden ORM, or migration framework. The code is not ready for those claims, and making them now would cement the weakest surfaces.

Foundations worth keeping:

- `resource`/`store` split, typed resource trees, and saved roots under `^`.
- `Id(^store)` and catalog-backed stable IDs. This is more idiomatic for Marrow than `Book::Id`, because identity belongs to durable store role plus key, not resource shape.
- Redb as private ordered-byte engine, not public backend semantics.
- Store-owned indexes as generated lookup trees, not user-maintained secondary records.
- Source-visible cost and hidden-traversal rejection. This is not a SQL optimizer, and that is the point.
- Source-native evolution with explicit `rename`, `default`, `transform`, and `retire`, as long as it remains narrow and exact.
- Typed backup/restore that validates source/catalog/engine/data and rebuilds derived indexes.

Risky but defensible:

- No query optimizer. This is elegant if Marrow stays access-pattern-first like a document/KV language; it becomes hostile if the language surface grows arbitrary joins, broad scans, or implicit traversals.
- Source-native evolution. It is novel and costly, but useful if implemented as a compiler/catalog/data proof, not as a hidden migration ledger.
- The typed tree-cell store. It is justified only while redb remains private and while store APIs do not expose raw physical key compatibility.
- Debug/admin data inspection. It is useful, but it must stay visibly debug/admin until a canonical semantic data API exists.

Weak foundations to cut or fix before v0.1:

1. Proposal-only evolution apply mismatch. If a source proposal can discharge as activatable but runtime apply cannot stage it before accepting the proposal, v0.1's source-native evolution story is unsound.
2. Production serve/query/API promises. Current `debug_data_*` is a debug protocol, not a product protocol.
3. Duplicate data path/query/key rendering. CLI path text, serve JSON, explain, trace, and checker durable paths need one semantic owner.
4. Unbounded materialization and shallow orphan classification in storage/evolution/tooling. Record Layer's bounded-resource lesson applies hard here.
5. Prototype grammar sediment and stale ADR claims such as `Book::Id`, `merge`, `lock`, and old byte-archive evidence.
6. Large multipurpose Rust files in checker/evolution/store. They are not just aesthetic debt; they make soundness review unreliable.

## 6. Long-Term Risks

Duplicate semantics. Marrow's whole promise depends on one source of durable meaning. Current duplication is already visible in CLI/serve path models, name-based index maintenance, member traversal helpers, and catalog path strings. If this survives v0.1, every tool will become a subtly different Marrow.

Dead prototype paths. The grammar and formatter still carry reserved or prototype constructs, while checker code fences them after parse. This is tolerable only if the parser/AST is intentionally broader than v0.1; otherwise it is compatibility sediment.

Weak Rust shape. The largest files are exactly the semantic hotspots: checking, evolution discharge, catalog binding, and store traversal. A 2500-line discharge module that includes proposal IDs, scans, repair verdicts, and text path ownership is too easy to review shallowly. The architecture needs smaller modules with one invariant each before more language surface is added.

Hidden compatibility glue. Data dump/get/explain and serve debug paths are useful tools, but path strings and byte renderings can become de facto stable APIs. Backup streaming data-family cell bytes is acceptable because manifest, validation, and index rebuild wrap it; it becomes weak if callers learn to depend on the physical cell stream.

Unidiomatic language/database design. The risk is not that Marrow is insufficiently SQL-like. The risk is that it becomes neither elegant NoSQL/tree nor mature relational: too much explicit ceremony for small apps, too little optimizer/query power for relational workloads, and too much compiler/catalog machinery for document-store users. The design must keep earning its cost by making typed tree identity, evolution, and source-visible access feel natural.

Local embedded limits. Redb and the current foundation are many readers plus one writer. That is good for a local language/database kernel, not for multi-tenant server claims. Future server or sync work must be explicit typed architecture, not a thin wrapper around debug data operations.

Evolution novelty. Gel, Prisma, and Alembic keep migration histories because schema evolution is hard and irreversible. Marrow's one-off `evolve` blocks are elegant, but only if preview/apply/source digest/catalog proposal/runtime place lowering are exact. The proposal-only mismatch is a warning sign.

Restore and repair boundary. Restore rejects orphan backup cells and verifies
before commit. Repair tooling may still need deeper classification for existing
local-store debris, but restore is no longer a raw debris-preserving import path.

## 7. Concrete Follow-Up Recommendations

Ranked by foundation risk:

1. Fix or explicitly reject proposal-only `evolve default` and `evolve transform` apply before v0.1. Add a runtime/CLI apply fixture where accepted catalog lacks the new required member, current source adds it with `evolve default`, old records exist, and apply must backfill before accepting the proposal. Either executable places must carry proposal data-cell IDs, or preview must mark such changes non-activatable until accepted.
2. Keep `marrow serve` as debug/admin only until there is a stable semantic API. Do not rename `debug_data_*` to product operations. Do not ship a public query/data protocol on path strings, raw bytes, or tool-local classifiers.
3. Centralize durable data path/query/key encoding into one shared semantic API. Delete the permissive `SourceMember` collapse or make it a strictly diagnostic parser layered over canonical typed segments. Standardize byte key rendering across dump, trace, serve, and explain.
4. Keep commit metadata boundary tests in place: multi-write transactions and generated index writes must describe the whole physical commit.
5. Keep backend conformance for the prohibited snapshot/write overlap so memory and redb cannot diverge.
6. Keep store traversal and evolution backfill on streaming visitors or paged cursors where the contract is unbounded data. Where full materialization remains, document it as a v0.1 bound with tests.
7. Strengthen orphan/integrity semantics without weakening restore. Keep restore
   rejecting orphan backup cells, and make repair/data-integrity tooling report
   existing local-store debris with deeper typed classification.
8. Decide whether `TreeStore` node/leaf/sequence APIs are production, test/debug, or removable. If runtime production writes use generic data/index paths, old facade shapes should not survive by accident.
9. Split `crates/marrow-check/src/evolution/discharge.rs` before adding any new evolution semantics. Suggested owners: proposal ID resolution, structural compatibility, data scans, default/transform obligations, index obligations, repair diagnostics, and witness accumulation.
10. Replace semantic string path parsing in catalog/evolution (`CatalogKey { path: String }`, path formatting, `rsplit_once`) with typed catalog addresses where feasible. Stable IDs are weakened whenever path text still carries hidden structure.
11. Update stale ADR/docs in place, not by adding ADRs. Remove `Book::Id` alias claims, mark Lane 10 status accurately, remove stale `archive.rs` evidence, and align index-depth wording between ADRs, language docs, and implementation.
12. Keep SQL and Postgres adapters out of v0.1. If external engines are revisited, treat them as implementations of the typed tree-cell contract or as an explicit product pivot. Do not grow a hidden table-mapping layer while still claiming source-native tree semantics.

Final fair read: Marrow's long-term foundation is not weak because it rejects SQL. It would be weak if it kept broad debug surfaces, duplicated path semantics, oversized semantic dispatchers, and proposal/apply gaps while adding more language. Refine the current direction, cut accidental API surface now, and keep the language elegantly tree-native.
