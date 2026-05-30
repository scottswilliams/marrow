# Marrow QA Bulk Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Resolve every issue recorded in the Marrow QA verified and follow-up ledgers, with regression coverage, conflict-aware branch choreography, and a shrinking open-work tracker.

**Architecture:** Treat the QA report as an external product backlog and land fixes through small, reviewed work packages. Each package starts from a failing reproduction, changes the minimum parser/checker/runtime/store/CLI/doc surface needed, removes only its fixed IDs from the tracker, and waits for integration before adjacent packages modify the same files.

**Tech Stack:** Rust workspace (`crates/marrow-*`), Marrow language docs under `docs/language/`, CLI integration tests under `crates/marrow/tests`, crate-local unit/integration tests, QA evidence under `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery`.

---

## Source Of Truth

- QA report: `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery/report/marrow-qa-report.md`
- Verified ledger: `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery/findings/verified.jsonl`
- Follow-up ledger: `/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery/findings/followups.jsonl`
- Active tracker: `docs/superpowers/plans/2026-05-30-marrow-qa-bulk-fix-tracker.md`

The tracker is intentionally an open-work document. When a row is fully fixed and verified, delete the row in the same commit that lands the fix. If only some IDs in a row are fixed, split the row first, delete the completed IDs, and leave the remaining IDs in the open table.

## Current Coordination State

Do not start implementation from `/Users/scottwilliams/Dev/marrow`; it currently has user/agent changes. Use fresh short-lived worktrees under `$HOME/agents-work/marrow/worktrees`.

Known active lanes that can conflict:

| Active branch | Areas touched | Coordinate before touching |
|---|---|---|
| `cli-doc-migration` | `docs/cli.md`, `docs/future/cli.md` | CLI docs, entry resolution docs |
| `element-loop-semantics` | checker loop rules, run traversal, collection/read paths | traversal typing, mutation guards, nested layers |
| `enum-binding-index` | checker binding indexes | enum/member binding diagnostics |
| `enum-segment-precision` | checker analysis and binding indexes | enum diagnostics, qualified enum names |
| `feat-defaults` | syntax AST/parser/formatter/checker/schema | parser and formatter packages |
| `identity-key-static-reject` | schema identity checking, error codes | identity constructor static checks |
| `literal-escape-decode` | literal runtime and language docs | string/byte escape fixes |
| `lsp-check-diagnostics` | check diagnostics, LSP, CLI docs | checker diagnostic wording and LSP output |
| `lsp-retire-inrepo` | CLI/LSP/serve docs and modules | serve or LSP-adjacent CLI changes |
| `fix-resource-ctor-runtime` | checker resource constructor tests and runtime calls | resource constructor work |
| `saved-walk-cursor` | store backend/conformance and serve protocol | `saved_walk`, store traversal, serve protocol |
| `unkeyed-required-fields` | schema and runtime read/write dispatch | unkeyed groups, required fields, partial records |

Before assigning any package, run:

```sh
git -C /Users/scottwilliams/Dev/marrow worktree list --porcelain
git -C /Users/scottwilliams/Dev/marrow fetch origin
```

If a package overlaps an active lane, inspect that lane first and either wait for integration or make the new worktree from that lane with explicit approval. Do not duplicate fixes already in flight.

## Baseline Commands

Set a lane-specific target directory before any Rust checks:

```sh
export CARGO_TARGET_DIR="$HOME/agents-work/marrow/cache/cargo-targets/<lane-name>"
```

Focused checks:

```sh
git diff --check
cargo test -p marrow-syntax
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow-store
cargo test -p marrow --test run_cli
cargo test -p marrow --test check_cli
cargo test -p marrow --test fmt_cli
cargo test -p marrow --test serve_cli
cargo test -p marrow --test backup_cli
```

Integration checks, one lane at a time:

```sh
export CARGO_TARGET_DIR="$HOME/agents-work/marrow/cache/cargo-targets/integration"
cargo build --workspace
cargo test --workspace
```

QA corpus checks after package integration:

```sh
QA_ROOT=/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery
for project in "$QA_ROOT"/corpus/*; do
  [ -f "$project/marrow.json" ] || continue
  cargo run -p marrow -- check "$project"
done
```

Expect some repro-only projects to remain non-clean until their owning package lands. The integration owner must compare failures against `corpus/manifest.json`, not just count nonzero exits.

## Design Questions To Ask Before Implementation

