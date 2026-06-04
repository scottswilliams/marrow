# Lane 8 Architecture Audit: Checked Runtime Execution And Write Planner

Date: 2026-06-04

Scope: Lane 8 checked runtime execution and write planner. This is a research
and architecture review only.

Archive note: this report preserves historical research findings only. Its
environment notes are not current authority; use the accepted ADRs and canonical
docs for product truth.

## 1. Local Vision Summary

The local vision is unusually explicit: Marrow is not a general query engine with
a hidden optimizer. It is a programming language whose durable data is checked as
part of the program, and whose source syntax names the durable access path.

The central roadmap frames v0.1 as a replacement of prototype runtime behavior
with checked facts and checked IR. Its goals include replacing syntax/string-backed
runtime behavior with checked facts and executable IR
(`docs/roadmap/prototype-to-v1-execution-plan.md:15`) and deleting prototype-only
surfaces rather than preserving old and new production paths side by side
(`docs/roadmap/prototype-to-v1-execution-plan.md:19`). The lane ownership section
assigns runtime execution, `lock`, `merge`, saved `inout`, fallback name
resolution, runtime path classifiers, maintenance bypasses, and syntax-body
execution to Lane 8
(`docs/roadmap/prototype-to-v1-execution-plan.md:224`).

Lane 8's own document makes the contract sharper: production runtime must consume
checked entry calls, checked bodies, checked places, explicit write plans,
transaction state, and index-maintenance facts, with no syntax bodies, source-name
splitting, saved path recovery, or prototype preservation
(`docs/roadmap/lanes/lane-08-runtime-checked-execution.md:5`). It also states
that saved reads, writes, deletes, traversal guards, and index maintenance use
checked facts and catalog identities
(`docs/roadmap/lanes/lane-08-runtime-checked-execution.md:18`), durable loops
stream rather than materialize generic `keys`, `values`, `entries`, `reversed`,
or fallback iteration
(`docs/roadmap/lanes/lane-08-runtime-checked-execution.md:21`), and `lock`,
`merge`, saved `inout`, and production `~` roots are not v0.1 runtime constructs
(`docs/roadmap/lanes/lane-08-runtime-checked-execution.md:29`).

The language docs match that design. The cost model says there is no hidden
optimizer; code names stores, indexes, and fields, and the access path is source
(`docs/language/cost-model.md:3`). It defines the cost of a program as storage
operations performed by the checked program
(`docs/language/cost-model.md:11`), records traversal and write facts for tools,
checker, and runtime (`docs/language/cost-model.md:40`), rejects hidden traversal
while allowing explicit full traversal
(`docs/language/cost-model.md:43`), and gives a minimal-plan guarantee:
the planner may remove only provably redundant operations and must not choose
semantically distinct plans by runtime statistics
(`docs/language/cost-model.md:51`). The resources/storage doc similarly says
tools use checked tree-cell facts rather than physical storage details
(`docs/language/resources-and-storage.md:9`), paths and traversal are typed
(`docs/language/resources-and-storage.md:20`), store identity is canonical
`Id(^books)` rather than raw path text
(`docs/language/resources-and-storage.md:120`), indexes are store-owned generated
entries (`docs/language/resources-and-storage.md:211`), lookups are written as
paths, traversal, or declared indexes rather than a separate query language
(`docs/language/resources-and-storage.md:286`), and managed writes update data
and indexes together (`docs/language/resources-and-storage.md:324`).

The historical ADR edits in `marrow-decisions` strengthen the same conclusion. The
foundation ADR now adds law 15: the access path is source, store/index/field reads
and writes are written by hand, and the planner emits only minimal forced
operations (`adr/foundations/01-architecture-laws-and-five-layers.md:56` in the
current working tree). It also says a cost-based optimizer below the language is
foreclosed (`adr/foundations/01-architecture-laws-and-five-layers.md:68`). The
transaction ADR now says the write planner may only elide provably redundant work,
never add operations or choose semantically distinct plans by statistics
(`adr/storage-engine/02-transactions-commits-and-recovery.md:28`), and that write
path cost is a source property
(`adr/storage-engine/02-transactions-commits-and-recovery.md:91`).

