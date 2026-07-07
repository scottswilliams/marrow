# Agent Instructions

This repository is Marrow: a lightweight, typed `.mw` language whose durable saved data is part of
the program. Data is scalars or trees; a resource is a typed tree; `^` marks saved data. The same
compiler that checks source governs what is already in the store, so a schema change is checked
against existing data before it can activate. Marrow is its own language and database model, not a
layer on another system.

`docs/language/` is the canonical source for language behavior; `docs/backend-contract.md` is the
store byte contract. Parser, checker, runtime, CLI, language-service adapters, examples, and tests
converge on those. When implementation and a spec page disagree, treat it as implementation work.

Marrow is unreleased. Clean the repository as if the current design had always been here: delete a
stale name, format, or example rather than carry it. Migrate or delete any test or fixture that
depends on outdated behavior so it asserts the v0.1 contract. Keep code free of agentic slop and
documentation sediment.

## Working rules

- Read `docs/language/` before changing `.mw` syntax, typing, resources, builtins, saved-data
  behavior, or user-facing terminology.
- Keep docs in the voice of a real language reference: precise, current, useful.
- Fold surviving content from a deleted file into the smallest durable reference page.
- Work with the changes already in the worktree; keep every edit scoped to the request.
- Prove each change with fresh command output before claiming it done.

## Engineering rules

1. Think first. State assumptions, surface tradeoffs, prefer the simpler approach, ask when a guess
   would risk the work.
2. Write the minimum code that solves the problem. Skip speculative features, single-use
   abstractions, and unrequested configurability.
3. Make surgical changes. Touch only what the request requires; match the surrounding style; let
   every changed line trace to the request.
4. Drive from a failing check. For a behavior change, write or identify the failing test first, then
   make it pass, then refactor under green. Exercise the production pipeline, not a replica.

## Rust style

Write Marrow like a senior language/database engineer. Each rule is checkable and backed by the
project's own precedent; `clippy -D warnings` and `fmt --check` cover the mechanical layer, so this
section is the judgment layer above them.

**Typed identity.** Give a value that carries semantic identity — a key, a catalog id, a type name,
a diagnostic code — a typed newtype or enum, compared by value. Store the typed form and render the
string on demand; never store a formatted string and compare it back. Derive a diagnostic's code from
its typed kind rather than caching both. (Precedent: `marrow-codes::Code`, `marrow-store::CatalogId`,
`facts.rs` id newtypes.)

**Diagnostics.** Emit a typed `Code` plus a typed payload; let `diagnostic_render.rs` own every
sentence. Branch on the code or payload, never on prose. Register every dotted code in `marrow-codes`
and let the generated `docs/error-codes.md` gate catch an unregistered one. (Precedent:
`diagnostic_render.rs`, the `marrow-codes` registry.)

**Compute once, own once.** Derive a fact once and thread the value; do not re-lower or re-derive it
per predicate. One semantic question has one classifier that a cursor query and an all-sites walk both
reuse. A repeated primitive — a hash step, an escape scan, an atomic write, an arg-parse loop — gets
one owner and each caller frames its own use. (Precedent: `classify_key_type`, the store navigation
cursor, `presence/keys.rs`.)

**Dispatch shape.** A match arm that inlines several checks delegates to a named helper. Split a
`mod.rs` by invariant into sibling modules before it crosses ~1k lines. Route structural recursion
through the one shared visitor, not a fresh per-pass match. Keep matches over semantic enums
exhaustive; when a match needs `unreachable!`, narrow the parameter or enum so the case cannot arrive.
(Precedent: `StatementCheck`, the `catalog/` directory, `CheckedBodyVisitor`.)

**Typed state over flags.** Replace a behavior-selecting `bool` or ambiguous `Option`/`None` sentinel
with a typed enum; partition a dispatch by matching the discriminant once. A lowered fact the checker
owns may ride as a `bool` field, but bundle threaded context into a struct rather than growing a
9-argument signature. (Precedent: `RunObservation`, `AcceptedAuthority`, `parse_decl/body.rs`.)

**Fallibility.** On a fallible path, return a typed error and fail closed: reach for a typed error
before `unwrap`, `expect`, or `panic!`, and reserve `unreachable!` for a state the type system has
proven impossible. (Precedent: `marrow-run`, where a checker-proven-unreachable branch returns a
typed `RuntimeError` rather than panicking.)

