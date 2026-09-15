# Marrow contributor instructions

[Vision](docs/vision.md) defines Marrow's purpose and product boundaries. Judge designs
against a general-purpose language's long-lived, large deployments; do not build
assumptions of small programs, data, teams, or deployment lifetime into the architecture.
Current bounds require evidence before widening, and ambition does not establish maturity
or readiness. [Status](docs/status.md) records implemented behavior and limitations, and
the [implementation map](docs/implementation/README.md) describes current owners; neither
a current topology nor a future mechanism is a compatibility promise. This file owns the
architecture rules; [CONTRIBUTING.md](CONTRIBUTING.md) owns the workflow: the checks, the
build directory, review, and filing an issue.

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

Concise reference pages, maintainable code, and production-path tests carry current
behavior together. There is no parallel design-specification tier, ADR archive,
target-contract queue, or agent-owned authority; a genuine product choice is discussed when
it becomes necessary, and git history is the archive. Future pages state goals, constraints,
evidence targets, and deferrals without publishing unchecked syntax or exact formats. A
semantic lane updates code, tests, and the current reference together: when an implementation
makes a future statement current, move the behavior into the reference and narrow or remove
the future statement, deleting obsolete syntax, commands, fixtures, diagnostics, dependencies,
and prose in the same lane. Compilation and test speed constrain representation, crate
boundaries and test design; [speed](docs/implementation/speed.md) owns those rules and the
three clocks every broad gate records, and soundness wins a conflict.

## Working rules

1. Read the canonical current reference before changing syntax, types, paths,
   transactions, identity, evolution, authority, or terminology.
2. State assumptions and tradeoffs, and ask the maintainer before building a
   consequential user-visible fork the direction and evidence do not settle.
3. Begin behavior changes with a failing production-pipeline test, observe the intended
   failure, and exercise source through the production compiler and runtime rather than
   a hand-built semantic replica.
4. Implement one coherent vertical invariant through parser, semantic owner,
   executable form, runtime, tools, and reference as applicable.
5. Delete the displaced family. Do not add a fallback, mode flag, compatibility copy,
   test-only production constructor, or duplicate semantic model.

## Rust architecture

Write Marrow like a language, compiler, and durable runtime maintained for years:
typed, direct, bounded, and organized around one semantic owner.

**Typed identity.** Use newtypes and small enums for IDs, provenance, operations,
diagnostics, lifecycle states, and capabilities; render strings at boundaries. Source
spelling, declaration identity, package lineage and snapshot, keyed address, store
identity, public URI, authority region, and physical key are separate concepts: do not
recover one from another's spelling, or meaning by comparing names, paths or prose.

**One owner.** Parse syntax once and classify each concept once. Do not duplicate builtins,
values, key eligibility/order, saved paths, effects, evolution verdicts, diagnostics, or
runtime facts across compiler, VM, kernel, tools, and tests. `marrow-compile` owns source
resolution, type/effect facts, the revisioned `AnalysisSnapshot`, and storeless image
compilation; LSP and renderers consume those facts rather than rebuilding them from strings.

**Runtime boundary.** Only independently validated executable artifacts enter the VM, and
every durable instruction names a validated typed effect site. Application code never
receives a database connection, raw physical key, engine handle, ceiling owner, maintenance
grant, or recovery handle. The compiler describes access demand and grants nothing; durable
access passes one path kernel under verified demand, exact candidate acceptance, a separate
ceiling and invocation attenuation.

**Storage boundary.** A raw engine owns ordered bytes, snapshots, consuming transactions,
sync, and native recovery; language representation, typed paths, authority, lifecycle,
logical integrity, and backup/restore belong above it, and engine-specific names stay out of
`.mw` source and public APIs. A representation change preserves a bounded path to full
logical backup and fresh restore.

**Diagnostics and code shape.** A typed diagnostic variant couples stable code, payload,
locations, and severity, and one renderer produces prose; semantic tests assert the variant,
code, payload, span, fact, value, or effect, not sentence fragments. `pub` needs a real
cross-crate caller: keep fields private, enforce invariants in constructors, prefer
consuming/typed-state APIs, split broad dispatchers, and page or stream potentially unbounded
user data. Comments explain durable rationale, representation, cost, or soundness, not history
or control flow. No `unsafe`; a new dependency needs maintainer approval, license review, and a
boundary the standard library or an existing dependency cannot satisfy, and repository source
remains Apache-2.0.

## Testing and evidence

- Keep tests beside the invariant. Identity, keys, types, effects, writes, transactions,
  storage, admission, activation, backup, and recovery require adversarial siblings.
- Every invariant ships its enforcement artifact: a type or visibility boundary, absence
  test, conformance law, or generated drift gate.
- Complete current `mw` examples must check; future pages contain no `mw` fences, and
  generated references and bindings require byte-exact drift tests.
- Performance and durability claims name workload, platform, toolchain, settings, limits,
  raw evidence, and regression policy. Do not call a behavior proven, safe, scalable,
  portable, or institution-ready without that evidence.
- Keep unsupported constructs and tool limitations explicit in typed diagnostics and
  the current reference.

## Development branch and release authority

`main` is the sole development and integration branch. New lanes derive from current
`origin/main`; after their required gates and reviews they rebase onto current `main`,
fast-forward `main`, push, fetch, and verify `origin/main`. It carries unreleased
development source: a push is not a versioned release, compatibility or support promise,
production-readiness claim, or safety-readiness claim. Tags, releases, release assets,
public candidate references, support declarations and visibility changes require the
explicit release gate, which ordinary lane authority does not grant; default-branch changes
are forbidden even there. The verdict remains **NOT READY FOR SAFETY-CRITICAL USE**.

Use an isolated worktree for substantial or multi-file changes, and never share a compiling
lane's build directory with another lane. `marrow-lsp` is downstream: add canonical
snapshot-versioned facts in Marrow first; editors, debuggers, automation, and optional
machine transports must not invent language or store behavior downstream.
