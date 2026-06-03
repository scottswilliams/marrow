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

## Completion Claim Discipline

Lane 11 may report **audit complete** after read-only scans, or **blocked** when
active semantic lanes still own the residue. It may not report lane complete
until Lanes 9 and 10 have landed or explicitly handed over file-disjoint cleanup
with no semantic ownership ambiguity.

Before any final hardening edit, refresh every scan from the current HEAD. The
code smell list below is a seed, not authority: stale line numbers, renamed
files, or already-fixed findings must be replaced with current evidence or
deleted from the active list. Do not implement from old chat memory.

Lane-complete requires:

- a refreshed feature-surface absence matrix proving each surviving language,
  runtime, store, evolution, tooling, protocol, test, and doc surface has an
  owning verdict;
- no active-lane blocker hidden as a Lane 11 cleanup item;
- focused deletion/split batches with tests or absence scans, not broad cleanup
  commits;
- no new compatibility glue, fallback classifier, or low-value comment added
  while deleting old code;
- final scans for `unsafe`, duplicate classifiers, raw production paths, broad
  glob imports, stale docs, unsupported feature terms, and comment sediment.

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

## Area Cleanup Gate

This lane is the final audit, not the place where active lanes dump avoidable
mess. If a scan finds oversized functions, duplicate classifiers, compatibility
glue, stale fixtures, or comment sediment introduced by an active semantic lane,
send the finding back to that lane before it integrates.

Lane 11 also performs the final global feature-surface absence scan. It proves
that each surviving language, runtime, storage, evolution, CLI, LSP, data,
serve, backup, restore, docs, test, and future-doc surface has an owning-lane
verdict: keep production, debug/admin only, rename/rescope, or delete. A missing
verdict is a blocker for the semantic owner, not permission for Lane 11 to invent
a product story.

When this lane does edit Rust or docs:

- split or delete the touched production path in the same focused change that
  exposes the smell;
- reject any leftover legacy path whose only purpose is keeping obsolete tests,
  fixtures, or compile-time callers alive; send it back to the owning lane when
  that owner is still active;
- keep each hardening batch file-disjoint from active semantic lanes;
- delete comments that narrate old edits, temporary migration state, or obvious
  control flow;
- preserve only comments that explain durable invariants or soundness rationale;
- ensure the idiom/spec reviewer explicitly checks that the lane did not become
  a generic cleanup grab bag.

## Production Contract

- No duplicate production semantic paths remain.
- No crate-root glob prelude grows as a replacement for explicit imports.
- `unsafe` remains absent.
- Rust modules have one clear invariant where a touched file needs splitting.
- Tests use source-driven production fixtures rather than duplicate classifiers.
- Prototype docs are deleted or folded into canonical references.
- Unsupported feature surfaces have been deleted, explicitly demoted to
  debug/admin, or returned to their semantic owner before integration.

## Prototype Removal Ledger

Replacement behavior: the production architecture in the central roadmap is the
only reachable architecture.

Delete or prove absent:

- AST runtime production path;
- source-name physical key production path;
- runtime fallback resolution;
- duplicate semantic classifiers in checker, runtime, schema, and tools;
- executable `Unknown` or recovery facts;
- stale `docs/future` content whose constraints moved into canonical docs;
- unsupported or unowned feature surfaces in language, runtime, storage,
  evolution, tooling, protocols, tests, and docs;
- comments that narrate edits, preserve temporary migration notes, or restate
  obvious Rust.

Production bridge: none after this lane.

## Code Smell Fix List

These are live blockers from read-only scans. Priority 1 blocks the named owner
before integration. Priority 2 blocks any lane that touches the area; Lane 11
owns only final absence scans and file-disjoint cleanup in already-integrated
areas. Delete each bullet once its owner proves absence or moves a real
out-of-scope item to the forward-only backlog; do not retain completed scan
history.

Before acting on any item, rerun the scan from the current lane worktree and
replace stale evidence with current file/line references. If an active owner is
still editing the area, return the refreshed finding to that owner and mark Lane
11 blocked for that surface. Lane 11 does not patch around unfinished semantic
work to make the final scan pass.

Priority 1:

**Lane 2 - Prototype Rejection.** Stop `merge` and `lock` from remaining normal
parser/formatter output. Evidence: `crates/marrow-syntax/src/ast.rs:384`,
`crates/marrow-syntax/src/ast.rs:437`,
`crates/marrow-syntax/src/format.rs:486`,
`crates/marrow-syntax/src/format.rs:568`, and
`crates/marrow-check/src/prototype.rs:67`. Make them rejection-only or remove
their v0.1 round-trip surface after checker rejection is established.

