# Marrow contributor instructions

Marrow's purpose and product boundaries are defined in [Vision](docs/vision.md).
Judge designs against a general-purpose language's long-lived, large deployments;
do not build assumptions of small programs, data, teams, or deployment lifetime
into the architecture. Current bounds require evidence before widening, and
ambition does not establish maturity or readiness.

[Status](docs/status.md) records implemented behavior and limitations; the
[implementation map](docs/implementation/README.md) describes current owners.
Do not turn a current topology or a future mechanism into a compatibility promise.

## Compilation and test speed

Compilation and test speed constrain representation, crate boundaries and test
design. [Compilation and test speed](docs/implementation/speed.md) owns those
rules. Soundness wins a conflict; never skip a gate or weaken a bound to improve
a measurement.

Every broad gate records three clocks in user-impact order: Marrow compilation
over `.mw` programs, workspace test wall time, then Rust clean and incremental
build time. A material regression names its cause; an unexplained regression
remains a finding. Preserve baselines and raw evidence. Give each figure its
revision, workload, platform and method; it establishes nothing about other
programs or machines.

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

The [durable reference](docs/language/durable-places.md) owns current place,
presence and clearing rules. [Future direction](docs/future/README.md) owns
unimplemented goals and deferrals. Contributor instructions do not create a
second feature plan or prescribe package, image, authority or deployment formats.

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

**Distinct identities.** Source spelling, stable declaration identity, package
lineage and snapshot, concrete keyed address, store identity, public URI,
authority region, and physical key are separate concepts. Do not recover one
from another's rendered spelling.

**Compiler facts.** `marrow-compile` owns source resolution, type/effect facts,
the immutable, revisioned `AnalysisSnapshot`, and storeless image compilation. LSP
and renderers consume those facts; they must not reconstruct them from source
strings or diagnostic messages.

**Runtime boundary.** Only independently validated executable artifacts enter
the VM. Every durable instruction names a validated typed effect site;
application code never receives a database connection, raw physical key, engine
handle, ceiling owner, maintenance grant, or recovery handle.
The compiler describes access demand and grants nothing. Durable access passes
one path kernel under verified demand, exact candidate acceptance, a separate
maximum ceiling and invocation attenuation.

**Storage boundary.** A raw engine owns ordered bytes, snapshots, consuming
transactions, sync, and native recovery. Language representation, typed paths,
authority, lifecycle, logical integrity, and backup/restore belong above it.
Engine-specific names and formats stay out of `.mw` source and public APIs.
Representation changes must preserve a bounded path to full logical backup and
fresh restore; this requirement does not claim those future tools exist.

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
- Keep unsupported constructs and tool limitations explicit in typed diagnostics
  and the current reference. Semantic-tooling changes use the production path.

## Development branch and release authority

`main` is the sole development and integration branch. New lanes derive from
current `origin/main`; after their required gates and reviews they rebase onto
current `main`, fast-forward `main`, push, fetch, and verify `origin/main`. The
former `beta` branch is not an integration target; its retained worktree and
unrelated user changes remain untouched until separately handled.

Public `main` contains unreleased development source. A push to `main` is not a
versioned release, compatibility or support promise, production-readiness claim,
or safety-readiness claim. Tags, releases, release assets, public candidate
references, support declarations and visibility changes require the explicit
release gate; ordinary lane authority grants none of them. Default-branch
changes are forbidden, including at that gate. The verdict remains
**NOT READY FOR SAFETY-CRITICAL USE**.

## Worktrees, builds, and integration

Use an isolated worktree for substantial or multi-file changes. Follow the
machine-level `AGENTS.md` for the mandatory external `CARGO_TARGET_DIR`; spell it
and an explicit `--manifest-path` in every Cargo invocation. Never create build
output in this repository or share a compiling lane's target with another lane.
Broad checks within a lane run serially.

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
