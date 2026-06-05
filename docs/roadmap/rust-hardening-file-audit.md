# Rust Hardening File Audit

Status: active
Started: 2026-06-05
Operational owner: orchestrator

This tracker is the operational source of truth for the Marrow rust-hardening audit. It records every tracked file, lane ownership, review state, findings, verification, reviewer verdicts, and absence evidence. Update it forward only: completed history is collapsed into current state plus evidence, and real deferred work becomes a concrete backlog item with an owner area and blocking reason.

## Base And Head

- Main checkout: `/Users/scottwilliams/Dev/marrow`
- Tracker lane worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-tracker`
- Tracker lane branch: `rust-hardening-tracker`
- Base commit: `7435c7dbd6ae9817460d5d44ebaa0e54c0aa9b70`
- Audit-start tracked head: `7435c7dbd6ae9817460d5d44ebaa0e54c0aa9b70`
- L00 integrated tracker commit on main: `9415b37635bfde9d42437bca3862f5db92d5fb9d`
- Main status at audit start: clean, `## main...origin/main`, head `7435c7dbd6ae9817460d5d44ebaa0e54c0aa9b70`
- Live main integration state is intentionally not frozen here. Re-run `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow status --short --branch` immediately before every integration, then record that fresh state in the lane evidence packet.
- Tracked file count at audit start: 279
- Current tracked file count after integrating this tracker on main: 277
- `docs/roadmap/` did not exist at audit start; this file creates it.

## Status Values

- `unreviewed`: Inventoried, owner lane assigned, no lane review yet.
- `reviewed-clean`: Read by a lane and found clean with evidence.
- `needs-lane`: Finding recorded; lane not started or not yet owning the fix.
- `in-lane`: Active lane owns the file.
- `blocked`: Real issue exists but is blocked by an explicit decision or dependency.
- `deleted`: File removed by an integrated lane.
- `complete`: Lane fixed or verified the file and passed review/integration gates.

## Lane Plan

Each implementation lane uses an isolated worktree outside the repo and a lane-specific `CARGO_TARGET_DIR` under `/Users/scottwilliams/Dev/.build/marrow-targets/`. Cargo commands must include both the lane target dir and `--manifest-path` pointing at that lane worktree's `Cargo.toml`.

| Lane | Area | Concurrency Notes | Initial Status |
|---|---|---|---|
| L00 | Root, manifests, fixtures, tracker | Serialize with integration gates and dependency checks. | unreviewed |
| L01 | Language reference | Language semantics require explicit proposal before behavior changes. | unreviewed |
| L02 | Non-language docs and future docs | File-disjoint from code lanes unless docs change with semantics. | unreviewed |
| L03 | Syntax, parser, formatter | Can run before checker/runtime lanes if syntax semantics are stable. | unreviewed |
| L04 | Schema/type compiler | May precede checker lanes when schema facts change. | unreviewed |
| L05 | Project config/discovery | Disjoint from language/checker/runtime except shared CLI fixtures. | unreviewed |
| L06 | Checker core | Hotspot; sequence with checker evolution and presence when touching shared facts. Shared checker test support is serialized here unless a lane splits it first. | unreviewed |
| L07 | Checker evolution | Soundness-critical; never integrate on first green. Sequence through L06 before editing shared checker test support. | unreviewed |
| L08 | Checker presence | Soundness-critical; sequence with checker core if fact shapes move. Sequence through L06 before editing shared checker test support. | unreviewed |
| L09 | Checker tooling facts | Coordinate with CLI/tooling lane for public surfaces. | unreviewed |
| L10 | Runtime core/read/write | Hotspot; sequence with runtime evolution when store/write facts overlap. | unreviewed |
| L11 | Runtime evolution | Soundness-critical; coordinate with checker evolution and store lanes. | unreviewed |
| L12 | Store/backend/value encoding | Storage durability lane; requires conformance coverage. | unreviewed |
| L13 | Backup/restore | Coordinate with store lane for archive and portability contracts. Sequence through L14 before editing shared CLI test support. | unreviewed |
| L14 | CLI, LSP, serve, data tooling | Downstream of checked/runtime facts; owns shared CLI test support unless a lane splits it first. | unreviewed |

## Review Gate Template

Every lane must record:

- Base/head commits and changed files.
- Failing check or identified failing coverage before behavior changes.
- Focused verification and full gate output with explicit target dir.
- Soundness reviewer verdict and findings.
- Idiom/spec reviewer verdict and findings.
- Fixes for every finding and re-review verdicts.
- Absence/sibling scan over the whole owned area.
- Integration evidence after rebasing/cherry-picking onto live `main`.

Per-lane evidence records must include these fields before a lane can be marked complete: `changed files`, `failing-or-focused check`, `focused gates`, `full gates`, `soundness verdict`, `idiom/spec verdict`, `finding IDs`, `fix evidence`, `re-review verdict`, `absence scan`, `base/head`, `integration command`, and `post-integration status`.

