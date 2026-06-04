# Lane 9 Source-Native Evolution And Activation Audit

Topic: Lane 9 source-native evolution and activation

Date: 2026-06-04

Auditor: Marrow v0.1 architecture research

## 1. Local Vision Summary With File/Line References

Git state inspected first:

- `/Users/scottwilliams/Dev/marrow` was clean on `main` at `854cd150da9a0a5a6d21014b0c809aa95da818f0` before this report branch was created. Existing worktrees included lane/research checkouts for lanes 5, 6, 7, 8, and 10; dependency files had no local churn.
- `/Users/scottwilliams/Dev/marrow-decisions` was dirty on `main` with unstaged edits to `/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md` and `/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md`. Those edits are included below, not ignored.

The local vision is not conventional migration history. It is source-native, catalog-bound, data-attached activation:

- The user-facing data evolution contract says schemas evolve through source changes plus source-native intent, durable identity is recorded automatically by first `run` or `evolve apply`, `preview` proves data obligations, and `apply` consumes the exact preview witness or fails closed (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:3`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:7`).
- The accepted change table distinguishes source-only changes, explicit `default` and `transform`, destructive `retire`, index rebuilds, fail-closed type/key-shape changes, and maintenance/repair paths (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:20`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:32`).
- Renames are explicit catalog decisions. The docs reject best-effort source diffs and migration scripts for preserving identity (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:131`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:147`).
- The accepted catalog is generated, committed metadata, not source syntax. `check` reads it and may propose changes but never writes it; only `run` and `evolve apply` establish or advance durable identity (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:149`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:176`).
- Activation fencing binds store epoch, source digest, and engine profile. v0.1 supports exactly the compiled epoch and schema, with stale or behind binaries failing closed before writes (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:186`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:225`).
- Index rebuilds are derived work: apply rebuilds and stamps in one transaction, and failed rebuilds publish no partial data (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:241`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:261`).
- Repair is not an unchecked bypass. A repair-required witness blocks check/preview/apply; repair is modeled maintenance code verified before and after (`/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:282`, `/Users/scottwilliams/Dev/marrow/docs/data-evolution.md:296`).
- The language reference says durable identity is invisible catalog infrastructure, assigns opaque IDs, and advances only on `evolve apply`; source spelling alone is not identity (`/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:179`, `/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:187`).
- `evolve` syntax covers `rename`, `default`, `retire`, and `transform`; the docs state that a bare source diff implies nothing about stored data (`/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:613`, `/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:635`).
- Transform is intentionally constrained: it is per-record, pure, old-only, no saved-data reads, no host effects, no same-evolve reads of rewritten members, and every read member must decode under its current type before apply (`/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:643`, `/Users/scottwilliams/Dev/marrow/docs/language/resources-and-storage.md:674`).
- The CLI contract says `evolve preview` discharges source, accepted catalog, store snapshot, and engine metadata into an exact witness; `evolve apply` recomputes that witness, requires exact match, gates the activation window, and commits data plus metadata in one transaction (`/Users/scottwilliams/Dev/marrow/docs/cli.md:97`, `/Users/scottwilliams/Dev/marrow/docs/cli.md:114`).
- The roadmap excludes an ORM layer, separate migration DSL, and hidden database-owned migration ledger (`/Users/scottwilliams/Dev/marrow/docs/roadmap/README.md:33`, `/Users/scottwilliams/Dev/marrow/docs/roadmap/README.md:40`).
- The Lane 9 doc reports the lane complete and states the essential discipline: no migration scripts, no source-diff identity inference, one proof-discharge pipeline, exact witness apply, fail-closed activation, and split review by invariant (`/Users/scottwilliams/Dev/marrow/docs/roadmap/lanes/lane-09-evolution-activation.md:15`, `/Users/scottwilliams/Dev/marrow/docs/roadmap/lanes/lane-09-evolution-activation.md:49`).
- ADR evolution 01 accepts "source-native, migrationless evolution" and explicitly rejects a separate migration subsystem, migration DSL, or hidden database ledger (`/Users/scottwilliams/Dev/marrow-decisions/adr/evolution/01-source-native-migrationless-evolution.md:9`, `/Users/scottwilliams/Dev/marrow-decisions/adr/evolution/01-source-native-migrationless-evolution.md:26`).
- ADR evolution 02 defines the lifecycle as `preview -> inspect -> approve or reject -> apply -> verify`, with exact witnesses, drift aborts, exact destructive approvals, bounded compatibility lenses, and epoch windows (`/Users/scottwilliams/Dev/marrow-decisions/adr/evolution/02-evolution-lifecycle-and-obligations.md:11`, `/Users/scottwilliams/Dev/marrow-decisions/adr/evolution/02-evolution-lifecycle-and-obligations.md:48`).
- ADR catalog identity makes the catalog the durable ABI, not a migration ledger. Rename is never inferred from spelling, and apply consumes exact source/catalog/engine/data/count facts (`/Users/scottwilliams/Dev/marrow-decisions/adr/catalog-identity/02-catalog-lifecycle-and-identity-binding.md:18`, `/Users/scottwilliams/Dev/marrow-decisions/adr/catalog-identity/02-catalog-lifecycle-and-identity-binding.md:82`).
- The recent unstaged ADR edits strengthen this direction by adding "the access path is the source" and rejecting below-language runtime selection of operation shapes (`/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:56`, `/Users/scottwilliams/Dev/marrow-decisions/adr/foundations/01-architecture-laws-and-five-layers.md:75`), and by saying write lowering may only elide provably redundant operations, never choose semantically distinct work by statistics (`/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:28`, `/Users/scottwilliams/Dev/marrow-decisions/adr/storage-engine/02-transactions-commits-and-recovery.md:94`).

