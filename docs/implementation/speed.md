# Compilation and test speed

Compilation and test execution speed is an architectural constraint on
Marrow's design. It is settled when a representation, a crate boundary, or a
test's evidence layer is chosen, and a design that is correct but slow is
unfinished.

The constraint governs cost only. It never licenses an unsound shortcut, a
skipped check, or a weakened bound; where speed and soundness conflict,
soundness wins and the cost is recorded as a finding. No measurement appears
here. [Project status](../status.md#measurements) records what each
clock has measured.

## Three clocks

Work is ranked against three intervals, in this order:

1. Marrow compile time over `.mw` programs: the interval between changing a
   `.mw` source file and obtaining a diagnostic, a formatted file, an image, or
   a test result. It ranks first because a Marrow program's author pays it on
   every edit, and because it belongs to the product.
2. Workspace test wall time: the interval a contributor pays to run the
   battery, many times a day.
3. Rust clean and incremental build time: the interval that precedes every
   experiment on the implementation itself.

A broad check records all three, so a regression is visible where it occurs.
A change that materially increases one names the cause. An unexplained
increase is a finding. A clock with no recorded baseline cannot show a
regression, so establishing a missing baseline is itself scheduled work.

## The rules

A change is reviewed against six rules.

1. Cheap to build and traverse beats structurally elegant. Prefer arenas and
   interned symbols to pointer-chasing graphs, indices to reference cycles,
   and flat slices to nested allocations.
2. One pass carries the fact. Do not introduce whole-program analysis, a
   global fixpoint, or re-derivation across phases where a single forward
   pass suffices.
3. A phase reuses what an owner already computed. Re-parsing, re-resolving,
   or re-walking an upstream owner's result is a defect even when the result
   is correct. The [ownership rule](README.md#ownership-rule) states the same
   boundary from the correctness side; this is its cost side.
4. Crates stay small behind narrow public seams, so an edit's incremental
   rebuild stays local to the owner it touches.
5. A heavyweight dependency needs a named reason. Proc-macro-heavy and
   monomorphization-heavy dependencies are paid for at every build, so each
   states a need beyond the approval and license review every dependency
   takes. Where a call is not hot, prefer an enum or a `dyn` seam to generics
   that multiply generated code.
6. A test takes the cheapest layer that proves its invariant. Prefer
   source-driven fixtures through the parser, checker, or compiler to
   integration binaries that spawn a process; keep CLI tests thin; share
   expensive setup across the cases that need it; and keep a single test's
   wall time proportionate to what it proves. The [evidence
   layers](testing.md) name what each layer buys.

## Slow tests are opt-in

A test whose cost is out of proportion to the rest of the battery is marked
`#[ignore]` with a reason that states that cost, and is run explicitly with
`--ignored`. The reason is where the cost is justified, as in
`crates/marrow/tests/durable_transactions.rs`:

```text
#[ignore = "burns the whole 1<<26 instruction budget (private VM const, no override) — ~1.3s debug; E07-gating evidence, run with --ignored"]
```

The same treatment covers the measurement harnesses, whose output is a
recorded number and never an assertion, and the tests a sandboxed command
environment cannot run because they spawn a process or bind a socket. Adding
a slow test to the default battery is a design review item.