## Initial Global Scan Evidence

Commands were run from `/Users/scottwilliams/Dev/marrow` at audit start, from `/Users/scottwilliams/Dev/marrow-rust-hardening-tracker` during L00 bootstrap, or from current main during L00 integration. Exact command text is kept here so evidence can be reproduced.

- Tracked file count at audit start:
  `git ls-files | wc -l`
  Result: `279`.
- Tracked file count after integrating the tracker on current main:
  `git ls-files | wc -l`
  Result: `277`.
- Main status at audit start:
  `git status --short --branch`
  Result: `## main...origin/main`.
- Live main integration status:
  `git -C /Users/scottwilliams/Dev/marrow status --short --branch`
  Result: run immediately before each lane integration and record in that lane's evidence packet.
- Live main integration head:
  `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD`
  Result: run immediately before each lane integration and record in that lane's evidence packet.
- Unsafe Rust:
  `rg -n "\bunsafe\b" -g "*.rs"`
  Result: no matches; command exited 1.
- Area counts:
  `git ls-files | awk -F/ '{ if ($1 == "crates") area=$1"/"$2; else if ($1 == "docs") area=$1"/"$2; else area=$1 } { count[area]++ } END { for (area in count) print count[area], area }' | sort -k2,2`
  Result: largest areas are `crates/marrow-run` 73, `crates/marrow-check` 66, `crates/marrow` 45; integrated tracker adds `docs/roadmap` 1.
- Oversized files:
  `git ls-files -z | xargs -0 wc -l | sort -nr | sed -n '1,60p'`
  Result: top files include `crates/marrow-run/tests/eval.rs` 11097, `crates/marrow-check/tests/project.rs` 6540, `crates/marrow-check/tests/evolution_discharge.rs` 5710, `crates/marrow-check/src/checks.rs` 3645, `crates/marrow-run/tests/evolution_apply.rs` 3269.
- Broad dispatch evidence:
  `rg -c "\bmatch\b" crates -g "*.rs" | sort -t: -k2,2nr | sed -n '1,10p'`
  Result: highest counts are `crates/marrow-check/tests/project.rs` 80, `crates/marrow-syntax/src/parse_decl.rs` 71, `crates/marrow-check/src/checks.rs` 58.
- Public raw/helper surface:
  `rg -n "pub\s+(fn|struct|enum|mod|type|trait|const)\b[^\n]*(raw|fallback|legacy|prototype|bridge|helper|any|Raw|Fallback|Legacy|Prototype|Bridge|Helper|Any)|pub\([^)]*\)\s+(fn|struct|enum|mod|type|trait|const)\b[^\n]*(raw|fallback|legacy|prototype|bridge|helper|any|Raw|Fallback|Legacy|Prototype|Bridge|Helper|Any)" crates -g "*.rs" | wc -l`
  Result: `36` lines. Production-looking hits include `crates/marrow-run/src/store.rs:161 pub(crate) fn raw_catalog_id` and `crates/marrow-store/src/backup.rs` raw archive constructors; test-source hits require lane triage.
- Message/prose assertions:
  `rg -n "message\.contains|\.message\.contains|stderr\.contains|stdout\.contains|error\.to_string\(\)\.contains|assert!\([^\n]*contains" crates -g "*.rs" | wc -l`
  Result: `410` lines.
- Source-text architecture scans:
  `rg -n "include_str!|read_to_string|match_indices|forbidden|architecture|source-text|source text|scan" crates/*/tests crates/*/src -g "*.rs" | wc -l`
  Result: `534` lines; known policy-scan files include `crates/marrow-run/tests/architecture.rs`, `crates/marrow-check/tests/presence_architecture.rs`, and `crates/marrow/tests/tooling_architecture.rs`.
- Comment sediment terms:
  `rg -n "\bTODO\b|\bFIXME\b|\blegacy\b|\bprototype\b|\bmigration\b|\btemporary\b|\bcompatibility\b|\bshim\b|\bbridge\b|\bpreviously\b|\bnow\b" AGENTS.md CLAUDE.md README.md docs crates -g "*.rs" -g "*.md" -g "*.mw" -g "*.toml" -g "*.json" | wc -l`
  Result: `196` lines.
- Duplicate classifier/name-family scan:
  `rg -n "\b(classify|is_builtin|builtin|saved_root|identity|store_key|catalog_id|raw_path|DataPath|SavedPath|RuntimePath|PathSegment|StoreKey|CatalogId)\b" crates -g "*.rs" | wc -l`
  Result: `2361` broad hits; this is a triage input, not proof of a bug.

## Initial Findings

