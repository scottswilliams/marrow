# Lane 11: Rust De-Slopification And Hardening

> For agentic workers: use the lane loop in `/Users/scottwilliams/Dev/AGENTS.md`.
> This lane proves cleanup happened; it is not a dumping ground for postponed
> semantic rewrites.

Goal: remove duplicate production paths, dead code, stale docs, prototype
vestiges, and weak Rust structure after the owning semantic lanes have replaced
their foundations.

Worktree: `/Users/scottwilliams/Dev/marrow-lane-11-rust-hardening`

Target dir: `/Users/scottwilliams/Dev/.build/marrow-targets/lane-11-rust-hardening`

Status: read-only scans may start anytime; broad code edits wait until owning
lanes land.

## Parallel Safety

This lane may run scans and file-disjoint style fixes in parallel only when the
owning lane is not touching the same files. If a scan finds a semantic bug in an
active lane, file it back to that lane instead of patching around it here.

Owned during final hardening:

- all Rust crates, sequenced by ownership boundaries already integrated;
- canonical docs and `docs/future/` stale content;
- top-level `AGENTS.md` or `CLAUDE.md` only for concise workflow/Rust guidance
  that applies to every lane.

Do not create new ADRs, new broad roadmaps, compatibility shims, or generic
cleanup commits with unrelated changes.

## Production Contract

- No duplicate production semantic paths remain.
- No crate-root glob prelude grows as a replacement for explicit imports.
- `unsafe` remains absent.
- Rust modules have one clear invariant where a touched file needs splitting.
- Tests use source-driven production fixtures rather than duplicate classifiers.
- Prototype docs are deleted or folded into canonical references.

## Prototype Removal Ledger

Replacement behavior: the production architecture in the central roadmap is the
only reachable architecture.

Delete or prove absent:

- AST runtime production path;
- source-name physical key production path;
- raw archive production backup path;
- runtime fallback resolution;
- duplicate semantic classifiers in checker, runtime, schema, and tools;
- executable `Unknown` or recovery facts;
- stale `docs/future` content whose constraints moved into canonical docs;
- comments that narrate edits, preserve temporary migration notes, or restate
  obvious Rust.

Temporary bridge allowed: none after this lane.

## TDD And Scan Start

Start with fresh scans, then turn each valid finding into the smallest focused
fix with a failing test or absence check:

```sh
rg -n 'unsafe\\s*(\\{|fn|impl|trait|extern)' /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/crates
rg -n 'Unknown|fallback|split\\(\"::\"\\)|@id|Book::Id|Author::Id|merge|lock|inout|migration script|raw path|backend bytes' \
    /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/crates \
    /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/docs
rg -n 'use super::\\*|pub use .*::\\*' /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/crates
```

Focused checks depend on each fix. Broad gate:

```sh
cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/Cargo.toml --all --check
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-11-rust-hardening \
    cargo test --manifest-path /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/Cargo.toml \
    --workspace --all-features
CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-11-rust-hardening \
    cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/Cargo.toml \
    --workspace --all-targets --all-features -- -D warnings
```

## Review Lenses

Soundness review checks removed paths are not reachable and scans cannot be
fooled by renamed helpers.

Idiom/spec review checks every deletion has an owner, Rust comments explain
durable rationale only, docs are not sediment, and no compatibility story is
invented without a prior lane.

## Integration Gate

Run the full central gate and repeat the scans above after rebasing onto the
current main. A match is acceptable only if it is explicit debug/admin scope,
future-only docs, or a rejection test.

## Starter Prompt

Continue Marrow v0.1 Lane 11 in `/Users/scottwilliams/Dev/marrow-lane-11-rust-hardening`.
Use branch `lane-11-rust-hardening`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-11-rust-hardening`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
Start with read-only scans for prototype paths, duplicate classifiers, `unsafe`,
glob preludes, stale docs, and low-value comments. Do not race active semantic
lanes; send semantic findings back to their owner. After owning lanes land,
delete remaining vestiges with focused tests or absence checks and leave the
worktree dirty for soundness and idiom/spec review.