The vision is therefore not "Marrow lacks an optimizer for now." It is "source is
the plan" as a language contract for v0.1, with future optimization limited to
source-visible diagnostics, source rewrites, source-declared indexes, or
proof-preserving redundant-operation elision.

## 2. Implementation Summary

The current Lane 8 implementation mostly realizes the vision.

Checked runtime artifacts are syntax-free at the public runtime boundary.
`CheckedRuntimeProgram` stores runtime modules, precomputed entry resolutions,
checked facts, catalog epoch, and source digest
(`crates/marrow-check/src/program.rs:371`). It is built from the checked program
and exposes read-only facts (`crates/marrow-check/src/program.rs:383`,
`crates/marrow-check/src/program.rs:419`). `CheckedRuntimeFunction` stores a
lowered checked runtime body, not a source syntax body
(`crates/marrow-check/src/program.rs:565`). The executable model carries checked
places, checked call targets, enum/member fact refs, and builtins
(`crates/marrow-check/src/executable.rs:36`,
`crates/marrow-check/src/executable.rs:67`,
`crates/marrow-check/src/executable.rs:78`).

Runtime entry execution now takes a checked boundary object. `CheckedEntryCall`
ties a resolved `CheckedFunctionRef` to its `CheckedRuntimeProgram`
(`crates/marrow-run/src/entry.rs:21`). The public `run_entry*` functions accept
that checked call rather than a raw entry string and program pair
(`crates/marrow-run/src/entry.rs:45`), and the single string lookup is confined to
boundary construction through the runtime program's precomputed entry map
(`crates/marrow-run/src/entry.rs:28`, `crates/marrow-run/src/entry.rs:126`). That
is the right boundary: caller-facing names are still strings, but execution is by
checked target.

The write planner is explicit and centralized. `WritePlan` consists of data
writes/deletes, record subtree deletes, index writes/deletes/subtree deletes, and
metadata stamps (`crates/marrow-run/src/write_plan.rs:9`). It commits in one
transaction unless already inside an open transaction
(`crates/marrow-run/src/write_plan.rs:74`) and exposes projected steps only for
debug/dry-run hooks (`crates/marrow-run/src/write_plan.rs:94`). Source assignments
and deletes route to focused plan builders in `write.rs`: resource writes clear
existing data and stage index rewrites
(`crates/marrow-run/src/write.rs:81`), resource deletes stage index deletes
(`crates/marrow-run/src/write.rs:119`), whole-root deletes plan a root and index
subtree delete (`crates/marrow-run/src/write.rs:139`), and field writes check
types, check unique conflicts, write data, and stage index rewrites
(`crates/marrow-run/src/write.rs:154`). Index maintenance is derived from checked
index facts and current stored data, not from a duplicate runtime schema
(`crates/marrow-run/src/index_maintenance.rs:17`,
`crates/marrow-run/src/index_maintenance.rs:197`).

Transaction behavior is source-structured and rollback-aware. The transaction
evaluator begins a transaction, runs the checked block, commits on normal control
flow including return/break/continue, and rolls back on escaping error or throw
(`crates/marrow-run/src/transaction.rs:8`). `Env` tracks transaction depth,
deferred required-field checks, maintenance deletes, and created required paths
(`crates/marrow-run/src/env.rs:43`). Plan application runs traversal guards,
generated-index guards, write hooks, metadata stamps, and commit
(`crates/marrow-run/src/env.rs:235`). Traversal guards reject mutation of a root,
child layer, or index branch currently being streamed
(`crates/marrow-run/src/env.rs:459`, `crates/marrow-run/src/env.rs:578`).