- F001 oversized test suites: `crates/marrow-run/tests/eval.rs`, `crates/marrow-check/tests/project.rs`, `crates/marrow-check/tests/evolution_discharge.rs`, `crates/marrow-run/tests/evolution_apply.rs`, and `crates/marrow-syntax/tests/parse.rs` are too large for review confidence. Owner lanes must split by invariant or convert to focused fixtures where the area cleanup justifies it.
- F002 broad checker/parser dispatch: `crates/marrow-check/src/checks.rs` and `crates/marrow-syntax/src/parse_decl.rs` have high match density and line counts. Owner lanes must inspect actual Rust shape and split only when it removes real complexity or duplicate semantic ownership.
- F003 prose assertions: `message.contains`, `stderr.contains`, and `stdout.contains` are widespread. Some CLI boundary assertions are legitimate rendering tests; checker/runtime/schema lanes must migrate semantic tests away from prose matching.
- F004 source-text architecture tests: existing source scans are useful backstops but cannot substitute for typed boundaries and positive behavior coverage. Lane owners must keep only identifier-aware scans with a reason and pair them with behavior coverage.
- F005 raw/catalog/helper surfaces: raw archive and catalog helpers may be internal and legitimate, but owner lanes must prove caller, isolation boundary, and absence of production raw saved-path APIs.
- F006 comment sediment: repeated `now`, `legacy`, `migration`, `compatibility`, `bridge`, and `temporary` hits require lane-local review. Domain terms may stay only when they describe durable semantics.

## Absence Ledger

| Pattern | Current Evidence | Status | Owner |
|---|---|---|---|
| `unsafe` Rust | `rg -n "\\bunsafe\\b" -g "*.rs"` returned no matches. | reviewed-clean | L00 |
| Prototype paths | Global term scan has docs/test hits; no production judgment yet. | needs-lane | all lanes |
| Duplicate semantic classifiers | Targeted scan found classifier families in checker/runtime; owner lanes must prove one semantic owner. | needs-lane | L06-L11 |
| Public raw/string APIs | Raw/catalog/archive hits require production-boundary review. | needs-lane | L10, L12, L13, L14 |
| Fallback branches and legacy modes | Term scan has test/doc hits; owner lanes must distinguish domain examples from compatibility glue. | needs-lane | all lanes |
| Message-parsing logic | L03 syntax, L04 schema, and L05 project-model have no `message.contains` semantic assertions after integration; remaining areas still need lane-local migration. | needs-lane | L06-L14 |
| Source-text architecture scans | Existing scans identified in architecture tests. | needs-lane | L08, L10, L14 |
| Comment sediment | L03 syntax hits were triaged as durable `rename ... now spelled` semantics and `now` sample text; L04 schema hits were triaged as `clock.now` domain text and a pre-existing `string`/`Str` bridge comment; L05 project-model hits were triaged as durable store-key migration wording and a `SystemTime::now()` false positive. | needs-lane | L00-L02, L06-L14 |
| Cargo target isolation | Future lane commands must spell lane-specific `CARGO_TARGET_DIR`. | needs-lane | all lanes |
| Cargo.lock churn | No lockfile change at audit start. | reviewed-clean | L00 |

## Lane Status Ledger

| Lane | Worktree | Target Dir | Base | Head | Status | Gates | Soundness | Idiom/Spec | Findings/Fixes | Absence/Integration |
|---|---|---|---|---|---|---|---|---|---|---|
| L00 tracker bootstrap | `/Users/scottwilliams/Dev/marrow-rust-hardening-tracker` | not needed for doc-only bootstrap | `7435c7dbd6ae9817460d5d44ebaa0e54c0aa9b70` | lane `7b04e4876c5927a1f5599d30bbb28f4f2ec4ce75`; main `9415b37635bfde9d42437bca3862f5db92d5fb9d` | complete | staged and post-cherry-pick diff checks clean; inventory checks clean | pass, no findings | pass, no findings | R001-R006 fixed and re-reviewed | integrated on main after live-main recheck |
| L01 language-docs | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L02 docs-meta | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L03 syntax | `/Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration` | `14bbe00fe0be30f741727b8a65da0ceb8bc4d403` | lane `2a961360cd428eb772b65fbf18f6b961b9230ef7`; main `0627dab32fd19a66edb14d0a960afd3fb36fb779` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | fail on typed reason probes, then pass after fixes | fail on broad reason/test-shape findings, then pass after fixes | L03-R001 through L03-R003 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L04 schema | `/Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration` | `5ca2a691806d963c5b44cef8a1eb02ac1b5da7e4` | lane `8b651049860539650ca534820cd3ca03711dd03d`; main `ee5422fe7de568a874ed2b2b4aaee6f9a721a7d8` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | L04-P001 fixed before review; no review findings | integrated on main after live-main recheck; tracker evidence recorded |
| L05 project-model | `/Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration` | `49556121dc4648dec8cd7e11692a4d85cdaf6d7e` | lane `5623e86632a0a62b29c02ad2d104ef1d5969d028`; main `aac2638f1430a3a85a4a7c98a1490b6b1ea7a28c` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | fail on object-shape probe, then pass after fix | pass, no findings; pass after re-review | L05-R001 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L06 checker-core | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L07 checker-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l07-checker-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L08 checker-presence | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l08-checker-presence` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L09 checker-tooling | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l09-checker-tooling` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L10 runtime-core | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L11 runtime-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l11-runtime-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L12 store | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L13 backup-restore | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L14 cli-tools-server | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server` | pending | pending | unreviewed | pending | pending | pending | pending | pending |

## File Inventory

### root
- `.gitignore` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `AGENTS.md` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `CLAUDE.md` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `Cargo.lock` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `Cargo.toml` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `LICENSE-APACHE` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `README.md` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.

### crates/marrow-check/core
- `crates/marrow-check/Cargo.toml` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/analysis.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/binding.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/catalog.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/checks.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/durable_path.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/enums.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.

### crates/marrow-check/evolution
- `crates/marrow-check/src/evolution/const_default.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/discharge.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/discharge/absent_source.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/intents.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/leaf_type.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/mod.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/preview.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/transform_reads.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/src/evolution/witness.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.

