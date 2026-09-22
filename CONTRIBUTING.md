# Contributing

Marrow is an unreleased language, compiler, runtime, and durable-state system. A
contribution leaves one clear semantic owner for each concept, updates the
current reference together with behavior, and removes replaced code instead of
adding a compatibility path. This page describes the workflow: the checks a
change runs, how it is reviewed, and what belongs in an issue.
[AGENTS.md](AGENTS.md) owns the architecture rules — crate boundaries, typed
identity, code shape, and the documentation authority table — and this page does
not restate them.

## How the source is organized

The workspace is a set of small Rust crates with narrow public interfaces, one
per stage of the pipeline: parser, compiler, image, verifier, VM, durable
kernel, storage engine, diagnostic-code registry, and tools. The
[implementation map](docs/implementation/README.md) describes each crate, the
direction of every dependency, and how one command travels the whole stack.
Those boundaries are the design: one concept has one owner, and a classifier for
paths, builtins, identity, stored values, diagnostics, or runtime behavior lives
in exactly one layer.

## Before changing behavior

Read the relevant [reference page](docs/language/) and the [implementation
map](docs/implementation/README.md). Every `mw` fence in the language reference
is a complete file that compiles and passes `marrow test`, so an example is
evidence and not illustration. Start behavior work with a failing test that
exercises the narrowest path able to prove the rule. Assert codes, spans,
values, facts, store effects, or receipts, and leave diagnostic prose to the
[diagnostic-voice guide](docs/implementation/diagnostic-voice.md). When behavior
changes, update the reference, status, implementation map, examples, and code in
the same change, and delete obsolete material so no contradictory timeline
remains.

Preserve unrelated changes in a dirty worktree, and report completion only from
fresh output.

## Checks

Choose a Cargo target directory outside the checkout and name it in every
command; `cargo` state is not inherited between invocations, and a build host
may have its own convention for where that directory lives — a local
convention, not repository policy.

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo build --workspace --all-targets --all-features --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test --workspace --all-targets --all-features --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test --workspace --doc --all-features --locked
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo fmt --all -- --check
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo clippy --workspace --all-targets --all-features --locked -- \
  -D warnings -F unsafe-code
```

That clippy line is the one CI runs.

The separate `--doc` command runs documentation examples and compile-fail public
boundary checks; `--all-targets` does not include them.

Run focused suites first, then the broad ones. Documentation changes check
inventory, links, anchors and terminology, generated diagnostic drift, and
complete `mw` examples through the production compiler and test runner:

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test docs_gates
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow --test doc_fences
```

A change to the diagnostic registry regenerates its reference before the drift
test:

```sh
MARROW_UPDATE_ERROR_CODES=1 CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
```

Every code change passes all of the above on the toolchain `rust-toolchain.toml`
pins, with zero `unsafe` in production code and no unapproved dependency or
`Cargo.lock` change. `Cargo.lock` is committed and changes only with an
intentional, reviewed dependency change; a new dependency needs explicit
approval and a license-compatibility review, since the source remains
Apache-2.0.

Storage, lifecycle, identity, index, and write changes also run their
corruption, recovery, and backend conformance coverage. Before handoff, run `git
diff --check`.

## Review

A change is merged after review by someone other than its author. Substantial
work takes two independent reviews, one for soundness with probes and one for
code shape and reference clarity. Soundness findings are fixed and re-reviewed
clean. Small changes take at least one review plus the standing checks. Fix
every in-scope finding, and sweep sibling APIs for the same defect family. A
change that establishes an invariant carries an artifact that keeps it: a type
boundary, a visibility restriction, an absence or tidy test, or a drift check,
so that a recurrence is conspicuous.

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
  approval queue or decision archive
  ([documentation authority](AGENTS.md#documentation-authority)), so an issue
  that asks to reserve future syntax or architecture is closed with that
  explanation.

Report a suspected vulnerability privately through the channel in
[SECURITY.md](SECURITY.md), and not in a public issue.