Durable traversal is checked and streaming. `SavedLoopSpec` recognizes durable
loop shapes from checked expressions and `SavedLoopPlan` dispatches to root,
index, unique-index, or child-layer scan implementations
(`crates/marrow-run/src/saved_iter.rs:45`,
`crates/marrow-run/src/saved_iter.rs:104`). The plan pushes a `TraversedLayer`
while streaming and pops it after the scan
(`crates/marrow-run/src/saved_iter.rs:137`). Standalone `keys`, `values`,
`entries`, and `reversed` materialize only local collections; durable collection
materialization fails closed with "iterate it directly"
(`crates/marrow-run/src/collection.rs:56`,
`crates/marrow-run/src/collection.rs:63`,
`crates/marrow-run/src/collection/materialize.rs:41`,
`crates/marrow-run/src/loop_exec.rs:515`). Count/exists use checked iterable
layers and bounded child-count operations rather than materializing children
(`crates/marrow-run/src/read.rs:130`).

Runtime values encode stable durable meaning. Enum runtime values carry enum and
member fact identities plus catalog IDs
(`crates/marrow-run/src/value.rs:53`), and enum stored/index values are encoded
through shared conversion functions (`crates/marrow-run/src/value.rs:71`).
Identity runtime values are lowered key segments, with composite identities kept
opaque rather than confused with arbitrary same-shaped scalar tuples
(`crates/marrow-run/src/value.rs:47`,
`crates/marrow-run/src/value.rs:165`).

Prototype surfaces are largely removed from production runtime. `saved inout` is
rejected at the checked call-argument boundary
(`crates/marrow-run/src/call_args.rs:302`). `merge`, `lock`, and saved `inout`
are checker-level prototype rejections
(`crates/marrow-check/src/prototype.rs:82`,
`crates/marrow-check/src/prototype.rs:144`,
`crates/marrow-check/src/prototype.rs:208`). Architecture tests assert the runtime
does not execute syntax bodies (`crates/marrow-run/tests/architecture.rs:4`),
runtime artifacts do not expose raw schema copies
(`crates/marrow-run/tests/architecture.rs:72`), entrypoints take checked calls
(`crates/marrow-run/tests/architecture.rs:1215`), and eval helpers do not keep
obsolete checked-entry helper shapes
(`crates/marrow-run/tests/architecture.rs:1254`).

Residual implementation risks are real but bounded:

- `DataAddress::raw` remains, currently used by evolution transform staging from
  already-resolved catalog IDs and live store identities
  (`crates/marrow-run/src/store.rs:17`,
  `crates/marrow-run/src/evolution/transform.rs:90`). This is not a production
  raw path resolver, but it must stay private/internal and should be called out in
  Lane 11 absence scans.
- `member_path_segments` still resolves a `Vec<String>` member path against
  checked members (`crates/marrow-run/src/store.rs:248`). It resolves names
  inside checked facts, not physical paths, but the shape is close enough to old
  raw-path code that a follow-up should either replace the string path with
  checked member refs or prove it cannot escape as production address semantics.
- The code has several comment-heavy invariant sections, especially in path
  lowering and evolution transform execution. Some comments are valuable
  soundness rationale, but Lane 11 should verify they are not compensating for
  over-broad functions.
- Lane 10 is not complete. Raw/path-addressed tooling and protocol surfaces remain
  explicitly active blockers
  (`docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:20`,
  `docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:126`). Lane 8 should be
  judged on runtime semantics, not on downstream CLI/protocol cleanup.

## 3. External Precedents And Counter-Precedents

