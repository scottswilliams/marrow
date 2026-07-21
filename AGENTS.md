# Marrow contributor instructions

Marrow is a general-purpose statically typed compiled language whose distinctive
capability is direct interaction with durable hierarchical data. Pure programs
need no store. Durable programs use typed language places rather than a query,
ORM, repository, or raw key API.

Marrow is not an experimental or hobby language. It is designed to be built with
production at scale in mind: judge architecture, representations, and semantics
against what a widely used mainstream language and its largest deployments
require, never against what a prototype or demo can get away with, and let no
design assume smallness of programs, data, teams, or deployment lifetime. Current
bounds and capability gaps are honest, evidence-widened waypoints, not the bar;
the beta's personal-application release criterion is a milestone on this path,
not the ambition. This raises the design bar without licensing maturity claims —
the documentation standard's evidence rules still govern what may be called
production-ready.

The current production path runs source through the parser and the storeless
checker/compiler to a reproducible immutable program image, an independent
verifier that is the image's only decoder, a bytecode VM, and a typed path
kernel over a private ordered-byte engine. These owners are present but early:
their admitted language subset is narrow, and their durable identity, lifecycle,
and authority attenuation are stubs with named refounding points. The capability
trough is explicit — a feature is absent until its refounding lane lands it. Do
not describe a stub or a future capability as implemented today, and do not turn
the current topology into a compatibility requirement.

## Documentation authority

Use one owner for each question:

| Question | Authority |
|---|---|
| Purpose and product boundaries | `docs/vision.md` |
| Current/legacy/future state | `docs/status.md` and reachable code |
| Current `.mw` behavior | `docs/language/` |
| Current tools and operations | `docs/tools/` and `docs/operations/` |
| Current code structure | `docs/implementation/` |
| Unimplemented direction | `docs/future/` |

Current behavior is carried by concise reference pages, maintainable code, and
production-path tests together. There is no parallel design-specification tier,
ADR archive, target-contract queue, or agent-owned authority.
Future pages state goals, constraints, evidence targets, and deferrals; they do
not publish unchecked proposed syntax or exact formats.

A semantic lane updates code, tests, and the current reference together. When a
new implementation makes a future statement current, move the behavior into the
reference and remove or narrow the future statement. Delete obsolete syntax,
commands, fixtures, diagnostics, dependencies, and prose in the same lane. Git
history is the archive.

The capability trough is deliberate: after the replacement storeless compiler
becomes public, prototype durable documentation must disappear with its code,
even while durable execution is temporarily unavailable. Do not preserve a
second production path to keep old docs or tests green.

## Product direction

- The language must be useful for ordinary storeless programs. Algebraic data
  types, real parametric functions and types, generic collections, modules,
  packages, formatting, tests, and editor support are foundations; closures are
  deferred until a maintained program is materially worse without them.
- A light package system uses Git/path locators, exact pinned edges (no lock
  file), a separate stable-identity ledger, a verified offline cache, and no
  dependency build scripts or ambient initialization.
- Direct durable reads, writes, presence, explicit transactions, and bounded
  ordered traversal are language operations. There is no user query planner,
  `EXPLAIN`, ORM, generated CRUD family, or source-level cost model.
- The compiler describes access demand; it never grants what it infers. Runtime
  access must intersect verified demand, exact candidate acceptance, a separate
  maximum ceiling, and invocation attenuation at one path kernel.
- Source spelling, stable declaration identity, package lineage/snapshot,
  concrete keyed address, store identity, public URI, authority region, and
  physical key remain distinct typed concepts.
- Storage engines are private transactional byte substrates. Marrow competes on
  language/compiler integration, not engine choice or database benchmarks.
- MUMPS is product evidence and inspiration, not a syntax, compatibility, or
  implementation constitution.
- Local terminal and desktop applications come before served execution. Public
  HTTP, principals/policy, replication, broad online evolution, and
  institutional readiness remain future until separately evidenced.

The current `surface`, server/client generator, CRUD operation tags, and cost
projection are rejected product families. The current resource/schema system,
managed indexes, `nextId`, permissive write/transaction behavior, store-owned
catalog, interpreter, project session, mixed JSON crate, global diagnostic
registry, and redb recovery wrapper are transitional. Remove rather than expand
them as their replacements land.

## Working rules

1. Read the canonical current reference before changing syntax, types, paths,
   transactions, identity, evolution, authority, or terminology.
2. State assumptions and tradeoffs. If an implementation reaches a
   consequential user-visible fork not settled by the approved direction and
   evidence, ask the maintainer before building that fork; do not create a
   standing approval queue.
3. Begin behavior changes with a failing production-pipeline test and observe
   the intended failure.