## 2. Implementation Summary With Crate/Module References

The implementation substantially matches the local vision.

- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/mod.rs` exposes a read-only evolution proof surface split into intents, leaf typing, discharge, preview, transform-read resolution, and witness modules. This is the right crate boundary: checker owns classification and proof, not mutation.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/witness.rs` defines `EvolutionWitness`, digest fields, catalog/store/engine facts, changed root/index IDs, verdicts, and counts. `is_activatable` is false when any verdict is repair or destructive approval.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/preview.rs` composes a witness from checked source plus live store facts and remains read-only.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/discharge.rs` is the proof kernel. It classifies store-key shape changes, source roots, absent source entries, transforms, and a default-deny structural backstop in one discharge path (`/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/discharge.rs:68`, `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/discharge.rs:134`). This supports the "one pipeline" invariant but is large enough to be a future maintainability risk.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/intents.rs` rejects bare source diff semantics, typechecks defaults/transforms, and enforces transform purity and old-only reads.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/leaf_type.rs` uses identity-aware tokens rather than source-spelling comparison for stored leaf shape.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/catalog.rs` binds source declarations to accepted catalog metadata, resolves renames and aliases, records pending identities, applies retire/default/transform metadata, and produces catalog proposals. This is the right place for identity recording semantics because the store and CLI should not rediscover identity.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/lib.rs` has the production catalog writer discipline: `commit_pending_identity` writes only an initial baseline and never auto-advances an accepted catalog; `write_accepted_catalog` is the single production catalog writer with atomic file replacement (`/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/lib.rs:332`, `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/lib.rs:386`).
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/apply.rs` validates the exact witness, fences against the pre-apply store shape, gates repair/destructive approval, stages verdict work, checks staged counts against witness counts, stamps metadata, and commits atomically (`/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/apply.rs:84`, `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/apply.rs:216`).
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/validate.rs` recomputes preview and requires full witness equality before apply.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/window.rs` implements the v0.1 exact epoch/source-digest/engine-profile activation fence.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/admission.rs` admits only activatable witnesses, requires maintenance for destructive retire, and matches approval by exact catalog ID and populated count.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/backfill.rs` stages default backfills, index rebuilds/drops, and retires from checked places and live store scans, not from an opaque migration list.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/src/evolution/transform.rs` uses runtime evaluation and existing codecs rather than a second transform interpreter, which lowers duplicate-semantics risk.
- `/Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_evolve/mod.rs` keeps the CLI surface to `preview` and `apply`, reuses preview for data-attached checks, commits pending identity first, applies the witness, writes the accepted catalog after the store commit, and handles the crash window by exact resume.
- `/Users/scottwilliams/Dev/marrow/crates/marrow/src/cmd_run.rs` records baseline identity before durable run and fences persistent stores before executing.