**Lane 8 - Enum Runtime Values.** Consume catalog-backed enum member identity for
runtime values and index maintenance. Evidence:
`crates/marrow-run/src/expr.rs:43`, `crates/marrow-run/src/exec.rs:364`,
`crates/marrow-run/src/write.rs:1425`, `crates/marrow-run/src/write.rs:1563`,
`crates/marrow-run/src/schema_query.rs:153`,
`crates/marrow-run/src/write_tests.rs:924`, and
`crates/marrow-run/tests/eval.rs:9745` still encode enum members by declaration
or pre-order ordinal. Lane 7 provides the tree-cell enum value codec, and Lane 6
provides catalog-backed enum member facts; runtime must consume those checked
facts instead of raw ordinals. Declaration or pre-order ordinals may survive only
as schema traversal indexes, not durable stored meaning.

**Lane 8 - Checked Runtime IR.** Ensure diagnostic/recovery `Unknown` cannot
enter checked runtime IR. Evidence: `crates/marrow-check/src/program.rs:129`
still exposes source-body functions with best-effort types, and runtime still
consumes that bridge. Keep explicit user `unknown` dynamic-boundary types
separate from recovery sentinels.

**Lane 8 - Checked Runtime.** Delete production AST execution. Evidence:
`crates/marrow-run/src/call.rs:138` still invokes source bodies, and
`crates/marrow-run/src/exec.rs:39` still interprets syntax blocks. Runtime
invocation must move to checked executable facts or checked IR.

**Lane 8 - Checked Runtime.** Stop resolving function and call targets by
strings at runtime. Evidence: `crates/marrow-run/src/call.rs:120`,
`crates/marrow-run/src/call.rs:357`, `crates/marrow-run/src/call.rs:929`, and
`crates/marrow-run/src/call.rs:1034` split `::`, expand aliases, and retry
lookup. Checked entry and call target IDs must be authoritative.

**Lane 8 - Checked Runtime.** Remove runtime saved-path classifiers as
production semantics. Evidence: `crates/marrow-run/src/schema_query.rs:206`,
`crates/marrow-run/src/path.rs:221`, `crates/marrow-run/src/read.rs:14`,
`crates/marrow-run/src/collection.rs:65`,
`crates/marrow-run/src/stdlib.rs:183`,
`crates/marrow-run/src/schema_query.rs:365`,
`crates/marrow-store/src/lib.rs:13`, `crates/marrow-store/src/path.rs:20`,
`crates/marrow-store/src/path.rs:47`, `crates/marrow-store/src/backend.rs:114`,
and `crates/marrow-store/src/conformance.rs:1` rederive or expose durable
meaning through syntax, decoded raw paths, or source-spelled saved-path bytes.
Runtime should consume checked durable-place, traversal, index, catalog/store,
and tree-cell facts. After runtime and tooling stop consuming saved paths, the
store backend/path surface must be debug/admin-only or deleted rather than a
production storage law.

**Lane 8 - Checked Runtime.** Stop building write addresses from source
spellings. Evidence: `crates/marrow-run/src/path.rs:195`,
`crates/marrow-run/src/write.rs:1210`, `crates/marrow-run/src/write.rs:1217`,
and `crates/marrow-run/src/write.rs:1477`. Writes must be driven by checked
durable-place and store-address facts, with source-spelling helpers limited to
debug rendering if they survive.

**Lane 8 - Checked Runtime.** Remove runtime compatibility fallbacks and
branches for rejected prototype constructs. Evidence:
`crates/marrow-run/src/call.rs:988`, `crates/marrow-run/src/call.rs:1012`,
`crates/marrow-run/src/stdlib.rs:618`,
`crates/marrow-run/src/exec.rs:132`, `crates/marrow-run/src/exec.rs:322`, and
`crates/marrow-run/src/call.rs:432`. Dispatch only checked std descriptors;
let checked IR exclude `merge`, `lock`, and saved `inout`.

**Lane 10 - Tooling And Protocols.** Replace raw backup, data, explain/CLI,
LSP, and serve protocol/tool surfaces. Evidence:
`crates/marrow-store/src/archive.rs:29`,
`crates/marrow-store/src/archive.rs:84`,
`crates/marrow-store/src/archive.rs:122`, `crates/marrow/src/cmd_data.rs:242`,
`crates/marrow/src/cmd_data.rs:292`, `crates/marrow/src/cmd_data.rs:394`,
`crates/marrow/src/cmd_data.rs:432`, `crates/marrow/src/cmd_explain.rs:89`,
`crates/marrow/src/cmd_explain.rs:254`,
`crates/marrow/src/serve/protocol.rs:78`,
`crates/marrow/src/serve/protocol.rs:121`, and
`crates/marrow/src/serve/protocol.rs:200` expose raw bytes, raw path JSON,
physical key bytes, or tool-local classifiers. Replace them with typed backup
manifests, opaque cursors, bounded snapshot/paging APIs, and shared
checked/catalog/store facts; raw archive replay must be debug/admin-only or
deleted as a backup/restore path, and restore must validate or rebuild derived
data before commit.

Priority 2:

