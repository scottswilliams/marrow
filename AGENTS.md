# Marrow Contributor Instructions

Marrow is a statically typed language in which hierarchical paths may be local
or durable. Durable paths are ordinary language places. The compiler owns their
types and stable semantic identities and is being designed to keep storage
encoding, public URI representation, path authority, and evolution coherent.

Storage engines are substrates behind a conformance boundary, not Marrow's
product or semantic authority. Embedded and served programs must obey the same
language and transaction semantics.

## Documentation Authority

Use one authority for each question:

| Question | Authority |
|---|---|
| Purpose and long-term direction | `docs/vision.md` |
| Current implementation status | `docs/status.md` and the code |
| Current `.mw` behavior | `docs/language/` |
| Approved unimplemented target | an explicitly accepted contract in `docs/design/` |
| Current code structure | `docs/implementation/` |
| Current storage implementation contract | `docs/backend-contract.md` |

Plans, reports, old ADRs, and decision logs are not specifications. A language
or durable-state target requires explicit approval from the human project
maintainer responsible for Marrow's direction, followed by a contract in
`docs/design/`, before implementation. Agents, lane owners, reviewers, tests,
and prototypes cannot grant that approval. When behavior becomes current,
update the canonical reference and delete the target contract. A later decision
record may explain rationale but cannot become a second source of law.

The current `surface` machinery, operation tags, generated CRUD-style
operations, and user-facing cost model are legacy. The tree-walking interpreter,
catalog layout, and redb backend are current but non-constitutional. Do not
promote either group into architectural requirements or expand a legacy
mechanism without an accepted replacement contract.

## Product And Design Direction

These constraints guide proposals. Unimplemented details are not current law
until an accepted target contract names them.

- Local and durable values share one typed tree model; `^` marks a durable
  place.
- Reading, assignment, deletion, and iteration are ordinary language
  operations, not a separate query language.
- One semantic path graph owns meaning. Stable schema path identities, entry
  identities, store UIDs, source spelling, concrete keyed addresses, URI text,
  authority regions, graph-version evolution, and physical keys remain distinct.
- Durable-root ownership and authority must be explicit in the future design.
  Current direct syntax does not settle that design or imply ambient global
  write authority.
- Authentication establishes identity. Authorization belongs to compiler-owned
  path facts and one total runtime enforcement seam.
- A local owner receives an explicit root capability; local execution does not
  bypass the security model.
- Source compilation is reproducible without a user store. Read-only store
  admission is separate. It returns an already-active verdict when no transition
  is needed, an exact state-bound witness for every permitted transition
  (including binding-only transitions), or a rejection. Activation consumes an
  exact valid witness; it atomically commits data changes, accepted schema state,
  and the store's active-image binding, then issues a receipt only after commit.
  The immutable image artifact remains outside the store transaction. These
  mechanisms supplement rather than replace human review.
- MUMPS is design evidence, not a compatibility target. Do not inherit syntax
  or behavior merely because it is historical.
- Marrow is not a database-engine, web-framework, or general-purpose-language
  competition.

Do not introduce a user-facing query, planner, scan, cost, or explain model; a
CRUD operation taxonomy; or a second application data model. Internal storage
traversal and empirical performance measurements remain legitimate
implementation concerns and must be named precisely.

## Documentation Style

Write public documentation in a neutral technical-reference voice:

- define the mechanism before describing its benefits;
- use complete checked examples;
- use present tense only for implemented behavior;
- label direction and research explicitly;
- state limitations beside the claims they qualify;
- avoid superlatives, competitor dismissals, and agent-centric positioning;
- use “checked,” “enforced,” “witnessed,” or “conformance-tested” rather than
  “proven” unless a formal result is published;
- give one concept one term and one prose owner;
- delete obsolete material rather than appending history or migration notes.

Agents are contributors and consumers of structured compiler facts, not
Marrow's target category. Documentation must remain readable by developers,
language implementers, storage engineers, and operators under pressure.

Every high-level code change updates its `docs/implementation/` page in place.
Implementation pages describe actual code, including legacy mechanisms while
they exist; they do not turn current topology into future law.

## Working Rules