SQL optimizers are the strongest counter-precedent to "source is the plan", but
they apply to a different language shape. PostgreSQL describes a planner that can
execute a SQL query in many equivalent ways and tries to select the fastest one;
when exhaustive search is too costly, it falls back to heuristics such as GEQO
for large joins:
[PostgreSQL Planner/Optimizer](https://www.postgresql.org/docs/17/planner-optimizer.html).
PostgreSQL's `EXPLAIN` docs emphasize plan trees, alternative scan/join nodes,
estimated total cost, row estimates, and cost parameters:
[Using EXPLAIN](https://www.postgresql.org/docs/current/using-explain.html). Its
planner statistics docs show that selectivity and table/index cardinality are
statistical and approximate:
[Statistics Used by the Planner](https://www.postgresql.org/docs/current/planner-stats.html).
This is appropriate for declarative set queries where the source does not choose
an access path. It is not a free upgrade for Marrow because introducing such a
planner below the language would break the current minimal-plan guarantee and
make runtime behavior depend on hidden statistics.

ORMs are a cautionary precedent for implicit access. SQLAlchemy documents lazy
relationship loading as "load upon attribute access" and warns that lazy loads
produce the common N+1 SELECT problem, with eager loading or `raiseload` needed to
control unintended I/O:
[SQLAlchemy relationship loading](https://docs.sqlalchemy.org/en/20/orm/queryguide/relationships.html#lazy-loading).
This supports Marrow's decision to reject hidden durable materialization and make
durable traversal explicit. Marrow's fail-closed `values/entries/reversed` behavior
for saved data resembles `raiseload` more than ORM default lazy loading, which is
good.

GraphQL is a partial precedent for "request shape is execution shape." The
GraphQL spec says execution generates a response from a request and executes only
executable definitions:
[GraphQL Execution](https://spec.graphql.org/October2021/#sec-Execution). In
practice GraphQL servers route field selections through resolvers; this is useful
for API composition, but it tends to push planning into resolver code and loader
libraries rather than provide one durable semantic model. For Marrow, a GraphQL-like
resolver layer would be a tool/API story, not the runtime foundation.

Datalog is a strong precedent for checked facts and declarative derivation, but a
counter-precedent for Marrow's imperative source-as-plan claim. Souffle describes
Datalog as a declarative logic query language for recursive queries:
[Souffle tutorial](https://souffle-lang.github.io/tutorial). Its rule docs ground
variables through positive predicates and expose execution-order qualifiers for rule
execution order:
[Souffle rules](https://souffle-lang.github.io/rules). Its relation docs describe
auto-indexing and magic-set/inlining transformations:
[Souffle relations](https://souffle-lang.github.io/relations). This shows that
declarative recursive systems benefit from automatic index and rule-plan selection,
but also that those systems have a relation/rule semantics unlike Marrow's
hand-written store/index/path operations.

FoundationDB is strong precedent for explicit transactional key-value foundations
and source-visible composition. Its developer guide describes global ACID
transactions with strict serializability, MVCC reads, optimistic writes, retry
loops, read/write conflict ranges, and idempotence concerns for unknown commit
results:
[FoundationDB Developer Guide](https://apple.github.io/foundationdb/developer-guide.html#transactions-in-foundationdb).
Its transaction processing overview says optimistic transactions avoid per-key
locks and are validated by a transactional authority:
[FoundationDB Transaction Processing](https://apple.github.io/foundationdb/transaction-processing.html).
This supports Marrow's separation of checked language semantics from ordered-byte
engine mechanics, while warning that ambiguous commits and idempotence are real
future contracts if Marrow grows beyond a local engine.

SQLite supports the local single-writer choice. SQLite documents serializable
isolation by serializing writes, with only one writer at a time:
[SQLite Isolation](https://www.sqlite.org/isolation.html). Its transaction docs
state that SQL statements auto-start transactions, explicit transactions persist
until commit/rollback, nested transactions require savepoints, and multiple read
transactions can coexist with only one simultaneous write transaction:
[SQLite Transactions](https://www.sqlite.org/lang_transaction.html). WAL docs also
warn that WAL files are persistent state and must be copied with the database:
[SQLite WAL](https://www.sqlite.org/wal.html). These are good precedents for
Marrow's v0.1 local serial writer, nested transaction/savepoint behavior, and
typed backup/restore concern.

Embedded scripting runtimes are a counter-precedent for durable semantics. Rhai
is a small embedded scripting engine for Rust:
[Rhai](https://rhai.rs/). It can compile scripts to AST and evaluate that
precompiled AST:
[Rhai compile to AST](https://rhai.rs/book/engine/compile.html). This is exactly
right for dynamic scripts, but weak for Marrow's saved data: AST execution would
keep runtime name/path resolution alive and force the runtime, tools, and
evolution to rediscover semantics that the checker already proved.

Rust typestate is a useful precedent for hardening runtime capabilities. The
Embedded Rust Book says Rust's type system can enforce properties at compile time
and typestate encodes state into types:
[Static Guarantees](https://doc.rust-lang.org/stable/embedded-book/static-guarantees/index.html),
[Typestate Programming](https://doc.rust-lang.org/beta/embedded-book/static-guarantees/typestate-programming.html).
Marrow already has a checked IR/data capability model, but runtime host
maintenance and transaction state still rely partly on runtime booleans/depth
counters. That is acceptable for v0.1, but typed capability/state tokens would be
a stronger long-term Rust shape.

Explicit hand-written scans are the nearest positive precedent. FoundationDB
applications and SQLite users often write exact key ranges or index-backed
queries, accepting that performance is a property of chosen key/index design.
Marrow's design fits that family if it continues to make all durable scans
visible in source and all indexes declared facts, and if `explain` becomes a
checked-operation renderer rather than a planner.

## 4. Alternatives Considered

**Alternative A: keep checked IR, checked places, explicit write plans, and source
as access path.** This preserves one semantic source for runtime, tools,
evolution, backup, and future LSP. It makes costs predictable, keeps hidden I/O
out of normal expressions, lets the checker reject missing presence/index facts,
and makes old prototype paths easy to delete by absence scans. This is the current
approach.

**Alternative B: add a SQL-style cost optimizer under Marrow.** This would help
only if Marrow had declarative relational queries with multiple equivalent plans.
Today the source names store roots, declared indexes, fields, and loops. A hidden
optimizer would either do nothing meaningful, or it would violate the language
promise that the written access path determines operations. It would also require
statistics, cardinality estimation, plan invalidation, and explain tooling. That
is too much foundation for v0.1 and cuts against the accepted ADR edits.

**Alternative C: add a separate query language for durable data.** This could be
valuable later as a source-visible feature for reports or tooling. It should not
replace the runtime foundation. If added, it needs its own checked facts,
operation model, indexes, and explain output, and its costs must be explicit in
source docs.

**Alternative D: execute syntax bodies or a generic AST at runtime.** This is a
bad foundation. It duplicates checker semantics in runtime, keeps unresolved
names and fallback resolution alive, and gives tools/evolution/backup another
semantic layer to drift against. Rhai-style AST execution is excellent for
embedded scripting, but Marrow's durable catalog identity makes checked IR the
right boundary.

**Alternative E: use ORM/GraphQL-style resolvers or lazy loading.** This would
make durable access feel ergonomic at first and then produce hidden I/O, N+1
patterns, loader caches, and resolver-local semantics. It is a better fit for a
future API adapter than for the language runtime. Marrow's current fail-closed
durable materialization is stronger.

**Alternative F: use Datalog-like derivation and automatic index/rule planning.**
This could be a strong future analysis or query subsystem if Marrow needs
recursive declarative facts over program/data snapshots. It should not replace the
imperative durable write planner. Datalog engines own a different semantic model:
relations, rules, fixed points, rule plans, auto-indexes, and magic-set
transformations.

**Alternative G: raw hand-written storage scans only, bypassing checked facts.**
This is fast to prototype and bad long-term. It collapses public paths, physical
keys, schema identity, and source spelling into one surface. Lane 8 correctly
keeps hand-written access paths at the source level while lowering them through
checked facts and catalog IDs.

## 5. Verdict

Verdict: **refine, not reverse**.

Runtime execution from checked facts/IR is the right long-term architecture for
Marrow v0.1. The "source is the plan" claim is strong for Marrow's current
language shape because source code explicitly names stores, indexes, fields,
loops, transactions, writes, and deletes. A hidden cost optimizer would be a weak
foundation here: it would add a second planning semantics without giving SQL-like
benefits, because Marrow is not yet a declarative set-query language with many
equivalent relational plans.

The audited implementation is good enough to build on, provided two refinements
happen before Marrow treats the runtime/tooling surface as complete:

1. Make checked operation plans/facts first-class enough for `explain`, trace,
   dry-run, backup/restore, and LSP/tooling to render them without scraping raw
   runtime paths or storage bytes.
2. Let Lane 11 harden the remaining internal raw-address and string-member-path
   helpers so they cannot become a public compatibility bridge.

Do not reverse into syntax-body interpretation, raw saved-path resolution, ORM-like
lazy loaders, GraphQL resolver planning, or a statistics-based optimizer below the
language. If Marrow later needs query optimization, add it as a source-visible,
checked query feature with its own explicit contract, not as a hidden runtime
rewrite of ordinary source paths.

## 6. Long-Term Risks

**Duplicate semantics.** The biggest remaining duplicate-semantics risk is not
Lane 8 runtime entry execution; it is downstream tooling. Lane 10 explicitly names
raw data, explain, serve, backup, restore, LSP, debug/admin, and trace/dry-run
surfaces as suspects
(`docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:126`). If those adapters
parse raw saved paths, decode backend bytes, or classify schema locally, they will
recreate the prototype model even though runtime is clean.

**Dead prototype paths.** Runtime production paths for syntax bodies, raw entry
strings, `merge`, `lock`, and saved `inout` look removed or rejected. The parser
still carries reserved/round-trip syntax for `merge` and `lock` because the
language reserves them (`docs/language/syntax.md:371`), and Lane 11 flags this as
a prototype-rejection hardening concern
(`docs/roadmap/lanes/lane-11-rust-hardening.md:135`). That is not a Lane 8
runtime blocker, but it is still a surface that can confuse tests and docs if not
kept rejection-only.

**Weak Rust shape.** Runtime is split into more focused modules than the old
prototype, but some helpers still encode state as runtime booleans and vectors:
maintenance is a host boolean (`crates/marrow-run/src/host.rs:122`), transaction
state is depth/check vectors (`crates/marrow-run/src/env.rs:43`), and
`DataAddress::raw` is a permissive constructor
(`crates/marrow-run/src/store.rs:17`). This is acceptable for v0.1, but the
long-term Rust shape should move toward typed capability/state tokens for
maintenance/evolution/restore contexts and narrower constructors for already
checked addresses.

**Hidden compatibility glue.** `DataAddress::raw` and `member_path_segments` are
the pressure points. They are currently private and used against checked facts,
but their names and shapes are close to raw path compatibility. If Lane 10 or
Lane 11 imports or widens them, Marrow will have recreated a raw-path bridge under
a checked veneer.

**Unidiomatic language/database design.** "No optimizer" is idiomatic only if
Marrow keeps durable access explicit and explainable. It would become unidiomatic
if Marrow grows report/query workloads where users must write manual nested loops
for complex joins but receive no query language, no plan diagnostics, and no
index suggestions. The correct mitigation is source-visible query/index tooling,
not a hidden optimizer in ordinary runtime execution.

**Transaction and side-effect modeling.** The current transaction model matches
SQLite/local embedded expectations and the ADR: many read snapshots, one writer,
savepoint-like nesting, rollback on escaping errors. It will not scale unchanged
to distributed or multi-writer operation. FoundationDB shows that optimistic
transactions need retry loops, conflict ranges, and idempotence for ambiguous
commit results. Marrow's ADR already defers distribution; future work must keep
that explicit and typed.

**Minimal-plan auditability.** The language promise says the planner performs no
more operations than the source requires, with only proof-preserving elision. The
implementation has explicit plan steps, but the project needs a durable way to
render and test operation counts/classes at the checked-plan level. Without that,
the guarantee is documented but hard for tools, reviewers, and users to verify.

**Maintenance mode.** Maintenance is correctly a host capability, not source
syntax. The risk is product-surface leakage: `--maintenance`, repair, restore,
debug data, and evolution apply must stay explicit and must not become a general
way to bypass normal semantics. Lane 10 names this exact surface
(`docs/roadmap/lanes/lane-10-tooling-backup-protocols.md:136`).

## 7. Concrete Follow-Up Recommendations

1. **Block hidden optimizer work below the language.** Treat the historical ADR
   edits as architecturally correct: no cost/statistics-based runtime optimizer
   under ordinary Marrow code. Any query optimization proposal must be
   source-visible and checked.

2. **Keep checked operation facts behind debug/admin tooling.** Do not let
   `marrow debug explain`, trace, dry-run, LSP, backup, or serve infer runtime
   operations from raw paths. They should render checked operation facts:
   root/index/field reads, traversal scans, write plan steps, index maintenance,
   transaction grouping, and bounded/materialization errors.

3. **Preserve Lane 10's integrated boundary.** Raw/path protocol cleanup is
   foundation-level because a raw production protocol can undo Lane 8's runtime
   cleanup from the outside.

4. **Narrow or rename `DataAddress::raw`.** Keep it private and evolution-only, or
   replace it with constructors that encode their proof source, such as
   `from_checked_catalog_segments` or an evolution staging address type. Add an
   absence scan proving no CLI/protocol/tool path can call it.

5. **Replace string member paths inside runtime address construction where
   feasible.** `member_path_segments` should either consume checked member refs or
   be quarantined behind a helper whose only inputs come from checked executable
   facts. The goal is not cosmetic; it prevents raw path shape from surviving as
   an internal habit.

6. **Move maintenance/evolution/restore capabilities toward typed Rust state.**
   For v0.1 the host boolean is acceptable, but follow-up hardening should use
   narrow capability tokens or typestate-like wrappers so maintenance-only paths
   cannot be reached by ordinary runtime code by accident.

7. **Turn the minimal-plan guarantee into tests over operation classes.** Add
   source-driven fixtures that assert exact write-plan classes for representative
   root assignment, field assignment, required-field delete, index rewrite, unique
   conflict, transaction grouping, and traversal guard cases. This should not
   assert physical byte keys, only checked operation facts.

8. **Keep Datalog/query ideas as future source features, not runtime patches.** If
   Marrow needs joins, recursive queries, or report-style workloads, propose a
   language-level query surface with declared indexes, checked cost/explain, and
   explicit result materialization rules.

9. **Scrub parser/docs/tests for rejected constructs after Lane 10.** `lock`,
   `merge`, saved `inout`, production `~` roots, raw saved paths, `@id`, source
   ordinal enum meaning, and raw fallback lookup should survive only as reserved
   syntax/rejection tests or debug/admin documentation with an owning verdict.

10. **Preserve the current checked-runtime direction through Lane 11.** Lane 11
    should treat syntax-body execution, raw entry strings, runtime-local saved-path
    classifiers, enum ordinal storage, unbounded durable materialization, and
    compatibility branches as regressions, not cleanup opportunities.

Bottom line: Lane 8's runtime architecture is a strong foundation for Marrow v0.1.
The right next move is not a different planner model. It is to make the checked
operation model more visible to tools, finish raw protocol cleanup, and harden the
few internal helpers whose shape still resembles the prototype.