Any active lane touching checker traversal or scope mechanics must split
duplicate walks and broad dispatchers before review. Lane 11 owns only the
final absence scan and file-disjoint cleanup in already-integrated areas.
Evidence: `crates/marrow-check/src/checks.rs:619`,
`crates/marrow-check/src/checks.rs:2408`,
`crates/marrow-check/src/enums.rs:291`,
`crates/marrow-check/src/binding.rs:739`, and
`crates/marrow-check/src/facts.rs:541`.

Any lane touching checker or runtime module boundaries must remove crate-root
glob plumbing and production `use super::*` in its changed area. Evidence:
`crates/marrow-check/src/lib.rs:31`,
`crates/marrow-check/src/checks.rs:8`, `crates/marrow-check/src/infer.rs:3`,
`crates/marrow-check/src/enums.rs:4`, and
`crates/marrow-run/src/call.rs:3`.

Any lane touching call/check/enum dispatch must eliminate clippy allowances that
hide weak structure. Evidence: `crates/marrow-run/src/call.rs:232`,
`crates/marrow-check/src/checks.rs:2407`,
`crates/marrow-check/src/checks.rs:2917`,
`crates/marrow-check/src/enums.rs:96`, and
`crates/marrow-check/src/enums.rs:492` suppress
`clippy::too_many_arguments`.

Any lane touching the catch-all test suites must move changed coverage into
source-driven invariant fixtures and delete migrated assertions from the
aggregator before review. Evidence: `crates/marrow-run/tests/eval.rs`,
`crates/marrow-check/tests/project.rs`, `crates/marrow-syntax/tests/parse.rs`,
`crates/marrow-check/tests/checked_program.rs`, and
`crates/marrow-run/src/write_tests.rs` are thousand-line aggregators.

Any lane touching these areas must delete comment sediment. Evidence:
`crates/marrow-check/src/lib.rs:31`, `crates/marrow-run/src/lib.rs:20`,
`crates/marrow-run/src/lib.rs:80`, and
`crates/marrow-run/tests/eval.rs:7706` narrate migration or module plumbing.
Retain only durable rationale that explains a non-obvious invariant.

## TDD And Scan Start

Start with fresh scans, then turn each valid finding into the smallest focused
fix with a failing test or absence check:

```sh
rg -n 'unsafe\\s*(\\{|fn|impl|trait|extern)' /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/crates
rg -n 'Unknown|fallback|split\\(\"::\"\\)|@id|Book::Id|Author::Id|merge|lock|inout|migration script|raw path|backend bytes' \
    /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/crates \
    /Users/scottwilliams/Dev/marrow-lane-11-rust-hardening/docs
rg -n 'explain|serve|trace|dry-run|maintenance|query|server|sync|generated API|migration DSL|source diff|raw saved|backend bytes|source-order|ordinal|@id|merge|lock|inout|cache\s*~|ensure\s*~|Id\s*\(\s*~' \
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
fooled by renamed helpers. It also verifies every surviving feature-surface
match has an owning-lane verdict and cannot be used as a production bypass.

Idiom/spec review checks every deletion has an owner, Rust comments explain
durable rationale only, docs are not sediment, and no compatibility story is
invented without a prior lane. It also rejects generic cleanup batches,
oversized replacement functions, duplicate classifiers, comment sediment, and
cleanup that should have been returned to an owning active lane.

## Integration Gate

Run the full central gate and repeat the scans above after rebasing onto the
current main. A match is acceptable only if it is explicit debug/admin scope,
future-only docs, or a rejection test.

## Starter Prompt

Continue Marrow v0.1 Lane 11 in `/Users/scottwilliams/Dev/marrow-lane-11-rust-hardening`.
Use branch `lane-11-rust-hardening`, use
`CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/lane-11-rust-hardening`
on every cargo command, and follow `/Users/scottwilliams/Dev/AGENTS.md`.
First inspect current `main`, worktrees, active branches, and dirty files. Start
with read-only scans for prototype paths, duplicate classifiers, `unsafe`, glob
preludes, stale docs, low-value comments, and unsupported feature surfaces.
Refresh every code-smell list item from current HEAD before using it; stale chat
or stale line numbers are not evidence.

If Lanes 9 or 10 have not landed, or if Lane 8/9/10 still owns a semantic
surface you find, return an **audit complete** or **blocked** packet, not a done
claim. Send semantic and feature-surface findings back to their owner. Final
hardening edits wait until the owning semantic lanes land, except truly
file-disjoint style fixes that do not touch active lane files.

No legacy survival for green tests: reject leftover legacy paths kept only for
obsolete tests, fixtures, compile-time callers, or runtime green-bar pressure,
and send them back to the active owner. After owning lanes land, delete remaining
vestiges with focused tests or absence checks. Before review, satisfy the Area
Cleanup Gate: keep hardening batches file-disjoint, return active-lane smells to
their owner, split or delete the touched production path in the same focused
change, and avoid generic cleanup grabs. Leave the worktree dirty for soundness
and idiom/spec review. A final done claim must include the completion evidence
packet required by the central plan.