**API surface.** `pub` means a cross-crate caller exists; otherwise `pub(crate)`. Every exported type
derives `Debug`. Enforce an invariant in the constructor, keep the field private, expose a borrowing
getter, and provide no setter. Take borrowed concrete types (`&str`, `&[T]`, `&Path`, `Option<&T>`)
at boundaries. (Precedent: `SealedStore`, `DiagnosticAnchor`, `marrow-catalog` constructors.)

**One crate, one concern.** A DTO crate serializes and deserializes; crypto, code generation, and
store execution each live in their own purpose-named crate. Engine logic a second front-end could call
belongs one layer below the binary. (Precedent: `marrow-store`, `marrow-codes`, and `marrow-catalog`
each own one concern; extracting `marrow-surface` from `marrow-json` is the planned next step.)

**Layout and tests.** Open every module with a `//!` sentence naming what it owns. Put public-surface
behavior tests under `tests/`; reserve inline `#[cfg(test)]` for a module's private internals; split a
suite by the invariant it pins (~400–500 lines). Page or stream user data; never materialize it
unbounded. A tamper test rebuilds its witness by calling the production owner, not a copied constant.
(Precedent: `marrow-store` conformance suite, the bounded-materialization budgets.)

**Comments.** Explain why — durable rationale, cost, or a soundness invariant — in full sentences. Do
not narrate edits, cite tickets or roadmap steps, or restate the code. Prefer a better name or a
smaller helper over a comment that explains what a branch does.

## Repository shape

- `docs/language/` is the language reference; `docs/backend-contract.md` is the store contract.
- `docs/implementation/` is the code-truth architecture map — a thin, progressive-disclosure guide to
  what each crate and module does and where to read the real code. Start at
  `docs/implementation/README.md`; every crate carries an `AGENTS.md` naming its page.
- Durable language, database, backend, and tooling docs live under `docs/`, nowhere else.
- Keep an example only when it matches `docs/language/` and the implementation; otherwise move its
  coverage into a test.

On any high-level change to the code — a module, type, pass, invariant, or data flow added, removed,
renamed, or reshaped — update its `docs/implementation/` page IN PLACE in the same change: rewrite the
stale lines, delete what no longer holds. A page is a thin map, not a changelog; if an edit makes it
longer without making it truer, cut instead. State a count once, in the table that enumerates it,
never a second time in prose.

## Coding invariants

- Typed IDs and small enums over strings or booleans when a value carries identity or state.
- Import the names a module uses; no crate-root glob preludes, no production `use super::*`.
- One semantic owner per concept — move the boundary rather than duplicate a classifier across parser,
  checker, runtime, tools, or tests.
- A change that establishes an invariant lands, in the same change, the mechanism that makes the old
  pattern unrepresentable or loudly detectable: a visibility change, a typed owner, an absence test,
  or a generated-artifact drift gate. A single-site fix without its sibling sweep is not review-ready.
- Storage behavior stays behind the backend contract; a backend is a store target, not the owner of
  Marrow semantics. Keep saved data inspectable through Marrow tools.
- Native `.mw` is the only default surface. No `unsafe`. Apache-2.0 only.
- `marrow-lsp` is downstream: add the analysis/check/schema API in Marrow first, then adapt the LSP.

## Verification and integration

Run focused checks before broad ones: no-compile checks (`git diff --check`, stale-term and link
scans, formatting of touched docs) first, then the smallest package or test target that proves the
change, then `cargo build --workspace` / `cargo test --workspace` for broad changes. Spell
`CARGO_TARGET_DIR` explicitly at an out-of-tree build root; never run broad gates in parallel against
one target dir. Build-isolation paths and worktree layout are machine-local; follow your environment's
process notes.

Do not merge feature work into `main` without a read-only, high-reasoning review. Use short-lived
branches, small commits, and prefer `git cherry-pick -x <reviewed-sha>` over merging a branch; if a
conflict is not an obvious mechanical one, send the branch back. Gate the push on a green full suite,
then a final review of the assembled diff. Before committing substantial code, spawn a read-only
review asking whether every changed line belongs and whether a senior language engineer would sign off
on the simplest solution.