### crates/marrow-check/core continued
- `crates/marrow-check/src/executable.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/call_target.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/expr.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/place.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/runtime_value.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/stmt.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/executable/syntax_parts.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/facts.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/infer.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/lib.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.

### crates/marrow-check/presence
- `crates/marrow-check/src/presence.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/calls.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/direct.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/effects.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/keys.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/proofs.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/scope.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/target.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/util.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/walk.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/src/presence/writes.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.

### crates/marrow-check/core continued
- `crates/marrow-check/src/program.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/rejected_surface.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/resolve.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/rules.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.

### crates/marrow-check/tooling
- `crates/marrow-check/src/tooling/data/children.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/mod.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/query.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/query_error.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/read.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/render.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/shape.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/traversal.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/data/walk.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/integrity.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/metadata.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.
- `crates/marrow-check/src/tooling/mod.rs` - status: unreviewed; owner: L09 checker-tooling; notes: initial inventory.

### crates/marrow-check/tests and remaining core
- `crates/marrow-check/src/typerules.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/analysis_api.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/binding_index.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/catalog_presence.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/checked_program.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/durable_path.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/evolution_discharge.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/tests/presence_architecture.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/tests/project.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/ranges.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/resource_store_contract.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/support/mod.rs` - status: unreviewed; owner: L06 checker-core; notes: serialized shared checker test support; L07 and L08 must sequence through L06 or split support before editing.
- `crates/marrow-check/tests/v01_fixtures.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.

### crates/marrow-project
- `crates/marrow-project/Cargo.toml` - status: complete; owner: L05 project-model; notes: reviewed-clean; no manifest churn.
- `crates/marrow-project/src/digest.rs` - status: complete; owner: L05 project-model; notes: reviewed-clean by lane gates and absence scans.
- `crates/marrow-project/src/lib.rs` - status: complete; owner: L05 project-model; notes: typed `ConfigErrorKind`, `ConfigPathField`, and `ConfigPathViolation` added; config object-shape hole fixed.
- `crates/marrow-project/tests/config.rs` - status: complete; owner: L05 project-model; notes: semantic config diagnostics assert typed facts instead of `message.contains`; render-only unknown-field text remains exact.
- `crates/marrow-project/tests/discovery.rs` - status: complete; owner: L05 project-model; notes: reviewed-clean; collection membership assertions are data-set assertions, not diagnostic prose parsing.
- `crates/marrow-project/tests/modules.rs` - status: complete; owner: L05 project-model; notes: reviewed-clean by lane gates and absence scans.

### crates/marrow-run/core
- `crates/marrow-run/Cargo.toml` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/activation.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/base64.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/call.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/call_args.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/collection.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/collection/append.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/collection/materialize.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/durable_read.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/entry.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/env.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/error.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.

### crates/marrow-run/evolution
- `crates/marrow-run/src/evolution/admission.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/apply.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/backfill.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/default.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/index.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/proposal.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/receipt.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/retire.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/transform.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/completion/verdict.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/evidence.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/lifecycle.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/mod.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/rebuild.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/transform.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/validate.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.
- `crates/marrow-run/src/evolution/window.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.

### crates/marrow-run/core continued
- `crates/marrow-run/src/exec.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/expr.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/group_write.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/host.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/host_effects.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/index_maintenance.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/lib.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/local_collection.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/loop_exec.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/neighbor.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/path.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/read.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/saved_iter.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/saved_iter/child_layer.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/saved_iter/index.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/saved_iter/root.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/saved_iter/unique.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/statement.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/std_pure.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/args.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/assertions.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/conversion.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/count.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/error_constructor.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/index_lookup.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/math.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/output.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/stdlib/tests.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/store.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/transaction.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/value.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch/delete.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch/field.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch/local.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch/required.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_dispatch/resource.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/src/write_plan.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/tests/architecture.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/tests/eval.rs` - status: unreviewed; owner: L10 runtime-core; notes: initial inventory.
- `crates/marrow-run/tests/evolution_apply.rs` - status: unreviewed; owner: L11 runtime-evolution; notes: initial inventory.

### crates/marrow-schema
- `crates/marrow-schema/Cargo.toml` - status: complete; owner: L04 schema; notes: reviewed-clean; no manifest churn.
- `crates/marrow-schema/src/error.rs` - status: complete; owner: L04 schema; notes: reviewed-clean by lane gates and absence scans.
- `crates/marrow-schema/src/lib.rs` - status: complete; owner: L04 schema; notes: typed `SchemaErrorKind` payloads added for schema diagnostics; duplicate-index render mismatch fixed.
- `crates/marrow-schema/src/stdlib.rs` - status: complete; owner: L04 schema; notes: reviewed-clean; `clock.now` sediment hit is domain text.
- `crates/marrow-schema/tests/compile_enum.rs` - status: complete; owner: L04 schema; notes: enum schema diagnostics assert typed facts instead of prose fragments.
- `crates/marrow-schema/tests/compile_resource.rs` - status: complete; owner: L04 schema; notes: resource/store schema diagnostics assert typed facts instead of `message.contains`; duplicate-index render has exact output coverage.
- `crates/marrow-schema/tests/resolve_type.rs` - status: complete; owner: L04 schema; notes: reviewed-clean; pre-existing `string`/`Str` bridge comment is durable type-spelling rationale.

### crates/marrow-store
- `crates/marrow-store/Cargo.toml` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/backend.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/backup.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/cell.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/conformance.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/decimal.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/key.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/lib.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/mem.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/metadata.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/redb.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/traversal.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/tree.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/src/value.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/tests/redb_store.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/tests/tree_store.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.
- `crates/marrow-store/tests/value_encoding.rs` - status: unreviewed; owner: L12 store; notes: initial inventory.

### crates/marrow-syntax
- `crates/marrow-syntax/Cargo.toml` - status: complete; owner: L03 syntax; notes: reviewed-clean; no manifest churn.
- `crates/marrow-syntax/src/ast.rs` - status: complete; owner: L03 syntax; notes: reviewed-clean; `rename ... now spelled` sediment hit is durable language semantics.
- `crates/marrow-syntax/src/diagnostic.rs` - status: complete; owner: L03 syntax; notes: typed `DiagnosticReason`, lexer reasons, and parser reasons added to the parse diagnostic surface.
- `crates/marrow-syntax/src/format.rs` - status: complete; owner: L03 syntax; notes: reviewed-clean; formatter output string assertions remain render-output coverage.
- `crates/marrow-syntax/src/lexer.rs` - status: complete; owner: L03 syntax; notes: lexer diagnostics now carry typed reasons at emission sites.
- `crates/marrow-syntax/src/lib.rs` - status: complete; owner: L03 syntax; notes: typed diagnostic reason enums re-exported; `now` sample text is durable fixture content.
- `crates/marrow-syntax/src/literal.rs` - status: complete; owner: L03 syntax; notes: reviewed-clean by lane gates and absence scans.
- `crates/marrow-syntax/src/parse_decl.rs` - status: complete; owner: L03 syntax; notes: declaration parser diagnostics now thread typed `ParseDiagnosticReason`/`ExpectedSyntax` from emission sites; no message-to-reason mapper remains.
- `crates/marrow-syntax/src/parse_expr.rs` - status: complete; owner: L03 syntax; notes: expression parser diagnostics now carry typed reasons at emission sites.
- `crates/marrow-syntax/src/token.rs` - status: complete; owner: L03 syntax; notes: reviewed-clean by lane gates and absence scans.
- `crates/marrow-syntax/tests/format.rs` - status: complete; owner: L03 syntax; notes: reviewed-clean; remaining `contains` checks assert formatted output text, not diagnostics.
- `crates/marrow-syntax/tests/lexer.rs` - status: complete; owner: L03 syntax; notes: lexer diagnostic tests assert typed reasons instead of `message.contains`.
- `crates/marrow-syntax/tests/parse.rs` - status: complete; owner: L03 syntax; notes: parser diagnostic tests assert typed reasons instead of `message.contains`; helper-specific expected reasons and keyed-var key-list errors covered.

### crates/marrow
- `crates/marrow/Cargo.toml` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/backup/archive.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/backup/create.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/backup/mod.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/backup/restore.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/cmd_backup.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/cmd_check.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data/get.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data/integrity.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/args.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/mod.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/render.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/store.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_fmt.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_restore.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/src/cmd_run.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_test.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/dry_run.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/lsp.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/main.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/mod.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol/codec.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol/cursor.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol/data.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol/tests.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/protocol/walk.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/trace.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/backup_cli.rs` - status: unreviewed; owner: L13 backup-restore; notes: initial inventory.
- `crates/marrow/tests/check_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/check_project_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/data_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/dry_run_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/evolve_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/fmt_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/lsp_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/run_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/serve_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/support/mod.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: serialized shared CLI test support; L13 must sequence through L14 or split support before editing.
- `crates/marrow/tests/test_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/tooling_architecture.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/trace_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/usage_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/v01_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.