Ask these before starting the related package. Record the answer in the package commit message or docs change, not as a stale note in the tracker.

1. Should `require ... else` become accepted Marrow syntax now, or should `followup-presence-require#0` remain a post-1.0 language design backlog item?
2. Should local sequences and local keyed trees support the documented subscript/append behavior, or should the docs be reduced and the checker reject those operations clearly?
3. Should conversion builtins implement the documented broad conversions (`string(int)`, `decimal(int)`, `string(bytes)`, canonical temporal text), or should the checker statically reject unsupported conversions and the docs narrow the surface?
4. Should temporal parse helpers accept common ISO 8601/RFC 3339 boundary text, or remain strict canonical Marrow parsers with renamed/docs-adjusted APIs?
5. Should evaluator runtime faults such as overflow, divide-by-zero, and parse failures be catchable by `try/catch`, matching `run.absent_element`, or remain fatal with docs updated?
6. Should `std::text::split(text, "")` keep leading/trailing empty pieces, or return only scalar pieces?
7. Should Marrow add a named integer quotient helper/operator, or keep decimal `/` plus explicit floor/rounding as the only route?

## Package Workflow

For every tracker row:

- [ ] **Step 1: Create a package worktree**

```sh
git -C /Users/scottwilliams/Dev/marrow worktree add \
  "$HOME/agents-work/marrow/worktrees/<package-branch>" \
  -b "<package-branch>" origin/main
cd "$HOME/agents-work/marrow/worktrees/<package-branch>"
export CARGO_TARGET_DIR="$HOME/agents-work/marrow/cache/cargo-targets/<package-branch>"
```

- [ ] **Step 2: Reproduce before fixing**

Create or identify the narrowest failing test in the files listed for the package. Run only that test and confirm it fails for the QA symptom named by the finding IDs.

- [ ] **Step 3: Implement the minimum fix**

Touch only the listed implementation/docs files unless the failing test proves a different owner. Prefer one root-cause fix over one-off special cases.

- [ ] **Step 4: Run focused verification**

Run the package's focused crate tests, `git diff --check`, and any CLI test covering the symptom.

- [ ] **Step 5: Update the tracker**

Delete fixed IDs from `docs/superpowers/plans/2026-05-30-marrow-qa-bulk-fix-tracker.md`. If the row is partially fixed, split it and leave only the unresolved IDs.

- [ ] **Step 6: Commit**

```sh
git add <changed source/test/docs files> docs/superpowers/plans/2026-05-30-marrow-qa-bulk-fix-tracker.md
git commit -m "<area>: fix <root cause>"
```

- [ ] **Step 7: Review before integration**

Run a read-only review against `main`. Integrate only from `$HOME/agents-work/marrow/worktrees/main` by cherry-picking reviewed commits. If a conflict is not obvious and mechanical, stop and send the package back.

## Work Packages

### Package P0: Design Decisions

**Purpose:** Resolve design questions that determine implementation direction for the syntax/conversion/local-collection/error-model packages.

**Files:**
- Modify if decisions change language reference: `docs/language/control-flow-and-effects.md`, `docs/language/builtins.md`, `docs/language/types.md`, `docs/language/resources-and-storage.md`
- Modify tracker: `docs/superpowers/plans/2026-05-30-marrow-qa-bulk-fix-tracker.md`

**Focused verification:**

```sh
git diff --check
```

**Covered IDs:** `algo-csv-splitter#3`, `algo-gcd-euclid#0`, `algo-palindrome-utf8#2`, `cluster-clock-duration#5`, `cluster-sparse-presence#5`, `followup-presence-require#0`

### Package P1: Parser, Formatter, CLI, And Diagnostics

**Purpose:** Make source-preserving tooling non-destructive and reject/diagnose invalid syntax and CLI combinations consistently.