Test coverage is also aligned with the design:

- `/Users/scottwilliams/Dev/marrow/crates/marrow-check/tests/evolution_discharge.rs` exercises source-driven proof cases for defaults, renames, enum changes, structural/key-shape failure, index obligations, transforms, and repair verdicts through the check pipeline.
- `/Users/scottwilliams/Dev/marrow/crates/marrow-run/tests/evolution_apply.rs` explicitly says every drift dimension is exercised by mutating the witness or store and proving apply aborts before staging a write (`/Users/scottwilliams/Dev/marrow/crates/marrow-run/tests/evolution_apply.rs:1`, `/Users/scottwilliams/Dev/marrow/crates/marrow-run/tests/evolution_apply.rs:9`). Test names cover source digest drift, transform digest drift, store commit drift, stale writer fencing, engine profile drift, destructive approval, maintenance, rollback, and idempotence.
- `/Users/scottwilliams/Dev/marrow/crates/marrow/tests/evolve_cli.rs` covers preview rendering, apply consuming a witness, repair rejection, destructive approval rendering, accepted catalog/store lockstep for retire and rename, half-applied resume, divergent resume fail-closed, noop apply, and absence of legacy `evolve migrate` subcommands.

An absence scan for migration scripts, source-diff identity, best-effort rename, transform shims, repair bypasses, and hidden ledgers found the terms mostly in docs as rejected concepts or in negative tests. Production code mentions "migrate" only as fail-closed diagnostics for unsupported store/key shape migration, redb storage format refusal, and the CLI negative test for `evolve migrate`.

## 3. External Precedents And Counter-Precedents With Source Links

Conventional migration-history systems are strong counter-precedents, but mostly for stacks where source and database are separate authorities:

- Rails Active Record migrations use timestamped migration files and a database `schema_migrations` table to track applied versions. Rails warns that replaying all migrations can drift if migrations were altered, reordered, or removed, and it recommends new migrations instead of editing committed ones. Source: [Rails Active Record Migrations](https://guides.rubyonrails.org/active_record_migrations.html), especially "Rails Migration Version Control" and "Old Migrations".
- Prisma Migrate is a hybrid declarative/imperative tool: the Prisma schema describes the target, generated SQL files form migration history, and `_prisma_migrations` stores metadata. Prisma uses a shadow database to detect drift by replaying migration history and comparing end state to the development database. Source: [Prisma Migrate overview](https://docs.prisma.io/docs/orm/prisma-migrate), [Prisma mental model](https://www.prisma.io/docs/orm/prisma-migrate/understanding-prisma-migrate/mental-model), [Prisma shadow database](https://www.prisma.io/docs/orm/prisma-migrate/understanding-prisma-migrate/shadow-database).
- EF Core records applied migrations in `__EFMigrationsHistory`, and customising that table after applying migrations makes the user responsible for updating existing database state. Source: [EF Core custom migrations history table](https://learn.microsoft.com/en-us/ef/core/managing-schemas/migrations/history-table).
- Flyway adds a schema history table as an audit trail with checksums and success/failure state, validates applied migrations against local files, and has `repair` when files/checksums no longer match the history. Source: [Flyway schema history table](https://documentation.red-gate.com/flyway/flyway-concepts/migrations/flyway-schema-history-table).
- Liquibase stores ordered changelogs in source control, identifies changesets by author/id/filename, and records executed changesets in `DATABASECHANGELOG`. It supports raw SQL and preconditions, which are powerful but create a separate schema-change language. Source: [Liquibase core concepts](https://www.liquibase.org/get-started/core-usage/liquibase-core-concepts-author-database-changes).

Schema-compatibility systems are closer precedents for Marrow's stable-identity and compatibility-lens direction:

- Avro supports schema resolution through defaults and aliases; old writer schema and new reader schema are both involved in decoding. Avro also says data should include the writer schema and that the exact same schema is safest. Source: [Apache Avro specification](https://avro.apache.org/docs/1.12.0/specification/).
- Protocol Buffers make numeric field identity durable. Field numbers cannot change once in use, should never be reused, and deleted fields should reserve both numbers and names to prevent ambiguous decoding or data corruption. Source: [Proto3 language guide, field numbers and deleting fields](https://protobuf.dev/programming-guides/proto3/).
- Datomic schema is data and supports changing an ident while preserving the underlying entity id; invariant-violating schema changes abort until the data is made valid. Adding an index can require waiting for schema/index sync before use. Source: [Datomic changing schema](https://docs.datomic.com/schema/schema-change.html).

Operational migration practice gives Marrow important cautionary examples:

- GitLab's zero-downtime migration docs show that destructive changes, renames, type changes, and defaults often require multi-release expand/contract choreography, temporary columns/triggers, post-deployment cleanup, and schema-cache awareness. Source: [GitLab avoiding downtime in migrations](https://docs.gitlab.com/development/database/avoiding_downtime_in_migrations/).
- PostgreSQL `CREATE INDEX CONCURRENTLY` avoids blocking writes but uses multiple transactions/scans, can leave invalid indexes on failure, and has caveats for uniqueness and snapshots. Source: [PostgreSQL CREATE INDEX](https://www.postgresql.org/docs/15/sql-createindex.html).
- Event-sourced systems often preserve immutable old events and use handler-time conversion or upcasters. Axon recommends handler-time payload conversion for simple field additions/removals/renames and uses ordered upcasters for stored-structure changes such as splitting/merging events. Source: [Axon event versioning](https://docs.axoniq.io/axon-framework-reference/development/events/event-versioning/).

The pattern across these systems: conventional migration history is valuable when the database is an independent mutable artifact and change scripts can do arbitrary work. Stable schema identity, explicit defaults/aliases, invariant checks, and bounded compatibility are valuable when the data format and compiler can be reasoned about together. Marrow is intentionally closer to the latter group, with some online-operational obligations borrowed from the former.

## 4. Alternatives Considered

1. Conventional migration history as primary model.

   This would introduce source-controlled migration files plus a store-side migration table, closer to Rails, Prisma, EF, Flyway, and Liquibase. It improves operator familiarity and audit trails, but it creates a second semantic language, duplicates checker/runtime meaning, and makes source/catalog/data drift a reconciliation problem rather than a compile-time fact. It conflicts with Marrow's documented rejection of a migration DSL and hidden ledger.

2. Generated migrations from source diff.

   This looks ergonomic but is the weakest option. Source diffs cannot distinguish rename from delete-add, split, merge, or retire without intent. Prisma documents this exact weakness for `db push`: it cannot tell a column rename and may reset or lose data. Marrow's catalog identity design correctly rejects this path.

3. Lazy runtime compatibility adapters.

   Avro, event-sourced upcasters, and some serialization stacks show that read-time evolution can work. For Marrow, a general old-schema runtime would become hidden permanent migration code unless lenses remain pure, total, epoch-scoped, and visible. The accepted ADRs correctly restrict v1 lenses and push nontrivial reshaping to checked transforms or repair.

4. Event-sourced immutable history with upcasters.

   This would preserve all old facts and transform at read/projection time. It is useful for append-only event logs, not for Marrow's local tree-cell store where latest-format writes, managed paths, indexes, and durable resource invariants are the language contract. It would import a lot of projection/replay machinery without matching Marrow's storage model.

5. Source-native exact witnesses plus non-semantic activation receipts.

   This is the best refinement path: keep source/catalog/data/engine proof as the authority, but record a typed receipt of applied activation facts for audit, restore, support, and reproducibility. The receipt must not be executable migration history and must not become a hidden schema ledger; it should be derived evidence tied to catalog epoch, source digest, store commit id, engine profile, affected IDs, verdicts, counts, approval identities, and outcome.

## 5. Verdict: Refine

Keep the source-native, migrationless evolution model as Marrow's long-term foundation. Do not adopt conventional migration history as the primary semantics.

The current model is not merely a v0.1 convenience. It follows from Marrow's core premise that source, accepted catalog, typed durable trees, checked IR, engine profile, and data snapshot compile into one conceptual machine. A conventional migration system would be familiar, but it would duplicate semantics outside the compiler and weaken the invariant that durable data is part of the Marrow program.

However, "migrationless" should not mean "historyless" or "jobless." The long-term model needs refinement in four places:

- applied activation receipts for audit/restore/support, explicitly non-executable and non-authoritative;
- source-native online activation jobs for large backfills/index builds/transforms, with explicit visibility, resume, fencing, and verification state;
- bounded compatibility windows beyond exact v0.1 epoch equality for rolling binaries;
- sustained Rust-shape hardening so the single proof pipeline does not become an oversized semantic dispatcher.

The exact preview/apply witness rule is the strongest part of Lane 9. Apply consuming only the exact preview witness is stricter than ordinary migration-history tools, where a script can run against a database whose actual contents or concurrent writer state differ from the assumptions under which the script was reviewed.

## 6. Long-Term Risks

Duplicate semantics risk: moderate. The design actively avoids duplicate semantics by having the checker classify obligations and the runtime execute checked facts. Transform apply uses the runtime evaluator rather than a second transform interpreter. The risk is concentrated in the large discharge classifier, CLI rendering, and test fixtures. Any future tooling that reclassifies evolution outside `marrow-check` would be a regression.

Dead prototype paths risk: low but keep scanning. The CLI negative test for `evolve migrate` and the absence scan indicate migration-script and source-diff paths are rejected or absent, not merely dormant. The remaining "migrate" mentions in production code are fail-closed diagnostics for unsupported store/key shape migration, which is acceptable.

Weak Rust shape risk: moderate. `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/discharge.rs` is 2572 lines and `/Users/scottwilliams/Dev/marrow/crates/marrow-check/src/evolution/intents.rs` is 1001 lines. They appear cohesive, but the size is already close to the point where future changes could create catch-all dispatch, duplicate classifiers, and comment sediment. This is not a reason to reverse the architecture, but it is a foundation risk before more evolution cases are added.

Hidden compatibility glue risk: moderate. v0.1 exact epoch/source-digest fencing is simple and good. Long-term rolling deploys will pressure the system to add ad hoc "old binary can still write" exceptions. The correct answer is explicit supported epoch windows and generation fencing, not silent compatibility branches.

Hidden history ledger risk: low today, higher long term. The catalog records identity decisions and epochs, and the store records activation stamps. That is not a migration ledger. But support, backup/restore, and incident analysis will want "what happened here?" evidence. If Marrow does not define a typed activation receipt, operators may recreate migration histories out of band.

Unidiomatic database-design risk: mixed. Rejecting SQL-style migration history is idiomatic for Marrow but unusual for application teams. The model is defensible because Marrow controls source semantics and storage layout together. The place where conventional systems have the stronger story is online operations: expand/contract deployments, nonblocking index builds, progress/resume, partial failure cleanup, and background verification.

Transform expressiveness risk: acceptable v0.1, incomplete long term. Per-record, pure, old-only top-level transforms are the right safe first contract. Future split/merge/multi-record changes need a typed source-native job model rather than host scripts or general migration files.

Catalog/store atomicity risk: acceptable v0.1, worth tightening. `evolve apply` commits store state and then writes the accepted catalog file, with exact resume for the crash window. That is a pragmatic local-store design. If multiple processes, sync tools, or package distribution enter the picture, catalog file CAS/locking and receipt validation become more important.

Online index rebuild risk: underdeveloped for long-term scale. Current apply rebuilds atomically and publishes no partial index. This is clean for v0.1 but not enough for large data. PostgreSQL and GitLab both show that online builds need explicit invalid/building/valid states, snapshot rules, write maintenance during build, failure cleanup, and user-visible progress.

Repair admission risk: acceptable, with operator ergonomics gap. Repair is correctly not a bypass. But "write maintenance code, then verify" may be too underspecified for users repairing production data. The long-term model needs typed repair previews and examples without adding a magic `repair` command that weakens semantics.

Source-owned access-path risk: coherent with recent ADR edits. The unstaged ADR edits make migrationless evolution more plausible because evolution proofs can name the source-forced access path and bounded scans instead of reverse-engineering hidden runtime choices. The risk is developer ergonomics: source must make expensive scans explicit and explainable.

## 7. Concrete Follow-Up Recommendations Ordered By Foundation Risk

1. Add an "activation receipt" design, not a migration history.

   Define a typed receipt persisted with store commit metadata and optionally mirrored in generated catalog metadata or backup manifests. It should include epoch, source digest, accepted/proposal catalog digests, engine profile, store commit pin, affected stable IDs, verdict kinds, counts, approval identities, and final commit id. It must be read-only evidence, not executable change history.

2. Split the evolution proof kernel by invariant before adding more evolution kinds.

   Keep one public discharge pipeline, but break the implementation into focused modules such as identity/key-shape, member presence/defaults, enum compatibility, index obligations, absent-source retire/drop, transform admission, and structural backstop. Add absence tests that no sibling module reclassifies a verdict already owned by another.

3. Specify online activation jobs.

   Add a source-native job protocol for large index rebuilds, backfills, and transforms: preview creates a bounded plan, apply starts or resumes a job, writes maintain both old and building derived state when required, production visibility changes only after verification, failed jobs remain nonvisible and cleanly retryable. Model this after PostgreSQL concurrent index caveats and GitLab expand/contract practice, but keep it under Marrow witness semantics.

4. Make compatibility windows explicit before multi-binary deployment exists.

   Replace "exact v0.1 epoch only" with a future ADR/spec for `[min, max]` supported epoch ranges, generation fencing, write admission rules, and old-binary lockout. Do not add ad hoc backward-write cases.

5. Keep source-diff tooling as edit assistance only.

   It is safe for an editor or CLI helper to suggest `evolve rename` based on a source diff, but the suggestion must create source intent and a fresh preview. It must never be accepted by apply as inferred identity.

6. Add transform roadmap boundaries.

   Keep v0.1 per-record transforms. For split/merge/multi-record changes, require a future source-native checked job form with explicit old reads, bounded traversal facts, deterministic output, and witness counts. Do not fall back to host migration scripts.

7. Strengthen repair ergonomics without creating a bypass.

   Provide examples and diagnostics that guide maintenance repair functions, exact data integrity checks, and post-repair preview. A typed repair preview is useful; a privileged repair command that mutates raw store state would be a reversal.

8. Tie backup/restore to activation receipts and index verification.

   Restore should verify catalog IDs, source digest compatibility, engine profile, activation receipts, and derived index validity. Indexes may be restored as optimization only if verified; otherwise rebuild from source data before visibility.

9. Preserve the no-migration-script line in docs and tests.

   Keep negative CLI tests for `evolve migrate`, absence scans for migration DSL/source-diff/best-effort rename/transform shim paths, and review criteria that fail any new duplicate evolution language.

10. Revisit operator-facing wording.

   "Migrationless" is accurate internally but can mislead users into thinking there is no operational activation. Prefer "source-native evolution with exact activation witnesses" in user-facing docs, while still rejecting conventional migration files as the source of semantics.