### docs/root
- `docs/README.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/backend-contract.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/cli.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/data-evolution.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/data-modeling.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/data-tools.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/error-codes.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.

### docs/future
- `docs/future/README.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/backend-contract.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/cli.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/data-evolution.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/data-modeling.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/data-tools.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/error-codes.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/implementation.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/language/builtins.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/language/control-flow-and-effects.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/language/modules-functions.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/language/resources-and-storage.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/language/standard-library.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/future/serve-protocol.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.

### docs/root continued
- `docs/implementation.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/install.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.

### docs/language
- `docs/language/README.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/builtins.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/control-flow-and-effects.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/cost-model.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/enums.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/grammar.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/modules-functions.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/resources-and-storage.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/sample.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/standard-library.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/syntax.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.
- `docs/language/types.md` - status: unreviewed; owner: L01 language-docs; notes: initial inventory.

### docs/root continued
- `docs/lsp.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/project-config.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/quickstart.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/serve-protocol.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.
- `docs/tooling-surfaces.md` - status: unreviewed; owner: L02 docs-meta; notes: initial inventory.

### docs/roadmap
- `docs/roadmap/rust-hardening-file-audit.md` - status: complete; owner: L00 tracker bootstrap; notes: creates the operational source of truth.

### root continued
- `fixtures/v01/library.mw` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.
- `rust-toolchain.toml` - status: unreviewed; owner: L00 root-fixtures; notes: initial inventory.

## Backlog

- B001: For each lane, replace semantic prose assertions with typed assertions where the owner area exposes a stable semantic value. Blocking reason: must be done with local API shape knowledge, not by a global mechanical edit.
- B002: For each architecture/source scan, either replace it with a type-boundary or keep it identifier-aware with positive behavior coverage. Blocking reason: requires lane-local ownership of the invariant.
- B003: Decide whether oversized tests should split by fixture, harness layer, or invariant. Blocking reason: each test suite touches different semantics and must be owned by its lane.
- B004: Public raw/catalog/archive helper review. Blocking reason: storage and backup lanes must prove caller, isolation boundary, and absence of production raw saved-path APIs.

## L00 Tracker Bootstrap Evidence

- Changed files: `docs/roadmap/rust-hardening-file-audit.md`.
- Lane commit: `7b04e4876c5927a1f5599d30bbb28f4f2ec4ce75`.
- Main integration commit: `9415b37635bfde9d42437bca3862f5db92d5fb9d`.
- Main integration base: `16f105e632ae05ebb7f7a44fd3f1b6e022efcdaa`.
- Focused gates:
  - `git diff --cached --check` passed with no output.
  - `comm -3 <(git ls-files | sort) <(sed -n 's/^- \`\([^`]*\)\` - status:.*/\1/p' docs/roadmap/rust-hardening-file-audit.md | sort)` passed with no output.
- Full gates: not run; L00 is a staged documentation bootstrap with no Rust code changes.
- Soundness review: pass after re-review; no findings.
- Idiom/spec review: pass after re-review; no findings.
- Fixed review findings:
  - R001: Removed the drifting lane file-count column.
  - R002: Replaced scan command placeholders with exact commands and concrete results.
  - R003: Added per-lane evidence fields and expanded the lane status ledger.
  - R004: Marked shared checker and CLI test support files as serialized dependencies.
  - R005: Removed volatile latest-main state from durable tracker state and made live-main recheck an integration gate.
  - R006: Reran current-main scan counts and updated stale post-integration totals after live-main file removals.
- Absence scan: `rg -n "\bunsafe\b" -g "*.rs"` returned no matches at audit start.
- Integration state: integrated on main; future lanes still must recheck live main immediately before their own integration.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` before cherry-pick: clean main at `16f105e632ae05ebb7f7a44fd3f1b6e022efcdaa`.
  - `git cherry-pick -x 7b04e4876c5927a1f5599d30bbb28f4f2ec4ce75` produced `9415b37635bfde9d42437bca3862f5db92d5fb9d`.
  - `git diff --check HEAD^..HEAD` passed with no output.
  - Main bidirectional inventory check passed with no output.
  - `git status --short --branch` after cherry-pick reported clean main ahead by one commit.