**Files:**
- Parser/formatter: `crates/marrow-syntax/src/lexer.rs`, `crates/marrow-syntax/src/parse_decl.rs`, `crates/marrow-syntax/src/parse_expr.rs`, `crates/marrow-syntax/src/format.rs`
- Checker diagnostics/name rejection: `crates/marrow-check/src/analysis.rs`, `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/resolve.rs`
- CLI check/fmt/test entry handling: `crates/marrow/src/cmd_check.rs`, `crates/marrow/src/cmd_fmt.rs`, `crates/marrow/src/cmd_test.rs`, `crates/marrow/src/cmd_run.rs`
- Tests: `crates/marrow-syntax/tests/parse.rs`, `crates/marrow-syntax/tests/format.rs`, `crates/marrow/tests/fmt_cli.rs`, `crates/marrow/tests/check_project_cli.rs`, `crates/marrow/tests/test_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs as needed: `docs/language/grammar.md`, `docs/cli.md`, `docs/error-codes.md`

**Focused verification:**

```sh
cargo test -p marrow-syntax
cargo test -p marrow-check
cargo test -p marrow --test fmt_cli
cargo test -p marrow --test check_project_cli
cargo test -p marrow --test test_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-collatz#3`, `algo-json-tokenizer#0`, `algo-matrix-multiply#1`, `algo-merge-sort#0`, `algo-merge-sort#1`, `algo-run-length-encode#1`, `algo-sieve-primes#1`, `app-expression-interpreter#5`, `app-double-entry-ledger#1`, `app-library-catalog#6`, `app-versioned-cms#6`, `cluster-cli-config-fmt#0`, `cluster-cli-config-fmt#1`, `cluster-cli-config-fmt#2`, `cluster-cli-config-fmt#3`, `cluster-cli-config-fmt#4`, `cluster-cli-config-fmt#5`, `cluster-controlflow-errors#0`, `cluster-controlflow-errors#1`, `cluster-controlflow-errors#2`, `cluster-modules-params#3`, `algo-factorial#4`, `app-versioned-cms#4`, `fuzz-6#0`, `fuzz-6#1`

### Package P2: Modules, Parameters, And Call Contracts

**Purpose:** Enforce `out`/`inout` contracts statically and preserve declared return types at call sites.

**Files:**
- Checker: `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/infer.rs`, `crates/marrow-check/src/rules.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime call validation if still needed: `crates/marrow-run/src/call.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/check_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/modules-functions.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test check_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-roman-numerals#3`, `apps:app-ttl-cache#4`, `cluster-modules-params#0`, `cluster-modules-params#1`, `cluster-modules-params#2`, `cluster-modules-params#6`

### Package P3: Module Constants

**Purpose:** Make module-level constants available at runtime wherever the checker resolves them.

**Files:**
- Schema/check artifacts: `crates/marrow-schema/src/lib.rs`, `crates/marrow-check/src/program.rs`, `crates/marrow-check/src/resolve.rs`
- Runtime environment/name resolution: `crates/marrow-run/src/env.rs`, `crates/marrow-run/src/expr.rs`, `crates/marrow-run/src/call.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/modules-functions.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-json-tokenizer#2`, `algo-palindrome-utf8#0`, `app-expression-interpreter#3`, `app-mini-spreadsheet#2`, `app-url-shortener#0`

### Package P4: Resource Constructors And Local Resource Values

**Purpose:** Align checked resource constructor syntax with runtime construction and qualified resource value handling.

**Files:**
- Checker/schema: `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/resolve.rs`, `crates/marrow-schema/src/lib.rs`
- Runtime: `crates/marrow-run/src/call.rs`, `crates/marrow-run/src/schema_query.rs`, `crates/marrow-run/src/value.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-schema/tests/compile_resource.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/types.md`, `docs/language/resources-and-storage.md`

**Focused verification:**

```sh
cargo test -p marrow-schema
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-compound-interest-decimal#3`, `algo-matrix-multiply#0`, `app-expression-interpreter#0`, `app-mini-spreadsheet#3`, `app-inventory-warehouse#2`, `app-library-catalog#2`, `app-dependency-graph#3`, `cluster-resources-identity#1`, `fuzz-10#0`, `fuzz-2#0`

### Package P5: Identity And Nominal Type Consistency

**Purpose:** Unify identity values across constructors, traversal, `nextId`, unique indexes, module-qualified spellings, equality, interpolation, and saved fields.

**Files:**
- Schema/checker: `crates/marrow-schema/src/lib.rs`, `crates/marrow-check/src/analysis.rs`, `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/resolve.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime identity/read/write: `crates/marrow-run/src/schema_query.rs`, `crates/marrow-run/src/value.rs`, `crates/marrow-run/src/expr.rs`, `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/write_dispatch.rs`, `crates/marrow-run/src/stdlib.rs`
- Store encoding if identity persistence changes: `crates/marrow-store/src/value.rs`
- Tests: `crates/marrow-schema/tests/compile_resource.rs`, `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow-store/tests/value_encoding.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/types.md`, `docs/language/resources-and-storage.md`, `docs/language/modules-functions.md`

**Focused verification:**

```sh
cargo test -p marrow-schema
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow-store
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-set-ops-keyedtree#1`, `app-banking-locks#5`, `app-registrar-composite-id#2`, `app-url-shortener#5`, `app-task-tracker#2`, `app-dependency-graph#8`, `app-inventory-warehouse#1`, `app-versioned-cms#0`, `app-audit-log#3`, `cluster-resources-identity#0`, `cluster-resources-identity#2`, `fuzz-11#1`