1. Read the canonical reference before changing syntax, types, durable paths,
   transactions, identity, evolution, authority, or user terminology.
2. State assumptions and tradeoffs before implementation. Prefer a smaller
   semantic core and explicit integration boundary.
3. Start behavior changes with a failing production-pipeline test.
4. Make the minimum coherent change and delete the replaced path; do not add a
   fallback, mode flag, or compatibility copy for green tests.
5. Preserve unrelated user changes in dirty worktrees.
6. Verify with fresh output before reporting completion.

## Rust Architecture

Write Marrow as a language/compiler/runtime system: typed, direct, and organized
around one durable invariant at a time.

**Typed identity.** Use newtypes or small enums for semantic identity, state,
and diagnostic kinds. Store typed values and render strings at boundaries. Do
not compare formatted paths, names, diagnostic prose, or protocol text to
recover meaning.

**One semantic owner.** Parse syntax once, classify each language concept once,
and thread typed facts through later stages. Do not duplicate definitions of
durable paths, builtins, identity, values, evolution verdicts, or diagnostics
across parser, checker, runtime, tools, and tests.

**Compiler facts.** The checker owns source resolution and semantic path facts.
URI, authority, evolution, and editor information must derive from those facts;
no adapter or renderer re-parses source strings.

**Runtime boundary.** The runtime consumes checked facts and fails closed with
typed errors. Every logical tree access must converge on one typed
data-session/path kernel as that architecture is introduced. Physical
substrate recovery remains a separately typed, private store component beneath
the kernel; it cannot return a store to service until complete program-image,
schema, and store validation plus a fresh already-active admission verdict
succeed.

**Storage boundary.** The store owns bytes, ordered operations, transactions,
durability, and recovery. It does not own `.mw` semantics, public paths,
authorization, or evolution meaning. Backend-specific names and formats stay
out of application source.

**Diagnostics.** Emit a typed code and typed payload. One render owner produces
prose. Tests assert codes, spans, facts, and values rather than sentence
fragments.

**Fallibility.** Return typed errors on fallible paths. Reserve `panic!`,
`expect`, and `unreachable!` for states made impossible by a narrow type or
constructor invariant.

**API shape.** `pub` requires a real cross-crate caller. Keep fields private,
enforce invariants in constructors, expose borrowing accessors, and prefer typed
state over behavior-selecting booleans.

**Code shape.** Split broad dispatchers by invariant. Prefer focused modules and
exhaustive matches over comment-heavy branches, generic helpers, or god crates.
Page or stream potentially unbounded user data.

**Comments.** Explain durable rationale, representation, performance, or
soundness. Do not narrate edits, cite roadmaps, or restate control flow.

No `unsafe`. Do not add dependencies without explicit approval in the current
canonical work plan and a license-compatibility review. Repository source
remains Apache-2.0.

## Testing

- Exercise the production pipeline rather than a hand-built semantic replica.
- Keep tests beside the behavior they establish and split suites by invariant.
- Storage, identity, write, evolution, and authorization changes require
  adversarial sibling cases, compatibility fixtures, and recovery coverage.
- A new invariant ships with an enforcement artifact: a type boundary,
  visibility restriction, absence test, conformance test, or generated drift
  gate.
- Documentation examples should parse and check; generated references require
  byte-for-byte drift tests.

## Worktrees, Builds, And Integration

Use an isolated worktree for substantial or multi-file changes. Follow the
machine-level `AGENTS.md` for the mandatory external `CARGO_TARGET_DIR`; never
create build output inside this repository.

Documentation-only changes require fresh link, anchor, terminology, snippet,
formatting, and generated-drift checks. Every code integration requires the
smallest affected crate or fixture, workspace build, full workspace tests,
`fmt --check`, `clippy -D warnings`, and zero `unsafe`. Broad checks sharing a
target directory run serially.

Do not merge feature work without independent semantic-accuracy and code-shape
review. Rebase on live main, rerun the relevant gates, integrate deliberately,
then retire the worktree and its build output together.

`marrow-lsp` is downstream. Add snapshot-versioned semantic facts to Marrow
first; editor, debugger, automation, and optional machine transports consume
those facts without inventing language behavior.
