# Contributing

Marrow is an unreleased language, compiler, runtime, and durable-state system.
Contributions should leave one clear semantic owner, update the current
reference with behavior, and remove replaced code rather than add a compatibility
path.

## Before changing behavior

Read the relevant [language reference](docs/language/) and
[implementation map](docs/implementation/). A source-language or durable-data
change updates code, production-pipeline tests, and its canonical reference in
the same change. Discuss a product choice when it becomes necessary; do not
create a parallel source of truth or speculative decision queue.

Start behavior work with a failing test that exercises the narrowest production
path able to prove the rule. Assert typed codes, values, facts, store effects, or
receipts rather than diagnostic prose.

## Checks

Choose a Cargo target directory outside the checkout. On a shared build host,
follow its local `AGENTS.md` convention and spell the target in every command.
For example:

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test --workspace
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo fmt --all -- --check
```

Run focused tests first. Documentation changes use these focused gates:

```sh
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow --test main docs_entrypoints::
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-check --test main language_reference_docs::
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-check --test main standard_library_docs::
CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
```

Changes to the diagnostic registry regenerate its reference before the drift
test:

```sh
MARROW_UPDATE_ERROR_CODES=1 CARGO_TARGET_DIR=/absolute/path/to/marrow-target cargo test -p marrow-codes --test error_codes_doc
```

Before handoff, run `git diff --check`, the affected focused suites, the full
workspace tests, formatter, and Clippy with warnings denied. Storage, lifecycle,
identity, and write changes also run their corruption, recovery, and conformance
coverage.

## Code and documentation shape

- Keep syntax ownership in the parser, semantic ownership in the compiler, and
  physical representation in the storage substrate.
- Prefer typed IDs, states, facts, and errors to strings, paths, booleans, or
  rendered-message matching.
- Keep potentially unbounded work paged or streamed.
- Delete obsolete helpers, tests, examples, exports, and comments with the code
  they served.
- Keep current, legacy, and future behavior in their documented sections; future
  pages do not define implemented behavior.

Report suspected vulnerabilities through the private channel in
[SECURITY.md](SECURITY.md).