### Package P6: Conversions, Literals, And Temporal Boundaries

**Purpose:** Make conversion builtins either work as documented or reject statically, and decode string/byte literals consistently.

**Files:**
- Checker conversion typing: `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/infer.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime conversion/literals: `crates/marrow-run/src/stdlib.rs`, `crates/marrow-run/src/expr.rs`, `crates/marrow-run/src/base64.rs`, `crates/marrow-run/src/value.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/builtins.md`, `docs/language/grammar.md`, `docs/language/syntax.md`, `docs/language/standard-library.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-base64-roundtrip#2`, `algo-compound-interest-decimal#1`, `algo-compound-interest-decimal#2`, `algo-csv-splitter#0`, `algo-date-daycount-leap#3`, `algo-json-tokenizer#3`, `app-banking-locks#1`, `app-fsm-engine#0`, `app-url-shortener#1`, `app-library-catalog#0`, `cluster-clock-duration#2`, `cluster-conversions-unknown#0`, `cluster-conversions-unknown#1`, `cluster-conversions-unknown#3`, `cluster-conversions-unknown#4`, `cluster-enums#1`, `cluster-numerics-decimal#1`, `cluster-numerics-decimal#4`, `cluster-strings-bytes#0`, `cluster-strings-bytes#1`, `cluster-strings-bytes#2`, `fuzz-11#0`

### Package P7: Runtime Error Model And Numerics

**Purpose:** Make arithmetic, temporal, and evaluator runtime faults follow the chosen catchability and diagnostic model, and fix decimal arithmetic behavior.

**Files:**
- Runtime control flow/errors: `crates/marrow-run/src/error.rs`, `crates/marrow-run/src/exec.rs`, `crates/marrow-run/src/expr.rs`, `crates/marrow-run/src/stdlib.rs`
- Decimal/backend helpers: `crates/marrow-store/src/decimal.rs`
- Tests: `crates/marrow-run/tests/eval.rs`, `crates/marrow-store/tests/value_encoding.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/control-flow-and-effects.md`, `docs/language/builtins.md`, `docs/error-codes.md`

**Focused verification:**

```sh
cargo test -p marrow-run
cargo test -p marrow-store
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-compound-interest-decimal#0`, `algo-compound-interest-decimal#4`, `algo-date-daycount-leap#0`, `app-url-shortener#2`, `cluster-clock-duration#4`, `cluster-numerics-decimal#0`

### Package P8: Type Surfaces For Reads And Traversal

**Purpose:** Stop leaking `unknown` through `count`, `keys`, `values`, `entries`, optional chains, caught `Error` fields, saved layer reads, and loop bindings.

**Files:**
- Checker inference and saved path analysis: `crates/marrow-check/src/analysis.rs`, `crates/marrow-check/src/binding.rs`, `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/infer.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime read parity where needed: `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/collection.rs`, `crates/marrow-run/src/stdlib.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-check/tests/analysis_api.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/check_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/builtins.md`, `docs/language/resources-and-storage.md`, `docs/language/control-flow-and-effects.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test check_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-ackermann#0`, `algo-collatz#0`, `algo-csv-splitter#4`, `algo-date-daycount-leap#1`, `algo-date-daycount-leap#4`, `algo-factorial#2`, `algo-fibonacci#3`, `algo-fizzbuzz#3`, `algo-insertion-sort#0`, `algo-sieve-primes#0`, `app-calendar-scheduler#0`, `app-calendar-scheduler#2`, `app-expression-interpreter#1`, `app-url-shortener#8`, `app-audit-log#1`, `app-library-catalog#1`, `app-double-entry-ledger#0`, `app-dependency-graph#0`, `app-versioned-cms#3`, `app-versioned-cms#5`, `app-dependency-graph#7`, `app-inventory-warehouse#4`, `app-task-tracker#0`, `app-task-tracker#1`, `app-double-entry-ledger#3`, `app-audit-log#4`, `app-audit-log#6`, `cluster-indexes#2`, `cluster-indexes#4`, `cluster-sparse-presence#4`