## L03 Syntax Evidence

- Changed files: `crates/marrow-syntax/src/diagnostic.rs`, `crates/marrow-syntax/src/lexer.rs`, `crates/marrow-syntax/src/lib.rs`, `crates/marrow-syntax/src/parse_decl.rs`, `crates/marrow-syntax/src/parse_expr.rs`, `crates/marrow-syntax/tests/lexer.rs`, `crates/marrow-syntax/tests/parse.rs`.
- Lane commit: `2a961360cd428eb772b65fbf18f6b961b9230ef7`.
- Main integration commit: `0627dab32fd19a66edb14d0a960afd3fb36fb779`.
- Main integration base: `14bbe00fe0be30f741727b8a65da0ceb8bc4d403`.
- Failing-or-focused checks:
  - Initial RED: focused lexer test failed because `DiagnosticReason`, `LexerDiagnosticReason`, and `Diagnostic.reason` did not exist.
  - Pre-review correction RED: source scan found a blocking `parse_reason_for_message` post-hoc classifier, which was removed.
  - Review-fix RED: focused parse test failed on missing narrow `ExpectedSyntax` variants before helper-site reasons were tightened.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml -p marrow-syntax --test parse` passed with 126 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml -p marrow-syntax --test lexer` passed with 19 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml -p marrow-syntax` passed, including 10 unit, 29 format, 19 lexer, 126 parse, and doctests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml -p marrow-syntax --all-targets -- -D warnings` passed.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: failed first review on keyed `var` key-list errors collapsing typed parse errors into `Expected(Statement)`, and on helper errors using broad reasons for known enum/resource/root/index invariants. Passed re-review after typed error propagation and narrower reasons were added; reviewer probes confirmed `EmptyKeyParameters`, `Expected(KeyType)`, `Expected(ResourceName)`, `Expected(SavedRootBeginning)`, `Expected(StoreRoot)`, `Expected(EnumName)`, `Expected(IndexName)`, and `Expected(IndexTail)`.
