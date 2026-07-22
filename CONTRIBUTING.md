# Contributing

Marrow is an unreleased language, compiler, runtime, and durable-state system. A
contribution should leave one clear semantic owner for each concept, update the
current reference together with behavior, and remove replaced code rather than add
a compatibility path. This page describes the workflow, the gates a change runs,
and what belongs in an issue.

## How the source is organized

The workspace is a set of small Rust crates behind narrow public seams, one per
layer of the pipeline. The [implementation map](docs/implementation/README.md)
describes each; the layer boundaries are load-bearing:

- the **parser** (`marrow-syntax`) owns syntax and spans;
- the **compiler** (`marrow-compile`) owns resolved names, types, the durable
  graph, effects, and exports, and lowers to a reproducible program image
  (`marrow-image`);
- the **verifier** (`marrow-verify`) independently rechecks an image and is its
  only decoder;
- the **VM** (`marrow-vm`) runs a sealed image, and the **path kernel**
  (`marrow-kernel`) mediates every durable read and write over the ordered-byte
  **storage engine** (`marrow-store`);
- the **diagnostic-code registry** (`marrow-codes`) owns the closed set of typed
  codes; tools (`marrow-lsp`, the `marrow` CLI) consume published facts and
  reclassify nothing.

One concept has one owner. Do not duplicate a classifier for paths, builtins,
identity, saved values, diagnostics, or runtime behavior across layers, and do not
add a `common`, `util`, `session`, or `model` dumping ground. The parser owns
syntax, the compiler owns semantics, the storage substrate owns physical
representation; `marrow-lsp` adds no language semantics of its own and requests a
missing fact from the compiler rather than reconstructing it.

## Before changing behavior

Read the relevant [language reference](docs/language/) and
[implementation map](docs/implementation/). The documentation authority is
layered: `docs/language/` documents current behavior, `docs/vision.md` states
direction and non-goals, `docs/status.md` separates current from future, and
`docs/future/` records unimplemented direction without proposed syntax. There is
no separate specification tier or decision archive; implemented code, tests, and
the concise current reference move together. Discuss a genuine product choice when
it becomes necessary rather than opening a parallel source of truth.

Start behavior work with a failing test that exercises the narrowest production
path able to prove the rule. Assert typed codes, spans, values, facts, store
effects, or receipts rather than diagnostic prose. When behavior changes, update
the reference, status, implementation map, examples, and code in the same change,
and delete obsolete material rather than leaving contradictory timelines.

## Checks

Choose a Cargo target directory outside the checkout, and name it in every
command — `cargo` state is not inherited between invocations. On a shared build
host, follow its local `AGENTS.md` convention for the target location.

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo build --workspace --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test --workspace --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo fmt --all -- --check
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo clippy --workspace --all-targets --locked -- -D warnings
```

Run focused suites first, then the broad ones. Documentation changes have their
own focused gates — the diagnostic-registry drift test and the fence gate that
compiles and independently verifies every complete `mw` example in the reference:

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow --test doc_fences
```

Changes to the diagnostic registry regenerate its reference before the drift test:

```sh
MARROW_UPDATE_ERROR_CODES=1 CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
```

Every code change is expected to pass, on the pinned Rust 1.89 toolchain:

- a clean workspace build and the full workspace test suite;
- `cargo fmt --all -- --check` and `cargo clippy … -D warnings`;
- **zero production `unsafe`**;
- no unapproved dependency or `Cargo.lock` change (`Cargo.lock` is committed and
  changes only with an intentional, reviewed dependency change; a new dependency
  needs explicit approval and a license-compatibility review, since the source
  remains Apache-2.0).

Storage, lifecycle, identity, index, and write changes additionally run their
corruption, recovery, and backend conformance coverage. Before handoff, run
`git diff --check`.

## Review

A change is merged only after independent review — it is not self-approved.
Soundness-critical work (image, verifier, kernel, identity, or durable-format
contracts) takes two independent reviews, a soundness/probe lens and a
Rust-idiom/simplicity lens, and soundness findings are fixed and re-reviewed
clean. Other changes take at least one independent review plus the standing gates.
Fix every in-scope finding, and sweep sibling APIs for the same defect family
rather than patching only the reported line. Each change that establishes an
invariant should carry an enforcement artifact — a type boundary, a visibility
restriction, an absence or tidy test, or a drift check — that makes a recurrence
conspicuous.

## Code and documentation shape

- Prefer typed IDs, small enums, typed facts, and typed diagnostics to strings,
  booleans, raw paths, source spelling, or rendered-message matching. A boolean
  that changes semantics usually deserves a typed state.
- Keep potentially unbounded work paged or streamed; bound every decoder and input
  before it allocates.
- Split a broad dispatcher into focused helpers before review.
- Comments explain durable rationale, representation invariants, resource bounds,
  or soundness — not what the code does or which change introduced it. Prefer a
  better name or a smaller function to a narrating comment.
- Keep current, legacy, and future behavior in their documented sections; a future
  page never defines implemented behavior, and an example presented as complete
  must parse and check against the documented version.

## Filing an issue

An issue should be reproducible and grounded in observed behavior:

- **A defect** names the command or API, the input (a minimal `.mw` source or
  call), the observed typed code, span, value, or store effect, and what was
  expected instead. A diagnostic issue quotes the dotted code, not only the
  rendered message.
- **A documentation issue** names the page and the specific claim, and whether the
  reference, the status, or an example is wrong.
- **A direction question** belongs against `docs/vision.md` or a `docs/future/`
  page; it is a question about goals and constraints, not a request for a
  speculative feature queue. The project keeps no approval queue or decision
  archive, so an issue that asks to reserve future syntax or architecture will be
  closed with that explanation.

Report a suspected vulnerability privately through the channel in
[SECURITY.md](SECURITY.md) rather than in a public issue.