### Package P9: Local Collections

**Purpose:** Make local sequences and local keyed trees behave according to the language reference, or narrow the reference and reject unsupported operations statically after design input.

**Files:**
- Parser/checker assignment target handling: `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/infer.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime local reads/writes/append/count: `crates/marrow-run/src/collection.rs`, `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/write_dispatch.rs`, `crates/marrow-run/src/stdlib.rs`, `crates/marrow-run/src/value.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow-run/src/write_tests.rs`, `crates/marrow/tests/check_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/types.md`, `docs/language/builtins.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test check_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-collatz#1`, `algo-collatz#2`, `algo-date-daycount-leap#5`, `algo-insertion-sort#1`, `algo-palindrome-utf8#1`, `algo-roman-numerals#4`, `app-calendar-scheduler#4`, `apps:app-ttl-cache#1`, `app-dependency-graph#4`, `app-dependency-graph#5`, `app-audit-log#5`

### Package P10: Saved Storage, Indexes, And Presence

**Purpose:** Fix unique index population and visibility, root counts, sparse/unkeyed writes, singleton integrity, and index argument support.

**Files:**
- Schema/index metadata: `crates/marrow-schema/src/lib.rs`
- Runtime read/write/index maintenance: `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/write.rs`, `crates/marrow-run/src/write_dispatch.rs`, `crates/marrow-run/src/schema_query.rs`
- Store traversal/integrity: `crates/marrow-store/src/backend.rs`, `crates/marrow-store/src/conformance.rs`, `crates/marrow-store/src/mem.rs`, `crates/marrow-store/src/redb.rs`, `crates/marrow-store/src/traversal.rs`
- Tests: `crates/marrow-schema/tests/compile_resource.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow-run/src/write_tests.rs`, `crates/marrow-store/tests/backend.rs`, `crates/marrow-store/tests/mem_store.rs`, `crates/marrow-store/tests/redb_store.rs`, `crates/marrow/tests/data_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/resources-and-storage.md`, `docs/data-tools.md`, `docs/backend-contract.md`

**Focused verification:**

```sh
cargo test -p marrow-schema
cargo test -p marrow-run
cargo test -p marrow-store
cargo test -p marrow --test data_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `app-calendar-scheduler#1`, `app-mini-spreadsheet#0`, `app-url-shortener#3`, `app-dependency-graph#1`, `cluster-backup-restore#0`, `cluster-clock-duration#3`, `cluster-indexes#1`, `cluster-indexes#3`, `cluster-saved-encoding-integrity#1`, `cluster-sparse-presence#0`, `cluster-sparse-presence#1`, `cluster-sparse-presence#2`, `cluster-sparse-presence#3`, `fuzz-9#1`

### Package P11: Traversal, Neighbors, And Mutation Guards

**Purpose:** Make traversal over composite and nested saved layers addressable, align `next`/`prev` with docs, and enforce traversed-layer mutation guards for dynamic/reversed/index writes.

**Files:**
- Checker loop mutation rules: `crates/marrow-check/src/analysis.rs`, `crates/marrow-check/src/checks.rs`
- Runtime traversal/read/write: `crates/marrow-run/src/collection.rs`, `crates/marrow-run/src/exec.rs`, `crates/marrow-run/src/path.rs`, `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/write_dispatch.rs`
- Store traversal ordering: `crates/marrow-store/src/traversal.rs`, `crates/marrow-store/src/path.rs`
- Tests: `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow-store/tests/traversal.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/builtins.md`, `docs/language/resources-and-storage.md`, `docs/language/control-flow-and-effects.md`

**Focused verification:**

```sh
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow-store
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `algo-csv-splitter#1`, `algo-csv-splitter#2`, `app-registrar-composite-id#1`, `app-library-catalog#7`, `cluster-controlflow-errors#3`, `cluster-sequences-traversal#0`, `cluster-sequences-traversal#1`, `cluster-sequences-traversal#2`, `cluster-sequences-traversal#3`, `fuzz-9#0`

### Package P12: Enums

**Purpose:** Make enum visibility, qualified enum field types, enum stored reads, scalar conversion, and diagnostics coherent.