4. Implement one coherent vertical invariant through parser, semantic owner,
   executable form, runtime, tools, and reference as applicable.
5. Delete the displaced family. Do not add a fallback, mode flag, compatibility
   copy, test-only production constructor, or duplicate semantic model.
6. Preserve unrelated user changes in dirty worktrees.
7. Verify from fresh output before reporting completion.

## Rust architecture

Write Marrow like a language, compiler, and durable runtime maintained for
years: typed, direct, bounded, and organized around one semantic owner.

**Typed identity and state.** Use newtypes and small enums for IDs, provenance,
operations, diagnostics, lifecycle states, and capabilities. Render strings at
boundaries. Do not recover meaning by comparing names, paths, prose, or protocol
text.

**One owner.** Parse syntax once and classify each concept once. Do not duplicate
builtins, values, key eligibility/order, saved paths, effects, evolution
verdicts, diagnostics, or runtime facts across compiler, VM, kernel, tools, and
tests.

**Current versus target compiler facts.** The current checker owns source
resolution and type/effect facts. The replacement compiler must own a partial,
revisioned `AnalysisSnapshot` and a clean storeless image path. LSP and renderers
consume those facts; they do not parse source strings or diagnostic messages.

**Runtime boundary.** Only independently validated executable artifacts enter
the target VM. Every durable instruction names a validated typed effect site;
application code never receives a database connection, raw physical key, engine
handle, ceiling owner, maintenance grant, or recovery handle.

**Storage boundary.** A raw engine owns ordered bytes, snapshots, consuming
transactions, sync, and native recovery. Language representation, typed paths,
authority, lifecycle, logical integrity, and backup/restore belong above it.
Engine-specific names and formats stay out of `.mw` source and public APIs.

**Diagnostics.** A typed variant couples stable code, payload, locations, and
severity. One renderer produces prose. Semantic tests assert the variant, code,
payload, span, fact, value, or effect—not sentence fragments.

**API and code shape.** `pub` needs a real cross-crate caller. Keep fields
private, enforce invariants in constructors, prefer consuming/typed-state APIs,
split broad dispatchers, and page or stream potentially unbounded user data.
Comments explain durable rationale, representation, cost, or soundness, not
history or control flow.

No `unsafe`. A new dependency needs explicit maintainer approval, license
review, and a concrete boundary that the standard library or an existing
dependency cannot satisfy. Repository source remains Apache-2.0.

## Testing and evidence

- Exercise source through the production parser/checker or compiler and the
  production runtime path; do not hand-build semantic replicas.
- Keep tests beside the invariant. Identity, keys, types, effects, writes,
  transactions, storage, admission, activation, backup, and recovery require
  adversarial sibling cases.
- Every invariant ships its enforcement artifact: type/visibility boundary,
  absence test, conformance law, or generated drift gate.
- Complete current `mw` examples must check. Future pages contain no `mw`
  fences. Generated references and bindings require byte-exact drift tests.
- Performance and durability claims name workload, platform, toolchain,
  settings, limits, raw evidence, and regression policy. Do not call a behavior
  proven, safe, scalable, portable, or institution-ready without the
  corresponding evidence.
- The compiler-dev audit mode is not executable on the beta line: `marrow check`
  is the minimal surface — it reports diagnostics and each export's durable access
  demand — and carries no `--compiler-dev` audit mode. Checker, inference, analysis, hover, and other
  semantic-tooling changes instead rely on the production-path test tiers above —
  source exercised through the production parser/checker or compiler and the
  production runtime — with adversarial sibling cases beside the invariant. A
  well-formed construct outside the admitted subset carries the typed
  `check.unsupported` diagnostic at its span, and that typed unsupported outcome
  is the recorded evidence for an absent capability; do not substitute a
  suppressed audit warning or another baseline for it.

## Worktrees, builds, and integration

Use an isolated worktree for substantial or multi-file changes. Follow the
machine-level `AGENTS.md` for the mandatory external `CARGO_TARGET_DIR`; never
create build output in this repository. Broad checks sharing one target run
serially.

Documentation-only changes require fresh inventory, link, anchor, terminology,
snippet, and generated-drift checks. Code integrations require focused tests,
workspace build/tests, `fmt --check`, `clippy -D warnings`, zero `unsafe`, and
dependency/absence scans as applicable.

Substantial changes receive independent soundness and code-shape/reference
review. Rebase on live main immediately before integration, rerun gates, push,
then retire the worktree and its build output together.

`marrow-lsp` is downstream. Add canonical snapshot-versioned facts in Marrow
first; editor, debugger, automation, and optional machine transports must not
invent language or store behavior downstream.
