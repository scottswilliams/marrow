# Contributing

Marrow is an unreleased language, compiler, runtime, and durable-state system. A
contribution leaves one clear semantic owner for each concept, updates the
current reference together with behavior, and removes replaced code instead of
adding a compatibility path. This page describes the workflow, the checks a
change runs, and what belongs in an issue.

## How the source is organized

The workspace is a set of small Rust crates with narrow public interfaces, one
per stage of the pipeline. The [implementation
map](docs/implementation/README.md) describes each; the boundaries between them
are the design:

- the parser (`marrow-syntax`) owns syntax and spans;
- the compiler (`marrow-compile`) owns resolved names, types, the durable
  graph, effects, and exports, and produces a reproducible program image
  (`marrow-image`);
- the verifier (`marrow-verify`) rechecks an image from its bytes alone and is
  its only decoder;
- the VM (`marrow-vm`) runs a verified image, and the durable kernel
  (`marrow-kernel`) carries every durable read and write to the storage engine
  (`marrow-store`);
- the diagnostic-code registry (`marrow-codes`) owns the closed set of codes;
  tools (`marrow-lsp`, the `marrow` CLI) consume published facts and reclassify
  nothing.

One concept has one owner. A classifier for paths, builtins, identity, stored
values, diagnostics, or runtime behavior lives in one layer, and there is no
`common`, `util`, `session`, or `model` module to collect strays. The parser
owns syntax, the compiler owns semantics, the storage engine owns physical
representation; `marrow-lsp` adds no language semantics of its own and asks the
compiler for a missing fact instead of reconstructing it.

## Documentation authority

The [language reference](docs/language/) defines current behavior, and every
`mw` fence in it is a complete file that compiles and passes `marrow test`.
`docs/vision.md` states direction and non-goals, `docs/status.md` separates
current from future work, and `docs/future/` records unimplemented direction
without defining syntax or exact formats. There is no separate specification
tier or decision archive: code, tests, and the reference move together, and a
genuine product choice is discussed when it becomes necessary.

## Before changing behavior

Read the relevant reference page and the [implementation
map](docs/implementation/). Start behavior work with a failing test that
exercises the narrowest path able to prove the rule. Assert codes, spans,
values, facts, store effects, or receipts, and leave diagnostic prose to the
diagnostic-voice guide. When behavior changes, update the reference, status,
implementation map, examples, and code in the same change, and delete obsolete
material so no contradictory timeline remains.

## Checks

Choose a Cargo target directory outside the checkout and name it in every
command; `cargo` state is not inherited between invocations. On a shared build
host, follow its local convention for the target location.

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo build --workspace --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test --workspace --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo fmt --all -- --check
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo clippy --workspace --all-targets --locked -- -D warnings
```

Run focused suites first, then the broad ones. Documentation changes have two
focused checks of their own: the diagnostic-registry drift test, and the fence
test that compiles and verifies every complete `mw` example in the reference:

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow --test doc_fences
```

A change to the diagnostic registry regenerates its reference before the drift
test:

```sh
MARROW_UPDATE_ERROR_CODES=1 CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
```

Every code change passes, on the pinned Rust 1.89 toolchain:

- a clean workspace build and the full workspace test suite;
- `cargo fmt --all -- --check` and `cargo clippy … -D warnings`;
- zero `unsafe` in production code;
- no unapproved dependency or `Cargo.lock` change. `Cargo.lock` is committed
  and changes only with an intentional, reviewed dependency change; a new
  dependency needs explicit approval and a license-compatibility review, since
  the source remains Apache-2.0.

Storage, lifecycle, identity, index, and write changes also run their
corruption, recovery, and backend conformance coverage. Before handoff, run `git
diff --check`.

## Review

A change is merged after review by someone other than its author.
Soundness-critical work (image, verifier, kernel, identity, or durable-format
contracts) takes two reviews, one for soundness with probes and one for Rust
idiom and simplicity, and soundness findings are fixed and re-reviewed clean.
Other changes take at least one review plus the standing checks. Fix every
in-scope finding, and sweep sibling APIs for the same defect family. A change
that establishes an invariant carries an artifact that keeps it: a type
boundary, a visibility restriction, an absence or tidy test, or a drift check,
so that a recurrence is conspicuous.

## Code and documentation shape

- Prefer newtyped IDs, small enums, and structured facts and diagnostics to
  strings, booleans, raw paths, source spelling, or rendered-message matching. A
  boolean that changes semantics usually deserves a named state.
- Keep potentially unbounded work paged or streamed; bound every decoder and
  input before it allocates.
- Split a broad dispatcher into focused helpers before review.
- Comments explain durable rationale, representation invariants, resource
  bounds, or soundness. They do not narrate what the code does or which change
  introduced it; prefer a better name or a smaller function to a narrating
  comment.

## Filing an issue

An issue is reproducible and grounded in observed behavior:

- A defect names the command or API, the input (a minimal `.mw` source or
  call), the observed code, span, value, or store effect, and what was expected
  instead. A diagnostic issue quotes the dotted code as well as the rendered
  message.
- A documentation issue names the page and the specific claim, and whether the
  reference, the status, or an example is wrong.
- A direction question belongs against `docs/vision.md` or a `docs/future/`
  page; it is a question about goals and constraints. The project keeps no
  approval queue or decision archive, so an issue that asks to reserve future
  syntax or architecture is closed with that explanation.

Report a suspected vulnerability privately through the channel in
[SECURITY.md](SECURITY.md), and not in a public issue.