**Files:**
- Schema/checker enum resolution: `crates/marrow-schema/src/lib.rs`, `crates/marrow-check/src/enums.rs`, `crates/marrow-check/src/resolve.rs`, `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/typerules.rs`
- Runtime enum reads/conversions: `crates/marrow-run/src/expr.rs`, `crates/marrow-run/src/read.rs`, `crates/marrow-run/src/schema_query.rs`, `crates/marrow-run/src/stdlib.rs`
- Tests: `crates/marrow-schema/tests/compile_enum.rs`, `crates/marrow-schema/tests/resolve_type.rs`, `crates/marrow-check/tests/project.rs`, `crates/marrow-run/tests/eval.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/language/enums.md`, `docs/language/types.md`

**Focused verification:**

```sh
cargo test -p marrow-schema
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `app-expression-interpreter#2`, `app-expression-interpreter#4`, `cluster-enums#2`, `cluster-enums#3`

### Package P13: Data CLI, Serve Protocol, And Locks

**Purpose:** Fix backup/restore error behavior, serve root traversal/limit semantics, and static/runtime lock target validation.

**Files:**
- Backup/data CLI: `crates/marrow/src/cmd_backup.rs`, `crates/marrow/src/cmd_data.rs`
- Serve CLI/protocol: `crates/marrow/src/cmd_run.rs`, `crates/marrow/tests/serve_cli.rs`; if `crates/marrow/src/serve/` exists on the active base, modify the protocol module there
- Store/archive/backend: `crates/marrow-store/src/archive.rs`, `crates/marrow-store/src/backend.rs`, `crates/marrow-store/src/conformance.rs`, `crates/marrow-store/src/mem.rs`, `crates/marrow-store/src/redb.rs`
- Checker/runtime lock validation: `crates/marrow-check/src/checks.rs`, `crates/marrow-run/src/exec.rs`
- Tests: `crates/marrow-store/tests/archive.rs`, `crates/marrow-store/tests/backend.rs`, `crates/marrow/tests/backup_cli.rs`, `crates/marrow/tests/data_cli.rs`, `crates/marrow/tests/serve_cli.rs`, `crates/marrow/tests/run_cli.rs`
- Docs: `docs/data-tools.md`, `docs/serve-protocol.md`, `docs/language/control-flow-and-effects.md`

**Focused verification:**

```sh
cargo test -p marrow-store
cargo test -p marrow-check
cargo test -p marrow-run
cargo test -p marrow --test backup_cli
cargo test -p marrow --test data_cli
cargo test -p marrow --test serve_cli
cargo test -p marrow --test run_cli
git diff --check
```

**Covered IDs:** `cluster-backup-restore#1`, `cluster-backup-restore#3`, `cluster-serve#0`, `cluster-serve#1`, `cluster-transactions-locks#1`

### Package P14: Corpus Promotion And Final Closure

**Purpose:** Convert fixed QA repros into durable tests/fixtures, run the full workspace and corpus checks, and close the tracker.

**Files:**
- Tests touched by packages above
- Optional durable fixture docs: `docs/roadmap/README.md` if release status changes
- Tracker: `docs/superpowers/plans/2026-05-30-marrow-qa-bulk-fix-tracker.md`

**Focused verification:**

```sh
git diff --check
cargo build --workspace
cargo test --workspace
QA_ROOT=/Users/scottwilliams/.config/superpowers/worktrees/marrow-qa/phase2-recovery
for project in "$QA_ROOT"/corpus/*; do
  [ -f "$project/marrow.json" ] || continue
  cargo run -p marrow -- check "$project"
done
```

The tracker is complete only when the open-work table is empty and every fixed ID is covered by a source/test/doc commit.

## Integration Rules

1. Prefer one package branch per root-cause area.
2. Keep commits small enough to review independently.
3. Never merge package branches directly into `main`.
4. Cherry-pick reviewed commits from `$HOME/agents-work/marrow/worktrees/main`.
5. If a package overlaps an active branch, wait for the owner or explicitly coordinate a base branch.
6. Run package-focused tests before review, then integration tests after cherry-picking.
7. Keep the tracker shrinking; do not add completed rows, stale transition notes, or historical clutter.

## Self-Review

- Spec coverage: The tracker covers 166 IDs: 165 verified findings plus 1 follow-up finding.
- Placeholder scan: No package contains a deferred "TBD" action; each has files, checks, and covered IDs.
- Conflict hygiene: Active worktrees are listed, and package workflow requires worktree inspection before implementation.
- Design input: Seven design questions are isolated before implementation so semantic choices do not leak into accidental code decisions.