- Idiom/spec review: failed first review on broad helper-site `Expected(Declaration)` reasons and a key-parameter reserved-word test that asserted only some parse error. Passed re-review after adding closed variants and typed test assertions.
- Fixed review findings:
  - L03-R001: Keyed `var` key-list errors now propagate typed `ParseError` values instead of falling through to `Expected(Statement)`.
  - L03-R002: Helper sites that know exact enum/resource/root/store/index/function invariants now use narrow `ExpectedSyntax` variants; `Expected(Declaration)` remains only top-level recovery.
  - L03-R003: Reserved-word key-parameter test now asserts `Expected(KeyName)`.
- Absence and sibling scans:
  - `rg -n "message\.contains|\.message\.contains|error\.message\.contains|parse_reason_for_message|Result<[^>]*&'static str>|Err\(\"" crates/marrow-syntax -g '*.rs'` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-syntax -g '*.rs'` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-syntax/Cargo.toml` returned no output.
  - `rg -n 'Expected\(ExpectedSyntax::Declaration\)' crates/marrow-syntax/src/parse_decl.rs crates/marrow-syntax/tests/parse.rs` found only the two reviewed top-level recovery sites.
  - `rg -n '\bTODO\b|\bFIXME\b|\blegacy\b|\bprototype\b|\bmigration\b|\btemporary\b|\bcompatibility\b|\bshim\b|\bbridge\b|\bpreviously\b|\bnow\b' crates/marrow-syntax -g '*.rs' -g '*.toml'` found only durable `rename ... now spelled` semantics and `now` sample text.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` before cherry-pick showed main aligned with `origin/main` and an unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git cherry-pick -x 2a961360cd428eb772b65fbf18f6b961b9230ef7` produced `0627dab32fd19a66edb14d0a960afd3fb36fb779`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` after cherry-pick showed main ahead by one commit with the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - Main post-integration scans matched the lane absence scans and `git diff --check` returned no output.

## L04 Schema Evidence

- Changed files: `crates/marrow-schema/src/lib.rs`, `crates/marrow-schema/tests/compile_enum.rs`, `crates/marrow-schema/tests/compile_resource.rs`.
- Lane commit: `8b651049860539650ca534820cd3ca03711dd03d`.
- Main integration commit: `ee5422fe7de568a874ed2b2b4aaee6f9a721a7d8`.
- Main integration base: `5ca2a691806d963c5b44cef8a1eb02ac1b5da7e4`.
- Failing-or-focused checks:
  - Initial RED: `compile_resource` failed on missing `SchemaErrorKind`, `SchemaSavedUnknownTarget`, and `SchemaError.kind`.
  - Pre-review sibling RED: `duplicate_index_name_is_an_error` failed because duplicate index names rendered as `duplicate resource member` while the typed target was `SchemaDuplicateTarget::Index`.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml -p marrow-schema --test compile_resource` passed with 79 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml -p marrow-schema --test compile_enum` passed with 14 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml -p marrow-schema` passed with 106 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml -p marrow-schema -- -D warnings` passed.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer probed schema error construction coverage, duplicate member/key/index/enum paths, saved unknowns, unsupported map contexts, identity-key/index collisions, index argument typing, `Type` payloads, Cargo metadata, and unsafe usage.
- Idiom/spec review: pass, no findings. Reviewer inspected touched Rust for catch-all public enums, duplicate semantic classifiers, compatibility glue, comment sediment, test helper bloat, and retained message-fragment semantics.
- Fixed review findings:
  - L04-P001: Fixed before review. `SchemaDuplicateTarget` now owns its duplicate noun, and duplicate store indexes render `duplicate index` while preserving `SCHEMA_DUPLICATE_MEMBER` and typed `DuplicateMember { target: Index, name }`.
- Absence and sibling scans:
  - `rg -n 'message\.contains|\.message\.contains|error\.message\.contains' crates/marrow-schema -g '*.rs'` returned no matches.
  - `rg -n '\bunsafe\b' -g '*.rs'` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-schema/Cargo.toml` returned no output.
  - `rg -n '\bTODO\b|\bFIXME\b|\blegacy\b|\bprototype\b|\bmigration\b|\btemporary\b|\bcompatibility\b|\bshim\b|\bbridge\b|\bpreviously\b|\bnow\b' crates/marrow-schema -g '*.rs' -g '*.toml'` found only `clock.now` domain text and a pre-existing `string`/`Str` bridge comment in `resolve_type.rs`.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` before cherry-pick showed main aligned with `origin/main` and an unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git cherry-pick -x 8b651049860539650ca534820cd3ca03711dd03d` produced `ee5422fe7de568a874ed2b2b4aaee6f9a721a7d8`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` after cherry-pick showed main ahead by one commit with the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - Main post-integration scans matched the lane absence scans and `git diff --check` returned no output.

## L05 Project Model Evidence

- Changed files: `crates/marrow-project/src/lib.rs`, `crates/marrow-project/tests/config.rs`.
- Lane commit: `5623e86632a0a62b29c02ad2d104ef1d5969d028`.
- Main integration commit: `aac2638f1430a3a85a4a7c98a1490b6b1ea7a28c`.
- Main integration base: `49556121dc4648dec8cd7e11692a4d85cdaf6d7e`.
- Failing-or-focused checks:
  - Initial RED: config tests failed on missing `ConfigErrorKind`, `ConfigPathField`, `ConfigPathViolation`, and `ConfigError.kind`.
  - Review-fix RED: `rejects_non_object_config_shapes` failed because `parse_config("[]")` returned `MissingSourceRoots` instead of `InvalidJson`.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml -p marrow-project --test config` passed with 12 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml -p marrow-project` passed with 33 tests.
- Full lane gates:
  - Evidence addendum on 2026-06-05 reran the formatter gate at lane head `5623e86632a0a62b29c02ad2d104ef1d5969d028`: `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: failed first review on top-level and nested `run`/`store` non-object JSON shapes; passed re-review after the family fix. External probe covered top-level null/scalar/array, `run`/`store` null/scalar/array, unknown fields, missing/empty `sourceRoots`, native `dataDir`, backend, and path violation cases.
