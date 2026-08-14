# Compilation and test speed

Marrow is designed around fast compilation and fast test execution. This is an
architectural constraint applied when a design is chosen, not a maintenance
activity and not a later optimization pass: it governs which representations,
crate boundaries, algorithms, and test structures a change may introduce, and a
design that is correct but slow is unfinished.

The constraint never licenses an unsound shortcut, a skipped gate, or a
weakened bound. Where speed and soundness genuinely conflict, soundness wins
and the cost is recorded as a finding rather than absorbed silently.

This page states the rules the implementation follows. It states no
measurement and makes no claim about how fast Marrow is. [Project
status](../status.md#compilation-and-test-speed) records what has been measured
against each clock and what has no baseline.

## Three clocks

Work is ranked against three wall-clock intervals, in this order:

1. **Marrow compile time over `.mw` programs** — the interval between changing
   a `.mw` source file and obtaining a diagnostic, a formatted file, an image,
   or a test result. It is ranked first because a Marrow program's author pays
   it on every edit, and because it is a property of the product rather than of
   this repository.
2. **Workspace test wall time** — the interval a contributor pays to run the
   battery, many times a day.
3. **Rust clean and incremental build time** — the interval that gates every
   experiment on the implementation itself.

A change that materially increases one of these names the cause. An
unexplained increase is a defect to investigate rather than an accepted new
baseline.

## Representation and algorithm rules

- Prefer representations that are cheap to build and traverse over
  representations that are only structurally elegant: arenas and interned
  symbols over pointer-chasing graphs, indices over reference cycles, and flat
  slices over nested allocations.
- Prefer one pass over several.
- Do not introduce whole-program analysis, a global fixpoint, or re-derivation
  across phases where a single forward pass carries the fact.
- A phase that re-parses, re-resolves, or re-walks what an earlier owner
  already computed is a defect regardless of whether its result is correct.
  The [ownership rule](README.md#ownership-rule) states that boundary from the
  correctness side; this is its cost side.

## Crate and dependency rules

- Keep crates small behind narrow public seams, so an edit's incremental
  rebuild stays local to the owner it touches. The
  [ownership rule](README.md#ownership-rule) already demands the same
  boundaries for semantic reasons.
- A proc-macro-heavy or monomorphization-heavy dependency needs a named reason
  that the standard library and existing dependencies cannot satisfy, in
  addition to the approval and license review every dependency needs.
- Where a call is not hot, prefer a typed enum or a `dyn` seam to generics that
  multiply generated code.

## Test architecture

Test wall time is a design property of the battery, not a cleanup task, so the
[evidence layers](testing.md) are chosen for the cheapest layer that proves the
invariant:

- Prefer source-driven fixtures through the production parser, checker, or
  compiler over integration binaries that spawn a process; keep CLI tests thin
  when the same behavior is reachable below process rendering.
- Share expensive setup across the cases that need it rather than rebuilding it
  per case.
- Avoid link-heavy integration-binary sprawl: an additional integration target
  costs a link at every build.
- Keep a single test's wall time proportionate to what it proves. A test that
  costs minutes states why in its own source.
- Slow measurement that is genuinely necessary — hostile-input measurement,
  soak, and the encoding measurement harnesses — is marked `#[ignore]` with a
  stated reason and run explicitly with `--ignored`, so it is opt-in rather
  than part of the default battery. Adding a slow test to the default battery
  is a design review item.