- Idiom/spec review: pass, no findings; pass after re-review.
- Fixed review findings:
  - L05-R001: Rejected non-object top-level config values and non-object present `run`/`store` values as `ConfigErrorKind::InvalidJson`, while keeping serde `deny_unknown_fields` as the unknown-key owner.
- Absence and sibling scans:
  - `rg -n 'error\.message\.contains|message\.contains|\.message\.contains|UnknownField|reject_unknown_fields|reject_unknown_object|unknown_field_message' crates/marrow-project -g '*.rs'` returned no matches.
  - `rg -n '\bunsafe\b' -g '*.rs'` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-project/Cargo.toml` returned no output.
  - `rg -n '\bTODO\b|\bFIXME\b|\blegacy\b|\bprototype\b|\bmigration\b|\btemporary\b|\bcompatibility\b|\bshim\b|\bbridge\b|\bpreviously\b|\bnow\b' crates/marrow-project -g '*.rs' -g '*.toml'` found only durable store-key migration wording and a `SystemTime::now()` false positive.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` before cherry-pick showed main aligned with `origin/main` and an unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git cherry-pick -x 5623e86632a0a62b29c02ad2d104ef1d5969d028` produced `aac2638f1430a3a85a4a7c98a1490b6b1ea7a28c`.
  - Evidence addendum on 2026-06-05 reran the formatter gate at main integration commit `aac2638f1430a3a85a4a7c98a1490b6b1ea7a28c`: `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l05-main-replay/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
