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
- Current tracked file count after L01 language-docs audit: 274
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
| Prototype paths | Completed L00-L05, L12, and L13 lanes found no retained prototype paths in their owned areas; remaining hits require lane-local review. | needs-lane | L06-L11, L14 |
| Duplicate semantic classifiers | Targeted scan found classifier families in checker/runtime; owner lanes must prove one semantic owner. | needs-lane | L06-L11 |
| Public raw/string APIs | L12 store raw archive constructors are `pub(crate)` typed backup boundaries or test-gated constructors; redb raw byte checks are native substrate tests. L13 backup/restore raw cell helpers are test-only malformed-archive constructors; production archive errors now carry typed payloads. Other raw/catalog hits require production-boundary review. | needs-lane | L10, L14 |
| Fallback branches and legacy modes | L00 root-fixtures hits are AGENTS policy prohibitions. L01 language-doc hits are v0.1/reserved boundary text rather than compatibility fallback behavior. L12 store hits are version-refusal and table-initialization comments or tests rejecting legacy manifest spellings. L13 legacy digest hits are rejection tests for old digest spelling, not compatibility behavior. Other term scan hits require lane-local review. | needs-lane | L06-L11, L14 |
| Message-parsing logic | L03 syntax, L04 schema, L05 project-model, L12 store, and L13 backup/restore have no `message.contains` semantic assertions after integration. L06 schema-payload, duplicate-root, duplicate-declaration, duplicate-module, module-path, rejected-surface, schema-unsupported-map, enum-payload, parent-not-category, script-import, private-enum, duplicate-named-argument, append-target, conversion-source, interpolation-source, type-mismatch, and reserved-catalog payload slices migrated the checker assertions they touched. L10 runtime throw-field slice removed `message.contains` assertions from `crates/marrow-run/tests/eval.rs`; remaining checker/runtime/tooling areas still need lane-local migration. | needs-lane | L06-L11, L14 |
| Source-text architecture scans | Existing scans identified in architecture tests. | needs-lane | L08, L10, L14 |
| Comment sediment | L00 root-fixtures hits are durable AGENTS policy prohibitions and repository operating rules. L01 language-doc hits were triaged as durable `migration DSL` negative scope, `std::clock::now()` examples, and `rename ... now spelled` evolution wording. L02 removed empty future placeholder pages; remaining L02 hits were triaged as durable data-evolution compatibility/migration contracts, `std::clock::now`, old path aliases, bridge wording for host-system extensions, and protocol cursor text. L03 syntax hits were triaged as durable `rename ... now spelled` semantics and `now` sample text; L04 schema hits were triaged as `clock.now` domain text and a pre-existing `string`/`Str` bridge comment; L05 project-model hits were triaged as durable store-key migration wording and a `SystemTime::now()` false positive; L12 store hits were durable redb format/version comments, native substrate raw-byte tests, and internal byte-decoder variable names; L13 backup/restore hits were durable legacy-digest rejection tests, raw-engine-copy contract docs, and test/output wording. | needs-lane | L06-L11, L14 |
| Cargo target isolation | Completed lanes spell lane-specific `CARGO_TARGET_DIR`; future lane commands must keep doing so. | needs-lane | L06-L11, L14 |
| Cargo.lock churn | No lockfile change at audit start. | reviewed-clean | L00 |

## Lane Status Ledger

| Lane | Worktree | Target Dir | Base | Head | Status | Gates | Soundness | Idiom/Spec | Findings/Fixes | Absence/Integration |
|---|---|---|---|---|---|---|---|---|---|---|
| L00 tracker/root-fixtures | tracker `/Users/scottwilliams/Dev/marrow-rust-hardening-tracker`; root `/Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures` | root lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures` | tracker `7435c7dbd6ae9817460d5d44ebaa0e54c0aa9b70`; root `d528f5e9a9e281c8076145a2b734976de3d8a12e` | tracker lane `7b04e4876c5927a1f5599d30bbb28f4f2ec4ce75`; root no source commit; tracker main `9415b37635bfde9d42437bca3862f5db92d5fb9d` | complete | tracker bootstrap checks, root focused checks, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | R001-R006 fixed and re-reviewed; no root review findings | integrated on main after live-main recheck; root evidence recorded |
| L01 language-docs | `/Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-review-soundness` | `76fc2843238992766aa04be31d91596f82641964` | no source commit; tracker evidence recorded | complete | focused language-doc scans, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | no review findings; no semantic proposals | no source cherry-pick required; tracker evidence recorded |
| L02 docs-meta | `/Users/scottwilliams/Dev/marrow-rust-hardening-l02-docs-meta` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-main-integration` | `3032e90a4e274fbcce91a3b3ebdd948643948e48` | lane `3dd44fa8989af9e5dc1599e22caadbb02b42d851`; main `fe34e8695dae03f2d9fb1e857a22482e63edb6ab` | complete | focused docs scans, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | no review findings | integrated on main after live-main recheck; tracker evidence recorded |
| L03 syntax | `/Users/scottwilliams/Dev/marrow-rust-hardening-l03-syntax` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-syntax`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l03-main-integration` | `14bbe00fe0be30f741727b8a65da0ceb8bc4d403` | lane `2a961360cd428eb772b65fbf18f6b961b9230ef7`; main `0627dab32fd19a66edb14d0a960afd3fb36fb779` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | fail on typed reason probes, then pass after fixes | fail on broad reason/test-shape findings, then pass after fixes | L03-R001 through L03-R003 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L04 schema | `/Users/scottwilliams/Dev/marrow-rust-hardening-l04-schema` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-schema`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l04-main-integration` | `5ca2a691806d963c5b44cef8a1eb02ac1b5da7e4` | lane `8b651049860539650ca534820cd3ca03711dd03d`; main `ee5422fe7de568a874ed2b2b4aaee6f9a721a7d8` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | L04-P001 fixed before review; no review findings | integrated on main after live-main recheck; tracker evidence recorded |
| L05 project-model | `/Users/scottwilliams/Dev/marrow-rust-hardening-l05-project-model` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-project-model`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l05-main-integration` | `49556121dc4648dec8cd7e11692a4d85cdaf6d7e` | lane `5623e86632a0a62b29c02ad2d104ef1d5969d028`; main `aac2638f1430a3a85a4a7c98a1490b6b1ea7a28c` | complete | focused, package, workspace build/test, workspace clippy, and fmt gates passed | fail on object-shape probe, then pass after fix | pass, no findings; pass after re-review | L05-R001 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L06 checker-core | `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`; schema reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-1` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-1`; duplicate-root reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-2` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-2`; duplicate-declaration reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-3` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-3`; duplicate-module reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-4` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-4`; module-path reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-5` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-5`; rejected-surface reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-6` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-6`; schema unsupported-map reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-7` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-7`; enum-payload reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-8` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-8`; parent-not-category reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-9` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-9`; script-import reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-10` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-10`; private-enum reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-11` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-11`; duplicate-named-argument reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-12` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-12`; append-target reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-13` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-13`; conversion-source reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-14` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-14`; interpolation-source reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-15` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-15`; type-mismatch reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-16` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-16`; reserved-catalog reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-17` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-17`; main targets recorded per slice | first slice base `580f92ad684840c30833dc025d3a908a5aaadc2c`; latest slice base `6d9a8b29a4359818cbd379e8e0c6189ab93169e3`; latest integration base `6d9a8b29a4359818cbd379e8e0c6189ab93169e3` | latest lane and main source `28afaf1d56f27dc2913f688a0e203fec15a43c75` | in-lane | focused schema payload, duplicate-root, duplicate-declaration, duplicate-module, module-path, rejected-surface, schema unsupported-map, enum payload, parent-not-category, script-import, private-enum, duplicate-named-argument, append-target, conversion-source, interpolation-source, type-mismatch, and reserved-catalog tests, package test, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings for integrated slices | pass after reserved-catalog test-shape fix; otherwise no findings for integrated slices | reserved-catalog R001 fixed and re-reviewed; no open review findings | schema diagnostic payload, duplicate-root owner payload, duplicate-declaration payload, duplicate-module payload, module-path payload, rejected-surface payload, schema unsupported-map assertion, enum diagnostic payload, parent-not-category assertion, script-import assertion, private-enum payload, duplicate-named-argument payload, append-target payload, conversion-source payload, interpolation-source payload, type-mismatch payload, and reserved-catalog payload slices integrated; broader L06 files remain in lane |
| L07 checker-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l07-checker-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L08 checker-presence | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l08-checker-presence` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L09 checker-tooling | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l09-checker-tooling` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L10 runtime-core | `/Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core`; base64 canonicality slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source` | base64 lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source`; base64 reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-serve-protocol-source` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-serve-protocol-source`; latest main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration` | initial `f7501f90c77edc95ae08297ac9d39583c79e6cac`; throw-field live rebase base `fe51bc62435437251607f77938150c766bbde7e6`; base64 slice base `451df5e4256dde7a04a1a015930a99e8fa348fdb` | latest source/main `1735ff618513bc54dcdba99ef5e43c216efe1396` | in-lane | focused throw-field, base64, serve-protocol, `marrow-run`, `marrow`, workspace build/test, workspace clippy, and fmt gates passed | base64 canonicality failed on non-zero base64 pad bits, then pass after fix; throw-field pass, no findings | base64 idiom/spec pass after fix; throw-field pass, no findings | base64 pad-bit finding fixed and re-reviewed; no open in-scope base64 findings | runtime throw-field test slice and shared base64 canonical decoder slice integrated; broader L10 files remain unreviewed |
| L11 runtime-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l11-runtime-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L12 store | `/Users/scottwilliams/Dev/marrow-rust-hardening-l12-store` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-soundness` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-idiom`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration` | `e3690d46d5cebb760728dfb20b49cd52d0806c2b` | no source commit; tracker evidence recorded | complete | focused store/default/native checks, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | no review findings | no source cherry-pick required; main integration gates passed |
| L13 backup-restore | `/Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-soundness` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-idiom`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration` | `2215296a4de471bf051e15990158e558b9d51bd6` | lane `fdbc324e025b5cd81b7bd97354544552c8e02bb5`; main `b1f0112ed36908535c0d4ef1dc09f198835134c1` | complete | focused backup tests, workspace build/test, workspace clippy, and fmt gates passed | fail on typed wrong-type manifest payload, then pass after fix | pass, then pass after re-review | L13-R001 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L14 cli-tools-server | `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server`; trace slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli`; usage/v01 slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01`; fmt/test slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli`; LSP slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp`; trace-source slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source`; cmd-fmt source slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source`; dry-run source blocker slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source`; serve-protocol/base64 slice `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source` | latest lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source`; latest reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-serve-protocol-source` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-serve-protocol-source`; latest main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration` | initial CLI slice `13852686ed8317e1b567f941ff160de345738d3b`; trace CLI source `4a3e473bbf6a9465c46e8d0fd05f9966e1fe2f1a`; usage/v01 no-source base `d732ad3a0d395a713f741c8882e0111748763284`; fmt/test source base `c1e97e1986d736377e0b0e4e2e2e146c5d2747e5`; LSP source base `3d4929a2ab199b503f8534526e17260d65f097cb`; trace-source no-source base `de9c2f52a5e72d1c808c876ef9cf2c6952a20d62`; cmd-fmt source base `103d39a446ecb26d6e4b4d6c051b272c5b02ad15`; dry-run source blocker base `40767f054cb522af74062d9ac27665a13225715a`; serve-protocol/base64 base `451df5e4256dde7a04a1a015930a99e8fa348fdb` | latest source/main `1735ff618513bc54dcdba99ef5e43c216efe1396`; dry-run blocker produced no source commit | in-lane | focused CLI, trace, usage, v0.1, fmt, test, LSP, cmd-fmt, dry-run, serve-protocol, base64, `marrow-run`, `marrow`, workspace build/test, workspace clippy, and fmt gates passed for integrated slices | serve-protocol soundness failed on oversized request-line suffix parsing and non-canonical base64 pad bits; base64 pass after fix and request-line issue deferred to B006; dry-run source failed on mixed program stdout with JSONL tooling records and overstrong byte-for-byte dry-run wording; cmd-fmt source failed on write failures reported as `io.read`, then pass after fix; LSP slice failed on unbounded header-line/header-block parsing and missing checker diagnostic range assertion, then pass after fix; trace-source no-source review pass; otherwise pass, no findings after review-fixes | serve-protocol/base64 idiom/spec pass after fix; dry-run idiom/spec failed on structured target assertions and typed-target doc wording; cmd-fmt idiom/spec pass before and after source fix; trace-source no-source review pass; LSP idiom/spec pass after source fix; fmt/test slice failed on multi-file fmt coverage, OS-prose negative assertion, and low-value comments, then pass after fix; trace slice failed on JSONL order assertion gap, then pass after fix; prior diagnostic slice failed on semantic text assertions and duplicate JSON parsing, then pass after fix | L14-R001, L14-R002, trace-order finding, L14-IDIOM-001 through L14-IDIOM-003, LSP-001, LSP-002, cmd-fmt write-error finding, and base64 pad-bit finding fixed and re-reviewed; dry-run findings deferred to B005; serve request-line finding deferred to B006 because `serve/mod.rs` is staged in the active `marrow-engine-resident-catalog` worktree; usage/v01 and trace-source no-source reviews had no findings | CLI diagnostic test-support, trace JSONL assertion, fmt/test coverage cleanup, LSP header-bound/range, cmd-fmt `io.write`, and shared base64 canonical decoder slices integrated; dry-run source slice retired without integration; serve connection framing blocked; usage/v01 and trace-source reviewed-clean with no source change; untouched L14 sibling source and CLI files remain unreviewed |

## File Inventory

### root
- `.gitignore` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean by root-fixtures audit.
- `AGENTS.md` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean; sediment hits are durable repository policy prohibitions and operating rules.
- `CLAUDE.md` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean one-line pointer to AGENTS.
- `Cargo.lock` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean; locked metadata passed and no lockfile churn.
- `Cargo.toml` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean; workspace metadata resolves and `unsafe_code = "forbid"` remains set.
- `LICENSE-APACHE` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean standard Apache-2.0 text.
- `README.md` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean root product and reference overview.

### crates/marrow-check/core
- `crates/marrow-check/Cargo.toml` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/analysis.rs` - status: in-lane; owner: L06 checker-core; notes: duplicate root owner, duplicate-module, and module-path payload slices integrated; broader checker-core review remains.
- `crates/marrow-check/src/binding.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/catalog.rs` - status: in-lane; owner: L06 checker-core; notes: reserved-catalog payload emitter integrated; broader checker-core review remains.
- `crates/marrow-check/src/checks.rs` - status: in-lane; owner: L06 checker-core; notes: schema diagnostic, duplicate-root, duplicate-declaration, duplicate-module, module-path, rejected-surface, enum, private-enum, duplicate-named-argument, append-target, conversion-source, interpolation-source, type-mismatch, and reserved-catalog payload work integrated, including suppression matches in this file; broader checker-core review remains.
- `crates/marrow-check/src/durable_path.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/enums.rs` - status: in-lane; owner: L06 checker-core; notes: enum match, `is`, and private-enum diagnostic payload emitters integrated; broader checker-core review remains.

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
- `crates/marrow-check/src/infer.rs` - status: in-lane; owner: L06 checker-core; notes: enum, private-enum, and interpolation-source value-position diagnostic payload emitters integrated; broader checker-core review remains.
- `crates/marrow-check/src/lib.rs` - status: in-lane; owner: L06 checker-core; notes: schema diagnostic, duplicate-root, duplicate-declaration, duplicate-module, module-path, rejected-surface, enum, private-enum, duplicate-named-argument, append-target, conversion-source, interpolation-source, type-mismatch, and reserved-catalog payload variants integrated; checker conversion-source facts have one owner; broader checker-core review remains.

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
- `crates/marrow-check/src/rejected_surface.rs` - status: in-lane; owner: L06 checker-core; notes: rejected-surface payload slice integrated; broader checker-core review remains.
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
- `crates/marrow-check/tests/catalog_presence.rs` - status: in-lane; owner: L06 checker-core; notes: reserved catalog path reuse assertion now uses exact diagnostic payload; broader checker-core test cleanup remains.
- `crates/marrow-check/tests/checked_program.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/durable_path.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/evolution_discharge.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/tests/presence_architecture.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/tests/project.rs` - status: in-lane; owner: L06 checker-core; notes: early schema-family, duplicate-root, duplicate-declaration, duplicate-module, module-path, rejected-surface, schema unsupported-map, enum/member-path, parent-not-category, script-import, private-enum, duplicate-named-argument, append-target, conversion-source, interpolation-source, and type-mismatch assertions now use exact diagnostic payloads for the integrated slices; broader checker-core test cleanup remains.
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
- `crates/marrow-run/src/base64.rs` - status: complete; owner: L10 runtime-core; notes: shared base64 decoder rejects unpadded, over-padded, misplaced-padding, invalid-character, and non-zero pad-bit spellings while preserving canonical padded forms for `std::bytes` and serve protocol callers.
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
- `crates/marrow-run/tests/eval.rs` - status: in-lane; owner: L10 runtime-core; notes: runtime throw-field slice migrated `RuntimeError.message.contains` assertions to typed Error resource field assertions; oversized suite still requires broader L10 cleanup.
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
- `crates/marrow-store/Cargo.toml` - status: complete; owner: L12 store; notes: reviewed-clean; no manifest churn.
- `crates/marrow-store/src/backend.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed backend contract and `StoreError` codes.
- `crates/marrow-store/src/backup.rs` - status: complete; owner: L12 store; notes: reviewed-clean; raw backup constructors are crate-internal or test-gated and validate typed data-cell targets.
- `crates/marrow-store/src/cell.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed cell-key encoding and child decoders.
- `crates/marrow-store/src/conformance.rs` - status: complete; owner: L12 store; notes: reviewed-clean shared backend conformance coverage.
- `crates/marrow-store/src/decimal.rs` - status: complete; owner: L12 store; notes: reviewed-clean canonical decimal envelope and arithmetic.
- `crates/marrow-store/src/key.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed saved-key encoding boundary.
- `crates/marrow-store/src/lib.rs` - status: complete; owner: L12 store; notes: reviewed-clean public module surface.
- `crates/marrow-store/src/mem.rs` - status: complete; owner: L12 store; notes: reviewed-clean memory backend conformance.
- `crates/marrow-store/src/metadata.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed commit metadata codec.
- `crates/marrow-store/src/redb.rs` - status: complete; owner: L12 store; notes: reviewed-clean native backend, transaction, and raw-byte substrate tests.
- `crates/marrow-store/src/traversal.rs` - status: complete; owner: L12 store; notes: reviewed-clean scan accumulator and paging boundary.
- `crates/marrow-store/src/tree.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed tree facade, backup traversal, metadata, and transaction coverage.
- `crates/marrow-store/src/value.rs` - status: complete; owner: L12 store; notes: reviewed-clean canonical saved-value codec.
- `crates/marrow-store/tests/redb_store.rs` - status: complete; owner: L12 store; notes: reviewed-clean native redb persistence and handle-boundary coverage.
- `crates/marrow-store/tests/tree_store.rs` - status: complete; owner: L12 store; notes: reviewed-clean typed tree-store behavior coverage.
- `crates/marrow-store/tests/value_encoding.rs` - status: complete; owner: L12 store; notes: reviewed-clean canonical value encoding coverage.

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
- `crates/marrow/src/backup/archive.rs` - status: complete; owner: L13 backup-restore; notes: typed archive format/corrupt error payloads added; present wrong-type manifest fields now differ from missing fields.
- `crates/marrow/src/backup/create.rs` - status: complete; owner: L13 backup-restore; notes: manifest binding mismatch tests assert typed corrupt payloads.
- `crates/marrow/src/backup/mod.rs` - status: complete; owner: L13 backup-restore; notes: `BackupError` carries typed `BackupFormatProblem` and `BackupCorruptProblem` payloads while preserving stable dotted codes.
- `crates/marrow/src/backup/restore.rs` - status: complete; owner: L13 backup-restore; notes: checksum and trailing-byte failures carry typed corrupt payloads.
- `crates/marrow/src/cmd_backup.rs` - status: complete; owner: L13 backup-restore; notes: reviewed-clean CLI render boundary; no source change.
- `crates/marrow/src/cmd_check.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data/get.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_data/integrity.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/args.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/mod.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/render.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_evolve/store.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_fmt.rs` - status: complete; owner: L14 cli-tools-server; notes: fmt source reports write failures as typed `io.write`, keeps directory mode on configured source roots, reports every per-file failure before overall failure, and has no unsafe, fallback, compatibility, or prose-driven semantics.
- `crates/marrow/src/cmd_restore.rs` - status: complete; owner: L13 backup-restore; notes: reviewed-clean CLI render boundary; no source change.
- `crates/marrow/src/cmd_run.rs` - status: blocked; owner: L14 cli-tools-server; notes: dry-run source review found `--dry-run --trace --format jsonl` mixes program stdout with JSONL tooling reports; source fix is blocked while `cmd_run.rs` is staged in `/Users/scottwilliams/Dev/marrow-engine-resident-catalog`.
- `crates/marrow/src/cmd_test.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/dry_run.rs` - status: blocked; owner: L14 cli-tools-server; notes: dry-run review found byte-for-byte wording too strong for native redb store files; complete review is blocked with the `cmd_run.rs` stream-separation fix.
- `crates/marrow/src/lsp.rs` - status: complete; owner: L14 cli-tools-server; notes: LSP message reader bounds header lines, header blocks, and bodies; diagnostics stay downstream of checker analysis; no prose matching, unsafe, raw, fallback, or duplicate checker semantics in this slice.
- `crates/marrow/src/main.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/serve/mod.rs` - status: blocked; owner: L14 cli-tools-server; notes: serve-protocol soundness review found oversized request lines can leave the suffix to be parsed as a follow-up request; source fix is blocked while `serve/mod.rs` is staged in `/Users/scottwilliams/Dev/marrow-engine-resident-catalog`.
- `crates/marrow/src/serve/protocol.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean debug/admin protocol dispatcher over typed ops; stale-epoch gate derives from parsed ops and no raw production `data_*` op is accepted.
- `crates/marrow/src/serve/protocol/codec.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean path/key/limit codec delegates bytes and cursor payloads to the shared strict base64 decoder; no duplicate saved-path or storage-locator semantics.
- `crates/marrow/src/serve/protocol/cursor.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean per-session cursor state signs scope and payload, rejects forged or cross-scope cursors uniformly, and exposes only serve-internal surfaces.
- `crates/marrow/src/serve/protocol/data.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean data ops use shared tooling facts for roots, reads, children, paging support, and store/tooling error classification.
- `crates/marrow/src/serve/protocol/tests.rs` - status: complete; owner: L14 cli-tools-server; notes: protocol tests assert structured replies, cursor replay/forgery rejection, stale raw-op absence, canonical base64 behavior, and non-zero pad-bit rejection without prose parsing.
- `crates/marrow/src/serve/protocol/walk.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean walk op resolves queries through shared tooling, enforces typed limit bounds, and derives cursors from checked query paths.
- `crates/marrow/src/trace.rs` - status: complete; owner: L14 cli-tools-server; notes: trace source reviewed clean with no source change; trace rendering stays a presentation boundary over typed runtime/write-target structures, JSON keeps raw bytes in `value_b64`, text streaming preserves run stdout, and retained raw-byte comments explain durable rendering behavior.
- `crates/marrow/tests/backup_cli.rs` - status: complete; owner: L13 backup-restore; notes: semantic restore error-code assertions moved to JSON `code`; remaining `contains` checks are render/effect assertions.
- `crates/marrow/tests/check_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: semantic check diagnostics moved to JSON/JSONL code and span assertions where structured output exists; remaining text checks are render or usage boundaries.
- `crates/marrow/tests/check_project_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: project diagnostics and summaries assert JSONL codes, paths, and status instead of prose fragments.
- `crates/marrow/tests/data_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: data integrity semantic problem assertions use JSON problem records, stable codes, source paths, tooling kind, and serialized JSON leakage checks.
- `crates/marrow/tests/dry_run_cli.rs` - status: blocked; owner: L14 cli-tools-server; notes: dry-run review requires structured target assertions and a `print(...)` plus `--dry-run --trace --format jsonl` regression; completion is blocked on B005.
- `crates/marrow/tests/evolve_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: semantic evolution diagnostics assert JSON codes, catalog IDs, populated counts, repair-required, approval-required, and schema-drift facts.
- `crates/marrow/tests/fmt_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: project-directory fmt tests now cover multiple source files and `--write` failure continuation; no-config directory assertions use stable `io.read` plus `marrow.json`; write failures assert stable `io.write`; retained text assertions are render-boundary checks because `marrow fmt` has no structured output mode.
- `crates/marrow/tests/lsp_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: LSP tests assert framed JSON-RPC fields, diagnostic codes, severity, source, and representative checker ranges; no `contains` or diagnostic prose matching remains.
- `crates/marrow/tests/run_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/serve_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/support/mod.rs` - status: complete; owner: L14 cli-tools-server; notes: shared CLI support provides small JSON/JSONL helpers and production catalog commit fixture helper; function-scoped dead-code allowances are integration-test-crate local.
- `crates/marrow/tests/test_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean ordinary test-result surface; retained text assertions are render-boundary checks because `marrow test --format` shapes trace output only, not pass/fail summaries.
- `crates/marrow/tests/tooling_architecture.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/trace_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: trace JSONL tests assert ordered step/write/summary records, structured write target fields, and structured test trace labels; remaining text assertions are render-boundary checks.
- `crates/marrow/tests/usage_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean usage/rendering boundary coverage; substring assertions are paired with exit code, empty stdout, and no-store-created checks where applicable.
- `crates/marrow/tests/v01_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: reviewed-clean end-to-end v0.1 fixture check through production CLI pipeline with exact final stdout.

### docs/root
- `docs/README.md` - status: complete; owner: L02 docs-meta; notes: future-docs pointer no longer implies a complete placeholder mirror.
- `docs/backend-contract.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean by L02 docs scans and reviewer pass.
- `docs/cli.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; deferred data-tool and restore references point to retained future pages.
- `docs/data-evolution.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; migration, compatibility-window, old-alias, and bridge wording is durable evolution contract text.
- `docs/data-modeling.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; `now` and old-key wording describes current saved-root and data-maintenance semantics.
- `docs/data-tools.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; deferred `diff`/`load` references point to retained future page.
- `docs/error-codes.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; `std::clock::now` and deferred-surface text are durable reference entries.

### docs/future
- `docs/future/README.md` - status: complete; owner: L02 docs-meta; notes: describes selected future surfaces instead of a complete mirror.
- `docs/future/backend-contract.md` - status: deleted; owner: L02 docs-meta; notes: placeholder-only page removed; no in-scope links remained.
- `docs/future/cli.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained restore future surface.
- `docs/future/data-evolution.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; compatibility-window and migration terms are durable future evolution contract text.
- `docs/future/data-modeling.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained custom identity allocation future surface.
- `docs/future/data-tools.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained `data diff` and `data load` future surface.
- `docs/future/error-codes.md` - status: deleted; owner: L02 docs-meta; notes: placeholder-only page removed; no in-scope links remained.
- `docs/future/implementation.md` - status: deleted; owner: L02 docs-meta; notes: placeholder-only page removed; no in-scope links remained.
- `docs/future/language/builtins.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained future builtins surfaces.
- `docs/future/language/control-flow-and-effects.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained future control-flow surface.
- `docs/future/language/modules-functions.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained visibility and parameter-doc future surfaces.
- `docs/future/language/resources-and-storage.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained future resources/storage surfaces.
- `docs/future/language/standard-library.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained future standard-library surfaces.
- `docs/future/serve-protocol.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; records retained future read-surface protocol notes.

### docs/root continued
- `docs/implementation.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; migration and bridge terms are durable implementation and extension boundary text.
- `docs/install.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean by L02 docs scans and reviewer pass.

### docs/language
- `docs/language/README.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; `migration DSL` wording is durable negative scope.
- `docs/language/builtins.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; placeholder/index wording describes durable collection semantics.
- `docs/language/control-flow-and-effects.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; `not yet supported` temporal range wording is an explicit v0.1 boundary.
- `docs/language/cost-model.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean by language-doc scans and reviewer pass.
- `docs/language/enums.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; reserved-word wording is accepted language contract text.
- `docs/language/grammar.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; reserved and future-`~` wording is explicit v0.1 grammar boundary.
- `docs/language/modules-functions.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; `clock::now` examples are durable standard-library usage.
- `docs/language/resources-and-storage.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; merge/lock, placeholder, and rename wording are durable saved-data semantics.
- `docs/language/sample.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; reference sample parses and language-doc examples lex.
- `docs/language/standard-library.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; `std::clock::now()` examples are durable API examples.
- `docs/language/syntax.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; reserved/planned wording describes intentional v0.1 syntax absence.
- `docs/language/types.md` - status: complete; owner: L01 language-docs; notes: reviewed-clean; identity-typed-key wording is explicit current boundary.

### docs/root continued
- `docs/lsp.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; future editor-feature boundaries are durable tooling surface text.
- `docs/project-config.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; old path alias wording is durable catalog contract text.
- `docs/quickstart.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; deferred data-tool references point to retained future page.
- `docs/serve-protocol.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; compatibility and previously-returned cursor wording are durable protocol contract text.
- `docs/tooling-surfaces.md` - status: complete; owner: L02 docs-meta; notes: reviewed-clean; raw saved-path compatibility text is a durable non-production boundary.

### docs/roadmap
- `docs/roadmap/rust-hardening-file-audit.md` - status: complete; owner: L00 tracker bootstrap; notes: creates the operational source of truth.

### root continued
- `fixtures/v01/library.mw` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean durable v0.1 fixture, included by checker and CLI v0.1 tests.
- `rust-toolchain.toml` - status: complete; owner: L00 root-fixtures; notes: reviewed-clean; pinned toolchain matches workspace Rust version.

## Backlog

- B001: For each lane, replace semantic prose assertions with typed assertions where the owner area exposes a stable semantic value. Blocking reason: must be done with local API shape knowledge, not by a global mechanical edit.
- B002: For each architecture/source scan, either replace it with a type-boundary or keep it identifier-aware with positive behavior coverage. Blocking reason: requires lane-local ownership of the invariant.
- B003: Decide whether oversized tests should split by fixture, harness layer, or invariant. Blocking reason: each test suite touches different semantics and must be owned by its lane.
- B004: Public raw/catalog/archive helper review. Blocking reason: storage and backup lanes must prove caller, isolation boundary, and absence of production raw saved-path APIs.
- B005: Re-run the L14 dry-run/cmd-run source lane after `cmd_run.rs` is free. Required fixes: keep program stdout separate from JSON/JSONL trace and dry-run tooling reports, add a `print(...)` plus `--dry-run --trace --format jsonl` regression, narrow dry-run wording/tests from native file byte identity to logical saved data or implement true byte stability, and integrate the structured dry-run target assertions. Blocking reason: `cmd_run.rs` is currently staged in `/Users/scottwilliams/Dev/marrow-engine-resident-catalog`, so the dry-run lane cannot own the required source fix under file-disjoint integration rules.
- B006: Re-run the L14 serve connection-framing lane after `serve/mod.rs` is free. Required fix: when a request line exceeds `MAX_REQUEST_BYTES`, drain through the newline or close the connection so the suffix of the over-limit logical frame cannot be parsed as a second request; add a real serve repro with an oversized line followed by a valid JSON request. Blocking reason: `serve/mod.rs` is currently staged in `/Users/scottwilliams/Dev/marrow-engine-resident-catalog`, so the serve-protocol slice could not own the connection-loop fix under file-disjoint integration rules.

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

## L00 Root-Fixtures Evidence

- Changed files: none; root/manifests/fixture files were reviewed-clean without source edits.
- Lane source commit: none.
- Main source integration commit: none; this evidence update records the audit state.
- Base/head: `/Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures` at `d528f5e9a9e281c8076145a2b734976de3d8a12e`.
- Failing-or-focused checks:
  - No RED/edit cycle was run because the build worker and reviewers found no concrete cleanup to make.
  - Baseline checks inspected all owned files: `.gitignore`, `AGENTS.md`, `CLAUDE.md`, `Cargo.lock`, `Cargo.toml`, `LICENSE-APACHE`, `README.md`, `fixtures/v01/library.mw`, and `rust-toolchain.toml`.
- Focused gates:
  - `git diff --check` passed with no output.
  - `git diff --name-status` returned no output.
  - `git diff --name-status -- docs/language docs/roadmap` returned no output.
  - `git diff --name-status d528f5e9a9e281c8076145a2b734976de3d8a12e -- Cargo.lock Cargo.toml rust-toolchain.toml` returned no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures cargo metadata --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml --locked --format-version 1 --no-deps` passed.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-root-fixtures cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected every owned file, reran locked metadata, and ran focused fixture consumers: `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-review-soundness cargo test -p marrow-check --test v01_fixtures --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml` passed and `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l00-review-soundness cargo test -p marrow --test v01_cli --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l00-root-fixtures/Cargo.toml` passed.
- Idiom/spec review: pass, no findings. Reviewer inspected root docs, manifest/lint shape, toolchain pin, lockfile state, and fixture naming/module shape.
- Fixed review findings: none; re-review not required.
- Absence and sibling scans:
  - Sediment scan over owned files found only AGENTS policy terms: shims, legacy, prototype, bridge, compatibility, previously, and now.
  - Unsafe scan over owned files found only the AGENTS rule forbidding unsafe Rust.
  - Fixture reference scan found `fixtures/v01/library.mw` included by `crates/marrow-check/tests/v01_fixtures.rs` and `crates/marrow/tests/v01_cli.rs`.
  - `git diff -- Cargo.lock Cargo.toml rust-toolchain.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` completed before root-fixtures tracker integration; `HEAD`, `origin/main`, and `FETCH_HEAD` were all `d528f5e9a9e281c8076145a2b734976de3d8a12e`.
  - No source cherry-pick was required because the lane changed no root/manifests/fixture files.
  - Tracker evidence was updated on main with the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` left untouched.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` after the tracker evidence update showed main aligned with `origin/main`, this tracker file modified, and the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L01 Language-Docs Evidence

- Changed files: none; language reference files were reviewed-clean without source edits.
- Lane source commit: none.
- Main source integration commit: none; this evidence update records the reviewed-clean audit state.
- Base/head: `/Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs` at `76fc2843238992766aa04be31d91596f82641964`.
- Failing-or-focused checks:
  - No RED/edit cycle was run because the build worker and reviewers found no concrete cleanup to make and no semantic proposals requiring user approval.
  - Baseline checks inspected all owned files under `docs/language/`.
- Focused gates:
  - `rg -n '\b(TODO|FIXME|legacy|prototype|migration|temporary|compatibility|shim|bridge|previously|now)\b' docs/language -g '*.md'` found only durable `migration DSL` negative scope, `std::clock::now()` examples, and `rename ... now spelled` evolution wording.
  - `rg -n '\b(future|not yet|deferred|planned|placeholder|v0\.1|reserved)\b' docs/language -g '*.md'` found only explicit v0.1/reserved/current-boundary language.
  - `rg -n 'docs/future|\.\./future|future/' docs/language -g '*.md'` returned no matches.
  - Relative Markdown link verification reported all relative language-doc links resolved.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml -p marrow-syntax --test lexer lexes_all_language_reference_mw_blocks_without_errors` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml -p marrow-syntax --test parse parses_all_documented_module_files` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml -p marrow-syntax --test parse parses_documented_reference_sample` passed with 1 test.
  - `git diff --check`, `git diff --name-status`, and no-outside-scope diff checks returned no output.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-language-docs cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l01-language-docs/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings and no semantic proposals. Reviewer verified links and anchors, fence inventory, language-reference example tests, and additional documented behavior tests: `the_reference_sample_checks_clean`, `supported_collection_wrappers_bind_their_documented_shapes`, `conversion_builtins_accept_documented_sources`, and `the_reference_sample_runs_end_to_end`, all with explicit `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l01-review-soundness`.
- Idiom/spec review: pass, no findings and no semantic proposals. Reviewer inspected all language docs and confirmed future/reserved wording is framed as explicit v0.1 absence or negative scope, not stale roadmap promises.
- Fixed review findings: none; re-review not required.
- Absence and sibling scans:
  - No `TODO`, `FIXME`, legacy/prototype/shim/bridge cleanup issue remained in `docs/language`.
  - No language doc links to `docs/future`.
  - No `Cargo.toml`, `Cargo.lock`, crate, fixture, roadmap, or non-language docs diff in the L01 worktree.
  - The durable current-boundary hits were triaged as accepted grammar and v0.1 contract text: `merge`/`lock` reserved, `~` reserved, identity-typed keys not yet supported, descending temporal ranges not yet supported, and `assert` absent from accepted grammar.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` completed before tracker integration; `HEAD`, `origin/main`, and `FETCH_HEAD` were all `76fc2843238992766aa04be31d91596f82641964`.
  - No source cherry-pick was required because the lane changed no language-doc files.
  - Tracker evidence was updated on main with the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` left untouched.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` after the tracker evidence update showed main aligned with `origin/main`, this tracker file modified, and the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L02 Docs-Meta Evidence

- Changed files: `docs/README.md`, `docs/future/README.md`, `docs/future/backend-contract.md`, `docs/future/error-codes.md`, `docs/future/implementation.md`.
- Deleted files: `docs/future/backend-contract.md`, `docs/future/error-codes.md`, `docs/future/implementation.md`.
- Lane commit: `3dd44fa8989af9e5dc1599e22caadbb02b42d851`.
- Main integration commit: `fe34e8695dae03f2d9fb1e857a22482e63edb6ab`.
- Main integration base: `3032e90a4e274fbcce91a3b3ebdd948643948e48`.
- Failing-or-focused checks:
  - RED: `git grep -n 'designed-but-unimplemented surface is recorded here yet' HEAD -- docs/future` in the L02 worktree found exactly the three placeholder-only future docs later deleted.
  - The initially proposed exact scan for `No designed-but-unimplemented surface is recorded here yet` missed because the sentence wrapped across lines; the base-tree `git grep` above was the effective failing check.
- Focused gates:
  - `rg -n 'No designed-but-unimplemented surface is recorded here yet|designed-but-unimplemented surface is recorded here yet' docs/future -g '*.md'` returned no matches after the cleanup.
  - `rg -n 'docs/future/(backend-contract|error-codes|implementation)\.md|future/(backend-contract|error-codes|implementation)\.md' docs -g '*.md' -g '!docs/roadmap/**'` returned no matches.
  - `git diff --name-only -- docs/language docs/roadmap` returned no output before tracker evidence updates.
  - `git diff --check` passed with no output.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l02-docs-meta/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l02-docs-meta/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l02-docs-meta/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-docs-meta cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l02-docs-meta/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified the deleted files were only heading/counterpart placeholders, found no in-scope links to deleted paths, confirmed no `docs/language` or `docs/roadmap` edits in the reviewed diff, and judged remaining sediment hits durable.
- Idiom/spec review: pass, no findings. Reviewer verified wording no longer implies a complete mirror or schedule, the retained future pages record real selected surfaces, and no empty placeholder sediment remains.
- Fixed review findings: none; re-review not required.
- Absence and sibling scans:
  - Placeholder text is absent from `docs/future`.
  - In-scope docs contain no links to `future/backend-contract.md`, `future/error-codes.md`, or `future/implementation.md`.
  - `git diff -- Cargo.lock Cargo.toml crates/*/Cargo.toml` returned no output.
  - L02 sediment scan retained only durable data-evolution compatibility/migration contracts, `std::clock::now`, old path aliases, bridge wording for host-system extensions, and protocol cursor text.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` completed before integration; `HEAD`, `origin/main`, and `FETCH_HEAD` were all `3032e90a4e274fbcce91a3b3ebdd948643948e48`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` before cherry-pick showed main aligned with `origin/main` and an unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git cherry-pick -x 3dd44fa8989af9e5dc1599e22caadbb02b42d851` produced `fe34e8695dae03f2d9fb1e857a22482e63edb6ab`.
  - `git diff --check HEAD^..HEAD` passed with no output.
  - Main post-integration placeholder and deleted-path link scans matched the lane absence scans.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l02-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` after the L02 cherry-pick showed main ahead by one commit with the unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

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

## L12 Store Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l12-store`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-soundness`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-idiom`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration`.
- Base/head:
  - Lane base/head: `e3690d46d5cebb760728dfb20b49cd52d0806c2b`.
  - Live main before tracker evidence: `e3690d46d5cebb760728dfb20b49cd52d0806c2b`, aligned with `origin/main`.
- Changed files: none in the L12 source worktree; only tracker evidence changed on main.
- Failing or focused check:
  - No RED check was warranted because the full store audit found no concrete behavior gap requiring a code change.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml -p marrow-store` passed with 42 unit tests, 16 `tree_store` tests, 20 `value_encoding` tests, and doc-tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml -p marrow-store --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml -p marrow-store --features native` passed with 51 unit tests including redb conformance, 7 `redb_store` tests, 17 `tree_store` tests, and 20 `value_encoding` tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml -p marrow-store --all-targets --features native -- -D warnings` passed.
- Store contracts inspected:
  - Transaction contract: `Backend`, `MemStore`, `RedbStore`, and `TreeStore` transaction wrappers; native redb rollback/commit uses an undo journal under the outer write transaction.
  - Backup/restore boundary: backup streams data-family cells through typed `DataCellKey` frames; restore tests validate framed cells before replay and revalidate the store before commit.
  - Integrity/conformance: shared conformance covers read/write/delete, pagination, snapshots, commit/rollback, and write-exclusion; native tests cover redb persistence and handle boundaries.
  - Raw/value boundary: byte decoders are private or `pub(crate)` typed codec boundaries; public encode/decode APIs take typed inputs or typed expected shapes.
  - Public raw/helper surfaces: `TreeBackupCell::from_raw` is crate-internal and validates data-cell targets; `TreeBackupCellBuf::from_raw` is test-gated; redb raw-byte tests are substrate probes.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. The reviewer exercised `marrow-store` default/native tests, backup unit tests, backup CLI tests, data CLI integrity tests, runtime transaction tests, native run persistence, and targeted evolution rollback/drift probes. One invalid combined-filter command and one zero-test filter were not counted as evidence; each intended probe was rerun with a valid focused command.
- Idiom/spec review: pass, no findings. The reviewer inspected all owned store Rust and tests, and ran `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-idiom cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l12-store/Cargo.toml -p marrow-store --all-features`, which passed with 51 unit tests, 7 `redb_store` tests, 17 `tree_store` tests, and 20 `value_encoding` tests.
- Fixed review findings:
  - None.
- Absence and sibling scans:
  - `rg -n '\bunsafe\b' crates/marrow-store -g '*.rs'` returned no matches.
  - `rg -n '\bunsafe\b|message\.contains|\.message\.contains|stderr\.contains|stdout\.contains|error\.to_string\(\)\.contains|assert!\([^\n]*contains' crates/marrow-store -g '*.rs'` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-store/Cargo.toml` returned no output.
  - `git diff --name-status` and `git diff --check` returned no output in the L12 worktree.
  - Raw/helper/prototype/legacy scans found only private decoder variables, crate-internal or test-gated backup constructors, redb native substrate tests, and durable format-version refusal wording.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin` completed; `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow rev-parse origin/main` both returned `e3690d46d5cebb760728dfb20b49cd52d0806c2b`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - Post-tracker-evidence `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main`, modified `docs/roadmap/rust-hardening-file-audit.md`, and unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - No source cherry-pick was required because the lane made no source changes.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.

## L13 Backup/Restore Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-soundness`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-idiom`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration`.
- Base/head:
  - Lane base: `2215296a4de471bf051e15990158e558b9d51bd6`.
  - Lane source commit: `fdbc324e025b5cd81b7bd97354544552c8e02bb5`.
  - Main source integration commit: `b1f0112ed36908535c0d4ef1dc09f198835134c1`.
- Changed files:
  - `crates/marrow/src/backup/archive.rs`
  - `crates/marrow/src/backup/create.rs`
  - `crates/marrow/src/backup/mod.rs`
  - `crates/marrow/src/backup/restore.rs`
  - `crates/marrow/tests/backup_cli.rs`
- Failing or focused checks:
  - Initial RED: `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml -p marrow backup::` failed because tests referenced new typed `BackupFormatProblem` and `BackupCorruptProblem` payloads before implementation.
  - Review-fix RED: the same command failed because `BackupFormatProblem::FieldType` did not exist before separating present wrong-type manifest fields from missing fields.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml -p marrow --check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml -p marrow backup::` passed with 16 backup tests after the review fix.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml -p marrow --test backup_cli` passed with 10 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - A concurrent `backup_cli`/clippy run against the same lane target dir was discarded as evidence; both commands above were rerun sequentially with the explicit target dir.
- Backup/restore contracts inspected:
  - Archive framing: magic header, format version, bounded manifest length, JSON manifest, and framed typed cell stream.
  - Manifest binding: source digest, catalog epoch, engine layout/profile/value codec, commit binding, and digest spelling validation.
  - Checksum and cell count: checksum over exact framed cell bytes, record-count-bounded replay, and trailing-byte rejection.
  - Restore validation: schema/source/catalog/engine checks before replay and integrity verification before commit.
  - Empty-target transaction rollback: non-empty targets fail before replay; replay, checksum, trailing-byte, and verify failures roll the transaction back.
  - Index rebuild: backup carries data cells only; restore rebuilds generated indexes in the restore transaction.
  - Commit metadata restamp: restored stores restamp engine profile, catalog epoch, and commit metadata after replay validation.
- Source changes:
  - Added `BackupFormatProblem` and `BackupCorruptProblem` typed payloads to `BackupError`.
  - Replaced backup unit-test `Display` parsing with typed payload assertions.
  - Added `BackupFormatProblem::FieldType { field, expected }` and a manifest wrong-type regression test.
  - Moved semantic `backup_cli` error-code checks to `--format json` and JSON `code` assertions; retained text `contains` only for render/effect checks.
- Full lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo build --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore/Cargo.toml --workspace --all-targets -- -D warnings` passed.
- Soundness review: failed first review on L13-R001, then passed re-review. Reviewer reran backup unit and CLI tests, inspected sibling readers, and manually crafted wrong-type and missing-field archive manifests to verify distinct typed/rendered behavior.
- Idiom/spec review: pass, no findings; passed re-review after `FieldType` landed. Reviewer inspected touched Rust for public type shape, duplicate classifiers, remaining `contains` checks, raw helpers, legacy hits, and warning-free production builds.
- Fixed review findings:
  - L13-R001: Present wrong-type manifest fields were classified as `MissingField`. Fixed with `BackupFormatProblem::FieldType { field, expected }`, object/number/string/array readers that distinguish missing from wrong type, and a regression test for `record_count: "1"`.
- Absence and sibling scans:
  - `rg -n 'to_string\(\)\.contains|message\.contains|\.message\.contains|error\.message\.contains' crates/marrow/src/backup crates/marrow/src/cmd_backup.rs crates/marrow/src/cmd_restore.rs crates/marrow/tests/backup_cli.rs -g '*.rs'` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow/src/backup crates/marrow/src/cmd_backup.rs crates/marrow/src/cmd_restore.rs crates/marrow/tests/backup_cli.rs -g '*.rs'` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow/Cargo.toml` returned no output.
  - `git diff --check` returned no output.
  - Raw/helper/prototype/legacy scans found only test-only raw archive mutation helpers, legacy digest rejection tests, raw-engine-copy contract docs, and render/effect comments.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin` completed before integration; `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow rev-parse origin/main` both returned `2215296a4de471bf051e15990158e558b9d51bd6`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow cherry-pick -x fdbc324e025b5cd81b7bd97354544552c8e02bb5` produced `b1f0112ed36908535c0d4ef1dc09f198835134c1`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - Post-tracker-evidence `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed main ahead of origin by the L13 source commit, modified `docs/roadmap/rust-hardening-file-audit.md`, and unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Schema Diagnostic Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-1`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-1`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-payload`.
- Base/head:
  - Lane base: `580f92ad684840c30833dc025d3a908a5aaadc2c`.
  - Lane and main source commit: `cd3d4d20a819b0b0447a2be9221edfc1817a2a95`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused schema test compile-failed before production changes because `DiagnosticPayload::Schema` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project split_store_applies_saved_field_schema_rules -- --exact` passed after the payload implementation.
- Source changes:
  - Added `DiagnosticPayload::Schema(marrow_schema::SchemaErrorKind)`.
  - Routed schema diagnostics emitted by `check_file_source`, resource schema checks, enum schema checks, and saved-member schema checks through `schema_diagnostic` or `push_schema_error`.
  - Kept compile-store duplicate suppression on the prior code/file/message/span key while preserving the schema payload on the diagnostic that survives.
  - Updated resolution suppression matches so schema payloads are never treated as hidden resolution facts.
  - Migrated the touched early schema-family checker tests to assert exact `SchemaErrorKind` payloads instead of checking rendered message substrings.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project split_store_applies_saved_field_schema_rules -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
  - An accidental concurrent `marrow-check` test/clippy run against the same lane target dir was discarded as evidence; both commands above were rerun sequentially with the explicit target dir.
- Soundness review: pass, no findings. Reviewer reran `git diff --check`, `cargo test -p marrow-check --test project`, and `cargo test -p marrow-check` with the soundness review target dir.
- Idiom/spec review: pass, no findings. Reviewer reran `git diff --check`, `cargo test -p marrow-check`, `cargo fmt --all --check`, and `cargo clippy -p marrow-check --all-targets -- -D warnings` with the idiom/spec review target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n 'marrow_schema::compile_|SchemaError|push_schema_error|schema_diagnostic' crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs` showed the touched schema emit paths route through `schema_diagnostic` or `push_schema_error`.
  - `rg -n 'message\.contains|rendered\.contains|\.contains\(' crates/marrow-check/tests/project.rs` still finds unrelated checker tests outside this slice; the migrated schema-family assertions no longer use rendered message substrings.
  - `git diff --name-only` before commit listed only the three changed L06 files; no manifest or lockfile changed.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` completed before integration; `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow rev-parse origin/main` both returned `580f92ad684840c30833dc025d3a908a5aaadc2c`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only cd3d4d20a819b0b0447a2be9221edfc1817a2a95` fast-forwarded main.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-payload cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs'` returned no matches.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-payload cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-payload cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-payload cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - Post-tracker-evidence `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed main at `cd3d4d20a819b0b0447a2be9221edfc1817a2a95`, modified `docs/roadmap/rust-hardening-file-audit.md`, and unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Duplicate Root Owner Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-2`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-2`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-root`.
- Base/head:
  - Lane base: `7667224db5cacd29be7ce59246a88dec0c567058`.
  - Lane and main source commit: `fda86c7510142f78aa575fdf90a408f7b58f2715`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/analysis.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused duplicate-root test compile-failed before production changes because `DiagnosticPayload::DuplicateRootOwner` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_two_stores_sharing_one_saved_root -- --exact` passed after the payload implementation.
- Source changes:
  - Added `DiagnosticPayload::DuplicateRootOwner { root: String, first_owner: PathBuf }`.
  - Emitted the payload at the `schema.duplicate_root_owner` diagnostic site while preserving diagnostic code, severity, file, rendered message, and span.
  - Updated the resolution suppression matches so duplicate-root payloads do not become hidden resolution facts.
  - Migrated `reports_two_stores_sharing_one_saved_root` to assert the typed payload instead of checking the rendered message for `books`.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_two_stores_sharing_one_saved_root -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer reran `git diff --check`, the exact duplicate-root test, and a configured-test resolution suppression regression with the soundness review target dir.
- Idiom/spec review: pass, no findings. Reviewer reran the exact duplicate-root test, clippy, and a valid package formatter check with the idiom/spec review target dir; an invalid formatter invocation without `--all` or `-p` failed and was not used as evidence.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/analysis.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n 'duplicate_root_owner|DuplicateRootOwner|message\.contains\("books"\)|schema\.duplicate_root_owner' crates/marrow-check/src/lib.rs crates/marrow-check/src/analysis.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` found the new payload sites and one unrelated `check.duplicate_declaration` prose assertion outside this slice.
  - `rg -n 'SCHEMA_DUPLICATE_ROOT_OWNER|duplicate_root_owner' crates/marrow-check/src crates/marrow-check/tests -g '*.rs'` found one duplicate-root emit site.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` completed before integration; `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow rev-parse origin/main` both returned `7667224db5cacd29be7ce59246a88dec0c567058`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin rust-hardening-l06-checker-core` fetched the lane branch, and `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-root cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs'` returned no matches.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-root cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-root cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-root cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - Post-tracker-evidence `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed main at `fda86c7510142f78aa575fdf90a408f7b58f2715`, modified `docs/roadmap/rust-hardening-file-audit.md`, and unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Duplicate Declaration Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-3`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-3`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-declaration`.
- Base/head:
  - Lane base: `d0d05109fdfd922cc16f639515b2a51a1ba09384`.
  - Lane and main source commit: `2a5d61d222b9ce934f5ddea106b1736eab03bf7e`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused duplicate-function declaration test compile-failed before production changes because `DiagnosticPayload::DuplicateDeclaration` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_duplicate_function_declaration -- --exact` passed after the payload implementation.
- Source changes:
  - Added `DiagnosticPayload::DuplicateDeclaration { name: String, first_span: SourceSpan }`.
  - Emitted the payload only at the true `check.duplicate_declaration` duplicate-name path while preserving diagnostic code, severity, file, rendered message, and later-occurrence span.
  - Left builtin-name collision diagnostics as `DiagnosticPayload::None`.
  - Updated project and configured-test resolution suppression matches so duplicate-declaration payloads do not become hidden resolution facts.
  - Migrated duplicate declaration tests for function, const, resource, const/resource, import/function, and enum/resource collisions to assert typed payloads instead of checking rendered message names.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_duplicate_function_declaration -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected the dirty diff, scanned duplicate-declaration payload and suppression sites, reran duplicate/collision focused project tests, reran configured-test suppression probes, and ran `git diff --check` with the soundness review target dir.
- Idiom/spec review: pass, no findings. Reviewer confirmed the changed-file set, payload emit site, suppression matches, migrated duplicate-declaration tests, and no manifest/lock churn; reran duplicate and collision filters with the idiom/spec review target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n "message\.contains\(\"run\"\)|message\.contains\('A'\)|message\.contains\(\"Book\"\)|message\.contains\(\"books\"\)|check\.duplicate_declaration|DuplicateDeclaration" crates/marrow-check/tests/project.rs crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs` found only the duplicate-declaration code constant, payload sites, and shared test helper.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane base and `origin/main` both returned `d0d05109fdfd922cc16f639515b2a51a1ba09384`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `d0d0510` to `2a5d61d`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `d0d05109fdfd922cc16f639515b2a51a1ba09384`, and `origin/rust-hardening-l06-checker-core` returned `2a5d61d222b9ce934f5ddea106b1736eab03bf7e`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `2a5d61d222b9ce934f5ddea106b1736eab03bf7e`.
  - A combined integration command stopped when an unsafe no-match scan returned status 1, so it was not used as gate evidence.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-declaration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-declaration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-declaration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-declaration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Duplicate Module Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-4`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-4`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-module`.
- Base/head:
  - Lane base: `d7979b6f43e51be6cae1ba584ddcfd0d63241e39`.
  - Lane and main source commit: `27522e95874141d59f042fbfe599c06e656b81e9`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/analysis.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused duplicate-source-module test compile-failed before production changes because `DiagnosticPayload::DuplicateModule` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_duplicate_module_across_source_roots -- --exact` passed after the payload implementation.
- Source changes:
  - Added `DiagnosticPayload::DuplicateModule { name: String, first_file: PathBuf }`.
  - Emitted the payload at both `check.duplicate_module` sites: source-root duplicate modules in `analysis.rs` and configured test/source module collisions in `lib.rs`.
  - Preserved diagnostic code, severity, file, rendered message, and span at both emit sites.
  - Updated project and configured-test resolution suppression matches so duplicate-module payloads do not become hidden resolution facts.
  - Migrated the duplicate source-root and configured-test collision assertions to exact payload checks instead of rendered message-name checks.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_duplicate_module_across_source_roots -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project analyze_project_reports_duplicate_when_test_module_collides_with_source_module -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected the dirty diff, proved there are only two `CHECK_DUPLICATE_MODULE` emit sites and both carry the payload, scanned for remaining duplicate-module rendered-message name assertions, reran duplicate-module and configured-test collision focused tests, and reran suppression probes with the soundness review target dir.
- Idiom/spec review: pass, no findings. Reviewer confirmed changed-file scope, both emit sites, both suppression matches, no manifest/lock churn, and reran duplicate-filter tests plus formatter with the idiom/spec review target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/analysis.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n 'message\.contains\("shared"\)|check\.duplicate_module.*message|duplicate_module.*message|module .*already declared|DuplicateModule|CHECK_DUPLICATE_MODULE' crates/marrow-check/src crates/marrow-check/tests/project.rs` found the two render emitters plus the new payload sites and assertions, with no duplicate-module tests parsing names from rendered messages.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane base and `origin/main` both returned `d7979b6f43e51be6cae1ba584ddcfd0d63241e39`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `d7979b6` to `27522e9`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `d7979b6f43e51be6cae1ba584ddcfd0d63241e39`, and `origin/rust-hardening-l06-checker-core` returned `27522e95874141d59f042fbfe599c06e656b81e9`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `27522e95874141d59f042fbfe599c06e656b81e9`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-module cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-module cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-module cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-module cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Module Path Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-5`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-5`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-module-path`.
- Base/head:
  - Lane base: `8473594d44045736b85b6c099fdbb2579128d4df`.
  - Lane and main source commit: `e172640e96945428f626d183850bdb0d54064f90`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/analysis.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused module-path mismatch test compile-failed before production changes because `DiagnosticPayload::ModulePath` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_module_path_mismatch -- --exact` passed after the payload implementation.
- Source changes:
  - Added `DiagnosticPayload::ModulePath { declared: String, expected: Option<String> }`.
  - Emitted the payload from the single `module_path_error` helper while preserving diagnostic code, severity, file, rendered message, and span.
  - Threaded the expected module path from both ordinary mismatch and defensive missing-expected paths in `analysis.rs`.
  - Updated project and configured-test resolution suppression matches so module-path payloads do not become hidden resolution facts.
  - Migrated the module-path mismatch and dotted-stem assertions to exact payload checks instead of rendered message-name checks.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project reports_module_path_mismatch -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project a_dotted_stem_file_cannot_be_a_module -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected the dirty diff, verified the single helper and both emit paths carry the payload, checked suppression behavior, scanned for remaining module-path rendered-message assertions, reran the two focused module-path tests, reran suppression probes, and reran the full `marrow-check` package with the soundness review target dir.
- Idiom/spec review: pass, no findings. Reviewer confirmed changed-file scope, small explicit payload shape, both suppression matches, no manifest/lock churn, reran the two focused module-path tests, ran formatter, and scanned changed files for old message-name assertions with the idiom/spec review target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/analysis.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n 'message\.contains\("shelf::books"\)|message\.contains\("config\.v2"\)|check\.module_path.*message|ModulePath|CHECK_MODULE_PATH|module_path_error' crates/marrow-check/src crates/marrow-check/tests/project.rs` found the code constant, helper, new payload sites, and payload assertions, with no module-path tests parsing names from rendered messages.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane base and `origin/main` both returned `8473594d44045736b85b6c099fdbb2579128d4df`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `8473594` to `e172640`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `8473594d44045736b85b6c099fdbb2579128d4df`, and `origin/rust-hardening-l06-checker-core` returned `e172640e96945428f626d183850bdb0d54064f90`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `e172640e96945428f626d183850bdb0d54064f90`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-module-path cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-module-path cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-module-path cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-module-path cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Rejected Surface Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-6`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-6`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-rejected-surface`.
- Base/head:
  - Lane base: `9e03d4290468858a99ef1e9bccc22420e7b94328`.
  - Lane and main source commit: `b7402e173580c6ff4e2532ed4a9fd1eeb6c34064`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/rejected_surface.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The focused saved-`inout` test compile-failed before production changes because `RejectedSurface` was not exported and `DiagnosticPayload::RejectedSurface` did not exist.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project saved_inout_through_resource_reference_is_rejected -- --exact` passed after the payload implementation.
- Source changes:
  - Added `RejectedSurface::{SavedInout, SavedTraversalMethod { method }}` and `DiagnosticPayload::RejectedSurface`.
  - Emitted `RejectedSurface::SavedInout` for saved `inout` rejection diagnostics while preserving diagnostic code, severity, file, rendered message, and span.
  - Emitted `RejectedSurface::SavedTraversalMethod { method }` for old saved traversal shaper diagnostics.
  - Updated project and configured-test resolution suppression matches so rejected-surface payloads do not become hidden resolution facts.
  - Migrated four saved-`inout` tests and the seven-method traversal-shaper test to exact payload checks instead of rendered message-name checks.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project saved_inout_through_resource_reference_is_rejected -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project saved_inout_through_index_entry_is_rejected_surface -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project malformed_saved_inout_through_keyed_root_field_is_rejected -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project malformed_saved_inout_through_index_branch_is_rejected -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project old_saved_traversal_method_shapers_are_rejected -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project declared_saved_members_named_like_traversal_shapers_are_not_rejected -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected the dirty diff, verified both rejected-surface emit sites pass typed payloads through the single `push` helper, checked saved-`inout` payloads, checked all seven traversal method payloads in source order, ran the declared-member negative test, ran suppression probes, and found no manifest or lockfile churn.
- Idiom/spec review: pass, no findings. Reviewer confirmed changed-file scope, small diagnostic-payload-scoped enum shape, direct emit-site payload construction, explicit suppression matches, no source message parsing in the migrated family, no dependency changes, and no unsafe.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/rejected_surface.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - ``rg -n 'check\.rejected_surface|RejectedSurface|payload: DiagnosticPayload::None|saved `inout`|take|window|after|from|until|resume|reverse|message\.contains' crates/marrow-check/src/rejected_surface.rs crates/marrow-check/tests/project.rs crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs`` found the new payload sites and then-remaining unrelated checker prose assertions, with no rendered-message assertions in the migrated rejected-surface family and no `DiagnosticPayload::None` in the rejected-surface emitter.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane base and `origin/main` both returned `9e03d4290468858a99ef1e9bccc22420e7b94328`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `9e03d42` to `b7402e1`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `9e03d4290468858a99ef1e9bccc22420e7b94328`, and `origin/rust-hardening-l06-checker-core` returned `b7402e173580c6ff4e2532ed4a9fd1eeb6c34064`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `b7402e173580c6ff4e2532ed4a9fd1eeb6c34064`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-rejected-surface cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-rejected-surface cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-rejected-surface cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-rejected-surface cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Schema Unsupported Map Assertion Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-7`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-7`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-unsupported-map`.
- Base/head:
  - Lane base: `70b4091a5ab6f353a84c06f7a955c0ba49ac225d`.
  - Lane and main source commit: `baedd30c8d6089895c4766d3b5c8eef38839dd48`.
- Changed files:
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - No production RED was required because `DiagnosticPayload::Schema(SchemaErrorKind::UnsupportedType { target, name })` already existed from the integrated schema payload slice.
  - The focused assertion migration replaced a rendered-message substring check with the exact existing schema payload for the unsupported `scores` field.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project map_annotations_outside_resource_members_are_not_supported_types -- --exact` passed after the assertion migration.
- Source/test changes:
  - Added the `SchemaUnsupportedTypeTarget` import in `crates/marrow-check/tests/project.rs`.
  - Replaced `message.contains("scores")` in `map_annotations_outside_resource_members_are_not_supported_types` with exact `DiagnosticPayload::Schema(SchemaErrorKind::UnsupportedType { target: SchemaUnsupportedTypeTarget::Field, name: "scores".into() })`.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project map_annotations_outside_resource_members_are_not_supported_types -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified the exact `SchemaErrorKind::UnsupportedType` payload assertion, confirmed the old `message.contains("scores")` assertion was gone, checked that no production code or manifests changed, and found no semantic regression in the focused test.
- Idiom/spec review: pass, no findings. Reviewer confirmed the one-file test scope, direct typed assertion shape, no new helper or compatibility surface, formatter cleanliness, and no dependency churn.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `rg -n 'map_annotations_outside_resource_members_are_not_supported_types|schema\.unsupported_type|UnsupportedType|message\.contains\("scores"\)|SchemaUnsupportedTypeTarget' crates/marrow-check/tests/project.rs crates/marrow-schema/src/lib.rs` found the exact payload/import and schema payload type, with no old `message.contains("scores")` assertion.
  - `rg -n '\bunsafe\b' crates/marrow-check/tests/project.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane base and `origin/main` both returned `70b4091a5ab6f353a84c06f7a955c0ba49ac225d`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `70b4091` to `baedd30`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `70b4091a5ab6f353a84c06f7a955c0ba49ac225d`, and `origin/rust-hardening-l06-checker-core` returned `baedd30c8d6089895c4766d3b5c8eef38839dd48`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `baedd30c8d6089895c4766d3b5c8eef38839dd48`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-unsupported-map cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-unsupported-map cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-unsupported-map cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-schema-unsupported-map cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Enum Diagnostic Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-8`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-8`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-enum-payload`.
- Base/head:
  - Lane base: `9211cece964e28eeddd7873d58300d42b977cd91`.
  - Lane and main source commit: `bca8b859817282a643ad21180907795c14085eab`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/infer.rs`
  - `crates/marrow-check/src/enums.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The first focused enum-payload migration failed before production changes because `marrow_check::EnumDiagnostic` and `DiagnosticPayload::Enum` did not exist.
  - The first green compile then found the sibling exhaustive match in `crates/marrow-check/src/checks.rs`; the fix keeps enum payload diagnostics during incomplete-module suppression because they are not hidden-module resolution noise.
- Source/test changes:
  - Added `EnumDiagnostic` and `DiagnosticPayload::Enum`.
  - Emitted enum payloads directly at enum-member and enum-match diagnostic emit sites for unknown members, ambiguous members, ambiguous match arms, nonexhaustive matches, duplicate match arms, and category-not-selectable value positions.
  - Added member labels to the resolved enum-member path so value-position and `is` diagnostics carry the same qualifying-path payload shape.
  - Updated incomplete-module suppression matches in both checker paths to keep enum payload diagnostics.
  - Migrated enum/member-path assertions from rendered `message.contains` checks to exact `EnumDiagnostic` payload assertions for the integrated enum family.
- Focused and lane gates:
  - Thirteen focused exact `project.rs` tests passed, each with 1 test: `a_match_missing_a_leaf_is_nonexhaustive`, `a_bare_duplicated_member_in_value_position_is_ambiguous`, `a_bare_duplicated_match_arm_is_actionably_ambiguous`, `a_match_missing_a_duplicated_leaf_reports_its_full_path`, `is_with_a_bare_duplicated_member_is_ambiguous`, `reports_an_unknown_enum_member`, `a_nonexhaustive_match_is_a_check_error`, `a_match_arm_for_an_unknown_member_is_a_check_error`, `a_match_over_an_enum_saved_field_enforces_exhaustiveness`, `a_nonexhaustive_match_over_a_qualified_enum_scrutinee_is_a_check_error`, `a_match_over_a_sequence_enum_element_enforces_its_identity`, `a_nonexhaustive_match_over_a_nested_module_enum_scrutinee_is_a_check_error`, and `an_unknown_member_of_a_nested_module_enum_literal_is_a_check_error`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer inspected the dirty diff, verified changed-file scope, verified enum payload facts at emit sites, scanned for old enum-family rendered-message assertions, found no manifest/lock churn or unsafe, and reran `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-8 cargo test -p marrow-check --test project --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml`, which passed with 368 tests.
- Idiom/spec review: pass, no findings. Reviewer confirmed the small payload-family shape, direct emit-site payload construction, durable comments, no unrelated L07 or non-diagnostic assertion migration, `git diff --check` cleanliness, and reran `reports_an_unknown_enum_member` with the idiom review target dir, which passed with 1 test.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the five changed L06 files.
  - `rg -n 'message\.contains\("deleted"\)|message\.contains\("banned"\)|message\.contains\("archived"\)|message\.contains\("bogus"\)|message\.contains\("tiger::siberian"\)|message\.contains\("tiger::paw"\)|message\.contains\("lion::paw"\)|message\.contains\("lion::mane"\)' crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/checks.rs crates/marrow-check/src/enums.rs crates/marrow-check/src/infer.rs crates/marrow-check/src/lib.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before integration; lane merge-base and `origin/main` both returned `9211cece964e28eeddd7873d58300d42b977cd91`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `9211cec` to `bca8b85`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `9211cece964e28eeddd7873d58300d42b977cd91`, and `origin/rust-hardening-l06-checker-core` returned `bca8b859817282a643ad21180907795c14085eab`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `bca8b859817282a643ad21180907795c14085eab`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-enum-payload cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-enum-payload cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-enum-payload cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-enum-payload cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Parent-Not-Category Assertion Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-9`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-9`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-parent-category`.
- Base/head:
  - Lane base: `e4eac9e49e9d28021830d6f1934ecae9fbad948d`.
  - Lane and main source commit: `ec6d7dd04361210eee21c137b1d1edbc2a0172de`.
- Changed file:
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The RED check temporarily asserted `SchemaErrorKind::ParentNotCategory { member: "lion" }` for `a_non_category_parent_with_children_is_rejected`; `cargo test ... --test project a_non_category_parent_with_children_is_rejected` failed because the diagnostic payload was `ParentNotCategory { member: "tiger" }`.
  - The GREEN check changed the expected payload to `member: "tiger"` and the same focused test passed with 1 test.
- Source/test changes:
  - Replaced the rendered `message.contains("tiger")` assertion with the existing `assert_schema_payload` helper and exact `SchemaErrorKind::ParentNotCategory { member: "tiger" }`.
  - No production source changed; the schema payload already existed at the emitter.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project a_non_category_parent_with_children_is_rejected` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified the one-file diff, absence of `message.contains("tiger")`, direct comparison through `assert_schema_payload`, no unrelated lane edits, and reran `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-9 cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project a_non_category_parent_with_children_is_rejected -- --exact`, which passed with 1 test.
- Idiom/spec review: pass, no findings. Reviewer verified the slice stayed limited to `crates/marrow-check/tests/project.rs`, used the existing helper/import shape, had no docs or manifest churn, and left unrelated message assertions outside the slice.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only `crates/marrow-check/tests/project.rs`.
  - `rg -n 'message\.contains\("tiger"\)' crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-check/tests/project.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml crates/marrow-schema/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before branch push; lane head was `ec6d7dd04361210eee21c137b1d1edbc2a0172de`, `origin/main` was `e4eac9e49e9d28021830d6f1934ecae9fbad948d`, and the merge-base was the same `e4eac9e49e9d28021830d6f1934ecae9fbad948d`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `e4eac9e` to `ec6d7dd`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `e4eac9e49e9d28021830d6f1934ecae9fbad948d`, and `origin/rust-hardening-l06-checker-core` returned `ec6d7dd04361210eee21c137b1d1edbc2a0172de`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `ec6d7dd04361210eee21c137b1d1edbc2a0172de`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-parent-category cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-parent-category cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-parent-category cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-parent-category cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Script Import Payload Assertion Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-10`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-10`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-script-import`.
- Base/head:
  - Lane base: `38547db2620e3f613b9766be6a320298733d007a`.
  - Lane and main source commit: `8f38f5a4bf76268ed68c05df35e6b11b8a6aae31`.
- Changed file:
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused check:
  - The RED check temporarily asserted `DiagnosticPayload::UnresolvedImport("other")` for `another_module_cannot_use_a_module_less_script`; `cargo test ... --test project another_module_cannot_use_a_module_less_script -- --exact` failed because the diagnostic payload was `UnresolvedImport("app")`.
  - The GREEN check changed the expected payload to `UnresolvedImport("app")` and the same focused test passed with 1 test.
- Source/test changes:
  - Replaced the rendered `message.contains("app")` assertion with an exact `DiagnosticPayload::UnresolvedImport("app")` assertion.
  - No production source changed; unresolved-import diagnostics already carry the imported module spelling as a payload.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project another_module_cannot_use_a_module_less_script -- --exact` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 368 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified the one-file diff, absence of `message.contains("app")`, direct comparison to `DiagnosticPayload::UnresolvedImport("app")`, no manifest or lock churn, and reran `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-10 cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project another_module_cannot_use_a_module_less_script -- --exact`, which passed with 1 test.
- Idiom/spec review: pass, no findings. Reviewer verified the slice stayed limited to `crates/marrow-check/tests/project.rs`, used the existing `DiagnosticPayload` import, added no helper or shim, and left unrelated message assertions outside the slice.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only `crates/marrow-check/tests/project.rs`.
  - `rg -n 'message\.contains\("app"\)' crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-check/tests/project.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main` completed before branch push; lane head was `8f38f5a4bf76268ed68c05df35e6b11b8a6aae31`, `origin/main` was `38547db2620e3f613b9766be6a320298733d007a`, and the merge-base was the same `38547db2620e3f613b9766be6a320298733d007a`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `38547db` to `8f38f5a`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `38547db2620e3f613b9766be6a320298733d007a`, and `origin/rust-hardening-l06-checker-core` returned `8f38f5a4bf76268ed68c05df35e6b11b8a6aae31`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `8f38f5a4bf76268ed68c05df35e6b11b8a6aae31`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-script-import cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-script-import cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-script-import cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-script-import cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.

## L06 Private Enum Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-11`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-11`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-private-enum`.
- Base/head:
  - Original lane base for RED: `719ba44302b17d04e09163ef58324c98616a9b90`.
  - First reviewed source commit before live-main rebase: `d5ac0099b53e2b83183e92ada2ff9233e4e4db54`.
  - Live integration base after concurrent store-lane commit: `59a6ac6710cc00fe61a5e18b2ca3282b29e34477`.
  - Rebased lane and main source commit: `07c346d2e773a562f5034304a6caed4f08b02058`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/src/infer.rs`
  - `crates/marrow-check/src/enums.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: `cross_module_use_of_a_private_enum_is_a_visibility_error` compile-failed because `DiagnosticPayload::PrivateEnum` did not exist.
  - GREEN: after adding the payload variant and emitters, the same exact test passed.
  - Sibling RED: after adding `is_operand_private_enum_diagnostic_carries_payload`, the `check_is` emitter was temporarily changed back to `DiagnosticPayload::None`; the exact test failed with actual `None` and expected `PrivateEnum("a::Hidden")`.
  - Sibling GREEN: restoring `DiagnosticPayload::PrivateEnum(private)` in the `check_is` emitter made the exact test pass.
- Source/test changes:
  - Added `DiagnosticPayload::PrivateEnum(String)`.
  - Emitted the payload from all three `check.private_enum` paths: type annotation, value-position enum member, and `is` right operand.
  - Updated the incomplete-module and configured-test resolution suppression matches so private-enum payloads remain non-suppressed visibility diagnostics.
  - Migrated the cross-module private-enum test from `message.contains("a::Hidden")` to exact payload assertions.
  - Added `is_operand_private_enum_diagnostic_carries_payload` to cover the `check_is` private-enum emitter with a production project fixture.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project cross_module_use_of_a_private_enum_is_a_visibility_error -- --exact` passed with 1 test after the first GREEN.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project is_operand_private_enum_diagnostic_carries_payload -- --exact` passed with 1 test after the sibling GREEN.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project private_enum` passed with 2 tests after rebasing over current `origin/main`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 369 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Initial review verified all private-enum emitters carried typed payloads and suppression behavior was unchanged. Final re-review verified the added `is` fixture reaches `check_is`, reran the `is_operand_private_enum_diagnostic_carries_payload`, `private_enum`, and `incomplete` focused filters with the soundness target dir, and found no remaining `CHECK_PRIVATE_ENUM` emitter with `DiagnosticPayload::None`.
- Idiom/spec review: pass, no findings. Initial and final reviews verified the diff stayed in the five expected L06 files, the payload fits the existing `DiagnosticPayload` design, the new `is` fixture is narrow, and there was no manifest/lock churn or comment sediment.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the five changed checker-core files above.
  - A targeted multi-line scan over `crates/marrow-check/src/checks.rs`, `crates/marrow-check/src/infer.rs`, and `crates/marrow-check/src/enums.rs` found no `CHECK_PRIVATE_ENUM` emitter followed by `payload: DiagnosticPayload::None`.
  - `rg -n 'message\.contains\("a::Hidden"\)|message\.contains\("Hidden"\)|\bunsafe\b' crates/marrow-check/src/checks.rs crates/marrow-check/src/infer.rs crates/marrow-check/src/enums.rs crates/marrow-check/src/lib.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main` before integration found `origin/main` at `59a6ac6710cc00fe61a5e18b2ca3282b29e34477`, a concurrent store-lane commit touching only `crates/marrow-store`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core rebase origin/main` rebased the reviewed L06 source commit cleanly to `07c346d2e773a562f5034304a6caed4f08b02058`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push --force-with-lease=rust-hardening-l06-checker-core:d5ac0099b53e2b83183e92ada2ff9233e4e4db54 origin rust-hardening-l06-checker-core` updated the lane branch to the rebased source commit.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/main` fast-forwarded main to the concurrent store commit, then `git -C /Users/scottwilliams/Dev/marrow merge --ff-only rust-hardening-l06-checker-core` fast-forwarded main to `07c346d2e773a562f5034304a6caed4f08b02058`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-private-enum cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `rg -n '\bunsafe\b' /Users/scottwilliams/Dev/marrow/crates -g '*.rs'` returned no matches through a wrapper that treats no matches as success.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-private-enum cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-private-enum cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-private-enum cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.

## L06 Duplicate Named Argument Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-12`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-12`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-named-argument`.
- Base/head:
  - Lane base: `988d72513a9563a3fa37e26288b67d23dd34676f`.
  - Lane and main source commit: `8d46e50964e3c423dfd7bc8fccfa57af72d2e879`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: `rejects_duplicate_named_arguments` and `a_resource_constructor_rejects_duplicate_fields` compile-failed because `DiagnosticPayload::DuplicateNamedArgument` did not exist.
  - GREEN: after adding the payload variant and emitters, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project duplicate` passed with 19 tests.
- Source/test changes:
  - Added `DiagnosticPayload::DuplicateNamedArgument(String)` for `check.call_argument` duplicate supplied-name diagnostics.
  - Emitted the payload from both duplicate supplied-name paths: duplicate user-function named parameter and duplicate resource-constructor field.
  - Left ordinary `check.call_argument` arity, unknown-name, required-field, conversion, and then-remaining type-mismatch diagnostics on `DiagnosticPayload::None`.
  - Updated incomplete-module and configured-test resolution suppression matches so duplicate named argument payloads remain non-suppressed semantic call diagnostics.
  - Migrated `rejects_duplicate_named_arguments` from `message.contains("a")` to exact payload assertion and added the resource-constructor sibling fixture for duplicate field `title`.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project duplicate` passed with 19 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 370 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified the user-function and resource-constructor duplicate emitters, confirmed non-duplicate `check.call_argument` paths still use `DiagnosticPayload::None`, checked incomplete-module and configured-test suppression behavior, reran duplicate, incomplete, suppress, call, and constructor focused filters with the soundness target dir, and found no manifest or lockfile churn.
- Idiom/spec review: pass, no findings. Reviewer confirmed the three-file scope, small payload variant shape, narrow emit-site changes, explicit suppression matches, no manifest/lock churn, and formatter cleanliness with the idiom target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the three changed checker-core files above.
  - `rg -n -C 4 'DuplicateNamedArgument|supplied more than once|message\.contains\("a"\)|CHECK_CALL_ARGUMENT|payload: DiagnosticPayload::None|\bunsafe\b' crates/marrow-check/src/checks.rs crates/marrow-check/src/lib.rs crates/marrow-check/tests/project.rs` found the two typed duplicate supplied-name emitters, the exact payload assertions, unrelated non-duplicate `DiagnosticPayload::None` call diagnostics, no old `message.contains("a")` assertion, and no unsafe usage.
  - A targeted scan for `supplied more than once` followed by `DiagnosticPayload::None` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main rust-hardening-l06-checker-core` completed before branch push; lane head was `8d46e50964e3c423dfd7bc8fccfa57af72d2e879`, `origin/main` was `988d72513a9563a3fa37e26288b67d23dd34676f`, and the merge-base was the same `988d72513a9563a3fa37e26288b67d23dd34676f`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `988d725` to `8d46e50`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration, and `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `8d46e50964e3c423dfd7bc8fccfa57af72d2e879`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-named-argument cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - A broad docs-including unsafe scan returned checklist and tracker text matches, so it was discarded as invalid gate evidence.
  - `if rg -n '\bunsafe\b' -g '*.rs' crates; then exit 1; fi` passed with no output from `/Users/scottwilliams/Dev/marrow`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-named-argument cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-named-argument cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-duplicate-named-argument cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow fetch origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus the same unrelated untracked files.

## L06 Append Target Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-13`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-13`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-append-target`.
- Base/head:
  - Lane base: `f8f6078ea34968d8c679d6433188670566d7c594`.
  - Lane and main source commit: `adedd4a7cecb5f9e80ee81b07a53427e2b88a633`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: after updating `append_to_a_group_layer_is_a_check_error` and `appending_to_a_string_keyed_layer_is_rejected` to expect typed append payloads, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project append` failed because `AppendTargetDiagnostic` and `DiagnosticPayload::AppendTarget` did not exist.
  - GREEN: after adding the payload type and emitters, the same append filter passed with 5 tests.
- Source/test changes:
  - Added `AppendTargetDiagnostic` and `DiagnosticPayload::AppendTarget`.
  - Emitted `AppendTargetDiagnostic::GroupLayer` when `append` targets a keyed group layer.
  - Emitted `AppendTargetDiagnostic::NonIntKeyedLayer { key_type }` when `append` targets a keyed leaf layer whose key is not `int`.
  - Updated incomplete-module and configured-test resolution suppression matches so append-target payloads remain non-suppressed semantic call diagnostics.
  - Migrated `append_to_a_group_layer_is_a_check_error` from `message.contains("leaf layer")` to an exact payload assertion.
  - Added a typed payload assertion to `appending_to_a_string_keyed_layer_is_rejected` for the sibling non-`int` keyed layer path.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project append` passed with 5 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 370 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Soundness review: pass, no findings. Reviewer verified both append-target emitters, confirmed the non-`int` keyed payload carries `MarrowType`, checked suppression behavior, scanned for old `message.contains("leaf layer")`, manifest/lock churn, unsafe, and append target `DiagnosticPayload::None`, reran the append filter, and reran sibling ordinary-call and suppression exact tests with the soundness target dir.
- Idiom/spec review: pass, no findings. Reviewer confirmed the three-file scope, durable payload enum shape, direct typed assertions, no generic compatibility/helper surface, no ordinary call-argument reclassification, and formatter cleanliness with the idiom target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the three changed checker-core files above.
  - `rg -n 'AppendTarget|append|message\.contains\("leaf layer"\)|\bunsafe\b|payload: DiagnosticPayload::None' crates/marrow-check/src/checks.rs crates/marrow-check/src/lib.rs crates/marrow-check/tests/project.rs` found the new payload sites and unrelated non-append `DiagnosticPayload::None` diagnostics, with no old leaf-layer prose assertion and no unsafe usage.
  - Review scans found no append-target emitter still using `DiagnosticPayload::None`.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core fetch origin main rust-hardening-l06-checker-core` completed before branch push; lane head was `adedd4a7cecb5f9e80ee81b07a53427e2b88a633`, `origin/main` was `f8f6078ea34968d8c679d6433188670566d7c594`, and the merge-base was the same `f8f6078ea34968d8c679d6433188670566d7c594`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane from `f8f6078` to `adedd4a`.
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin main rust-hardening-l06-checker-core` completed before integration; main HEAD and `origin/main` both returned `f8f6078ea34968d8c679d6433188670566d7c594`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow merge --ff-only origin/rust-hardening-l06-checker-core` fast-forwarded main to `adedd4a7cecb5f9e80ee81b07a53427e2b88a633`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-append-target cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `if rg -n '\bunsafe\b' -g '*.rs' crates; then exit 1; fi` passed with no output from `/Users/scottwilliams/Dev/marrow`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-append-target cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-append-target cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-append-target cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After `git -C /Users/scottwilliams/Dev/marrow fetch origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - After `git -C /Users/scottwilliams/Dev/marrow push origin main`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus the same unrelated untracked files.

## L06 Conversion Source Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-14`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-14`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source`; superseded by `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source-2` after `origin/main` advanced during the first clean gate.
- Base/head:
  - Lane base: `9384185cb71a0dde6514c85dfddc814e6c9062d3`.
  - Worker source commit before rebase: `3e32a7f41a57bec9879b34975155a0a62e710cbd`.
  - Reviewed source commit rebased onto the then-live main: `3ace8fc58fe27a32aa0431fe0e0f027d0e96608f`.
  - Final integration base after concurrent main update: `7cb179e616b8d83b97358d129434d54449cad152`.
  - Final lane and main source commit: `15dc6e474dc0904ffd8d0885546b164c0612552e`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: after updating the conversion assertions to expect typed payloads, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project conversion` failed because `ConversionTarget`, `ConversionUnsupportedSourceDiagnostic`, and `DiagnosticPayload::ConversionUnsupportedSource` did not exist.
  - GREEN: after adding the conversion payload type and emitter, the same conversion filter passed with 9 tests.
- Source/test changes:
  - Moved the checker conversion target identity from `checks.rs` into `lib.rs` as `ConversionTarget`, so `string` and `ErrorCode` remain distinct conversion identities even though both store as `ScalarType::Str`.
  - Added `ConversionUnsupportedSourceDiagnostic` and `DiagnosticPayload::ConversionUnsupportedSource`, carrying the conversion target, rejected source type, and accepted static source types.
  - Updated `check_conversion_arg` to emit the typed payload for unsupported known conversion sources while preserving the rendered `check.call_argument` diagnostic.
  - Updated incomplete-module and configured-test resolution suppression matches so conversion-source payloads remain non-suppressed semantic call diagnostics.
  - Updated `is_builtin_name` and `conversion_return_type` to use `ConversionTarget`, preserving the `ErrorCode` conversion return type while avoiding a second conversion-name classifier.
  - Migrated conversion-source tests from rendered prose checks to exact payload assertions; expected accepted source lists are built from `ConversionTarget::accepted_source_types()` rather than duplicated in tests.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project conversion` passed with 9 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 370 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer verified the conversion emitters and suppression matches, checked `string` versus `ErrorCode` behavior, scanned manifests and unsafe usage, reran the conversion filter with 9 tests, and reran `cargo test -p marrow-check --test project` with 370 tests using the soundness target dir.
  - Idiom/spec review: pass, no findings. Reviewer confirmed the three-file scope, one owner for conversion accepted-source facts, no duplicated accepted-source classifier in tests, no compatibility glue or comment sediment, no manifest/lock drift, and reran the conversion filter with the idiom target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the three changed checker-core files above.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - Conversion-family scans found the new `ConversionUnsupportedSource` payload sites and no conversion-source `message.contains` assertions or conversion-source emitter left on `DiagnosticPayload::None`; the then-remaining `message.contains` assertions in `project.rs` were the known non-conversion interpolation, identity-return, and enum-assignment backlog, later migrated by the interpolation-source and type-mismatch slices.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core rebase origin/main` first rebased the reviewed source cleanly to `3ace8fc58fe27a32aa0431fe0e0f027d0e96608f`.
  - Main was fast-forwarded to the then-live `origin/main` `4dc0df006cf0a49787de043b8e1a519c0a925b51`, then fast-forwarded to `3ace8fc58fe27a32aa0431fe0e0f027d0e96608f`; the first full main gate passed, but `origin/main` advanced to `7cb179e616b8d83b97358d129434d54449cad152` before push.
  - `git -C /Users/scottwilliams/Dev/marrow rebase origin/main` replayed the reviewed source commit on top of `7cb179e616b8d83b97358d129434d54449cad152`, producing final source commit `15dc6e474dc0904ffd8d0885546b164c0612552e`.
  - On the final combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source-2 cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the final combined head, `rg -n 'unsafe\s*\{|unsafe\s+fn|unsafe\s+impl|unsafe\s+trait' --glob '*.rs'` returned no matches.
  - On the final combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source-2 cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the final combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source-2 cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the final combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-conversion-source-2 cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After a final `git -C /Users/scottwilliams/Dev/marrow fetch origin`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main` pushed `main` from `7cb179e` to `15dc6e4`.
  - After the main push, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus this tracker edit and the same unrelated untracked files.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core rebase origin/main` skipped the previously applied source commit, leaving the L06 branch at `15dc6e4`; `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane branch from `9384185` to `15dc6e4`.

## L06 Interpolation Source Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-15`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-15`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-interpolation-source`.
- Base/head:
  - Lane and integration base: `20a662b23bcd447936c0343db19ea354e6b86563`.
  - Final lane and main source commit: `4fe3da56ca4575bc0ac71a07770be2dae14feb14`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/infer.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: after updating the bytes and enum interpolation assertions to expect typed payloads, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project interpolation` failed because `DiagnosticPayload::InterpolationUnsupportedSource` did not exist.
  - GREEN: after adding the interpolation payload and emitter, the same interpolation filter passed with 2 tests.
- Source/test changes:
  - Added `DiagnosticPayload::InterpolationUnsupportedSource { source: MarrowType }` for values that string interpolation cannot render.
  - Updated the interpolation type check in `infer.rs` to emit the typed payload while preserving `check.operator_type` and the rendered diagnostic text.
  - Updated incomplete-module and configured-test suppression matches so interpolation-source payloads remain non-suppressed semantic diagnostics.
  - Migrated the bytes interpolation and enum interpolation tests from rendered prose checks to exact payload assertions.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project interpolation` passed with 2 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 370 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer verified that interpolation no longer shares the generic operator payload, probed bytes and enum unsupported-source cases, reran the focused interpolation filter, reran `cargo test -p marrow-check`, reran workspace tests, and reran workspace clippy with the soundness target dir.
  - Idiom/spec review: pass, no findings. Reviewer confirmed the four-file L06 scope, the payload shape matched existing diagnostic payload idioms, there was no manifest or lock churn, no compatibility glue, and reran the interpolation filter, fmt, and clippy with the idiom target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` before commit listed only the four changed checker-core files above.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow-check/Cargo.toml` returned no output.
  - `rg -n 'message\.contains\("bytes"\)|message\.contains\("Color"\)|InterpolationUnsupportedSource|interpolation_unsupported_source|CHECK_OPERATOR_TYPE|payload: DiagnosticPayload::None|\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/infer.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` found the new interpolation payload sites and unrelated non-interpolation `DiagnosticPayload::None` diagnostics, with no old bytes or Color prose assertions and no unsafe usage.
  - `rg -n 'operator_diagnostic\(|interpolation' crates/marrow-check/src/infer.rs crates/marrow-check/src/checks.rs` found interpolation routed through `interpolation_unsupported_source_diagnostic`; remaining `operator_diagnostic` call sites are ordinary operator checks in `checks.rs`.
- Integration gates:
  - After source review, `origin/main` was still `20a662b23bcd447936c0343db19ea354e6b86563`; main fast-forwarded to `4fe3da56ca4575bc0ac71a07770be2dae14feb14`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-interpolation-source cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n 'unsafe\s*\{|unsafe\s+fn|unsafe\s+impl|unsafe\s+trait' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-interpolation-source cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-interpolation-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-interpolation-source cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After a final `git -C /Users/scottwilliams/Dev/marrow fetch origin`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main` pushed `main` from `20a662b` to `4fe3da5`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane branch from `20a662b` to `4fe3da5`.

## L06 Type Mismatch Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-16`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-16`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-type-mismatch`.
- Base/head:
  - Lane and integration base: `7d0e9cb38977a62f9de840b975a398b603bed743`.
  - Final lane and main source commit: `8a403163567160cbdf6f198eaa790f58bb805297`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/project.rs`
- Failing-or-focused checks:
  - Initial RED: after updating the two remaining checker-core `project.rs` rendered-message assertions to expect typed payloads, the focused tests failed because `DiagnosticPayload::TypeMismatch` did not exist.
  - GREEN: after adding the type-mismatch payload and emitters, the focused tests for `multiple_stores_over_one_resource_keep_distinct_identities` and `assignment_between_same_named_enums_reports_qualified_payload` each passed with 1 test.
- Source/test changes:
  - Added `DiagnosticPayload::TypeMismatch { expected: MarrowType, found: MarrowType }` for known incompatible `check.return_type` and `check.assignment_type` diagnostics.
  - Updated `check_return_type` to carry the declared return type as `expected` and returned value type as `found` while preserving the rendered diagnostic.
  - Updated `check_assignment` to carry the place type as `expected` and assigned value type as `found` while preserving the existing group-entry/resource compatibility special case and rendered diagnostic.
  - Updated incomplete-module and configured-test resolution suppression matches so type-mismatch payloads remain non-suppressed semantic diagnostics.
  - Migrated the identity-return and same-named enum assignment tests from rendered prose checks to exact payload assertions.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project multiple_stores_over_one_resource_keep_distinct_identities` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project assignment_between_same_named_enums_reports_qualified_payload` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test project` passed with 370 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer verified the only `CHECK_RETURN_TYPE` and `CHECK_ASSIGNMENT_TYPE` emitters now carry `TypeMismatch`, checked that untyped-value branches remain distinct, checked group-entry/resource assignment compatibility before emission, verified suppression behavior, reran the two focused tests, reran `cargo test -p marrow-check --test project` with 370 tests, and reran `cargo test -p marrow-check` with the soundness target dir.
  - Idiom/spec review: pass, no findings. Reviewer confirmed the three-file scope, semantic `MarrowType` payload facts rather than rendered strings, consistent `DiagnosticPayload` placement and exhaustive matches, no manifest/lock churn, and reran the two focused tests with the idiom target dir.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` before commit listed only the three changed checker-core files above.
  - `git diff --name-only -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains|rendered\.contains' crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/project.rs` returned no matches.
  - `rg -n -U -P 'code: CHECK_(RETURN_TYPE|ASSIGNMENT_TYPE),[\s\S]{0,500}?payload: DiagnosticPayload::None' crates/marrow-check/src/checks.rs` returned no matches.
- Integration gates:
  - After source review, `origin/main` was still `7d0e9cb38977a62f9de840b975a398b603bed743`; main fast-forwarded to `8a403163567160cbdf6f198eaa790f58bb805297`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-type-mismatch cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n 'unsafe\s*\{|unsafe\s+fn|unsafe\s+impl|unsafe\s+trait' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-type-mismatch cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-type-mismatch cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-type-mismatch cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After a final `git -C /Users/scottwilliams/Dev/marrow fetch origin`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main` pushed `main` from `7d0e9cb` to `8a40316`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane branch from `7d0e9cb` to `8a40316`.

## L06 Reserved Catalog Payload Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-17`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-17`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-reserved-catalog`.
- Base/head:
  - Initial worker base: `3d6cf6822465b49066356f295196fb0f17586ddf`.
  - Live integration base after rebase: `6d9a8b29a4359818cbd379e8e0c6189ab93169e3`.
  - Final lane and main source commit: `28afaf1d56f27dc2913f688a0e203fec15a43c75`.
- Changed files:
  - `crates/marrow-check/src/lib.rs`
  - `crates/marrow-check/src/catalog.rs`
  - `crates/marrow-check/src/checks.rs`
  - `crates/marrow-check/tests/catalog_presence.rs`
- Failing-or-focused checks:
  - Initial RED: after updating `reserved_catalog_path_blocks_source_reuse_without_intent` to expect a typed payload, the focused test failed because `DiagnosticPayload::ReservedCatalogPathReuse` did not exist.
  - GREEN: after adding the payload and emitter, the focused reserved-catalog test passed with 1 test.
- Source/test changes:
  - Added `DiagnosticPayload::ReservedCatalogPathReuse { source_kind, source_path, reserved_stable_id }` for `check.catalog_intent` diagnostics where source tries to reuse a reserved catalog path.
  - Updated `push_reserved_reuse` to carry the source catalog kind/path and matched reserved stable id while preserving the rendered diagnostic.
  - Updated incomplete-module and configured-test resolution suppression matches so reserved catalog path reuse remains a non-suppressed catalog diagnostic.
  - Migrated `reserved_catalog_path_blocks_source_reuse_without_intent` from `message.contains("reserved")` to exact payload presence, then tightened it after idiom review so the assertion does not depend on first `CHECK_CATALOG_INTENT` ordering.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test catalog_presence reserved_catalog_path_blocks_source_reuse_without_intent` passed with 1 test on the initial and rebased heads.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --test catalog_presence` passed with 69 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check` passed with 362 `project.rs` tests and all package tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core/Cargo.toml -p marrow-check --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer verified the single `push_reserved_reuse` path emits the payload with source kind/path and reserved stable id, confirmed the suppression filters keep it non-suppressible, checked no manifest/lock or unsafe churn, and reran the focused and `catalog_presence` tests with the soundness target dir.
  - Idiom/spec review: first failed on test shape because the assertion selected the first broad `CHECK_CATALOG_INTENT` before comparing payload. The test was fixed to search for the exact expected payload under the catalog-intent code, and re-review passed with no findings.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only the four changed checker-core files above.
  - `git diff --name-only -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains\("reserved"\)|rendered\.contains|push_reserved_reuse[\s\S]{0,700}?payload: DiagnosticPayload::None' crates/marrow-check/src/catalog.rs crates/marrow-check/tests/catalog_presence.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-check/src/lib.rs crates/marrow-check/src/catalog.rs crates/marrow-check/src/checks.rs crates/marrow-check/tests/catalog_presence.rs` returned no matches.
- Integration gates:
  - After source review, `origin/main` had advanced to `6d9a8b29a4359818cbd379e8e0c6189ab93169e3`; the reviewed lane commit rebased cleanly to `28afaf1d56f27dc2913f688a0e203fec15a43c75`.
  - Main fast-forwarded to `origin/main` `6d9a8b29a4359818cbd379e8e0c6189ab93169e3`, then fast-forwarded to `28afaf1d56f27dc2913f688a0e203fec15a43c75`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-reserved-catalog cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n 'unsafe\s*\{|unsafe\s+fn|unsafe\s+impl|unsafe\s+trait' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-reserved-catalog cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-reserved-catalog cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-main-integration-reserved-catalog cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - After a final `git -C /Users/scottwilliams/Dev/marrow fetch origin`, `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main [ahead 1]` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md` and `docs/superpowers/`.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main` pushed `main` from `6d9a8b2` to `28afaf1`.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core push origin rust-hardening-l06-checker-core` pushed the lane branch from `3d6cf68` to `28afaf1`.

## L10 Runtime Throw Field Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-review-soundness-1`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-review-idiom-1`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-main-integration`.
- Base/head:
  - Initial worker base: `f7501f90c77edc95ae08297ac9d39583c79e6cac`.
  - Live rebase/integration base: `fe51bc62435437251607f77938150c766bbde7e6`.
  - Final lane and main source commit: `c428dd58c5af38de73623f3b806cbc842783c9c0`.
- Changed files:
  - `crates/marrow-run/tests/eval.rs`
- Failing-or-focused checks:
  - Initial RED: after changing `std_assert_fail_raises_with_its_message` to call `error_throw_fields`, the focused test failed because the helper did not exist.
  - GREEN: after adding the helper and migrating all seven targeted assertions, each focused runtime throw-field test passed.
- Source/test changes:
  - Added a small `error_throw_fields` test helper that reads `RuntimeError.throw` as `Value::Resource` and fails closed for missing throws, non-resource throws, missing `code` or `message`, and non-string fields.
  - Migrated `std_assert_fail_raises_with_its_message`, `finally_runs_after_a_fault_and_can_replace_it`, `a_throw_in_finally_replaces_the_outcome`, and four `run_entry_rejects_host_values_*` tests from rendered `RuntimeError.message.contains` checks to exact Error resource `code` and `message` field assertions.
  - No production code, manifests, lockfile, or docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval std_assert_fail_raises_with_its_message` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval finally_runs_after_a_fault_and_can_replace_it` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval a_throw_in_finally_replaces_the_outcome` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval run_entry_rejects_host_values_that_do_not_match_checked_parameters` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval run_entry_rejects_host_values_for_moded_parameters` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval run_entry_rejects_host_values_for_identity_parameters` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval run_entry_rejects_host_values_for_resource_parameters` passed with 1 test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --test eval` passed with 422 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l10-runtime-core/Cargo.toml -p marrow-run --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer reran focused throw-field tests, checked the helper fails closed for malformed throw values, verified the targeted `message.contains` assertions were absent, and confirmed only `crates/marrow-run/tests/eval.rs` changed.
  - Idiom/spec review: pass, no findings. Reviewer reran fmt, clippy, and the exact seven focused tests, inspected the helper shape and local style, found no stronger public typed helper available to integration tests, and confirmed no production or manifest churn.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only `crates/marrow-run/tests/eval.rs`.
  - `git diff --name-only -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains\("boom"\)|message\.contains\("cleanup\.failed"\)|message\.contains\("from\.finally"\)|message\.contains\("entry argument `n`"\)|message\.contains\("inout"\)|message\.contains\("entry argument `id`"\)|message\.contains\("entry argument `book`"\)' crates/marrow-run/tests/eval.rs` returned no matches.
  - `rg -n 'message\.contains' crates/marrow-run/tests/eval.rs` returned no matches.
  - `rg -n '\bunsafe\b' crates/marrow-run/tests/eval.rs` returned no matches.
- Integration gates:
  - After source review, `origin/main` had advanced to `fe51bc62435437251607f77938150c766bbde7e6`; the reviewed lane commit rebased cleanly to `c428dd58c5af38de73623f3b806cbc842783c9c0`.
  - Main fast-forwarded to `origin/main` `fe51bc62435437251607f77938150c766bbde7e6`, then fast-forwarded to `c428dd58c5af38de73623f3b806cbc842783c9c0`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main rust-hardening-l10-runtime-core` pushed `main` from `fe51bc6` to `c428dd5` and created `origin/rust-hardening-l10-runtime-core` at `c428dd5`.

## L14 CLI Diagnostic Test-Support Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-4`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-4`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration`.
- Base/head:
  - Lane base: `13852686ed8317e1b567f941ff160de345738d3b`.
  - Lane source commits: `86ff6e4f4647e5f88f2ea8abe9a0598961fb5c94`, `79adda94db4aae1e733f1d17a5582d971cce1293`.
  - Main source integration commits: `14f71ea`, `1b22287`.
- Changed files:
  - `crates/marrow/tests/check_cli.rs`
  - `crates/marrow/tests/check_project_cli.rs`
  - `crates/marrow/tests/data_cli.rs`
  - `crates/marrow/tests/evolve_cli.rs`
  - `crates/marrow/tests/support/mod.rs`
- Source changes:
  - Moved semantic CLI diagnostic assertions from stderr/stdout prose scraping to JSON/JSONL `code`, status, source path/span, catalog ID, populated count, repair-required, approval-required, schema-drift, and leakage-boundary assertions where structured output exists.
  - Added shared `support::json`, `support::jsonl`, and `support::codes` helpers for integration tests.
  - Kept text assertions only for rendered human output, usage/help output, explicit path/value display, and negative leakage or absence checks.
  - No production code, manifests, or lockfile changed.
- Focused gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow --test check_project_cli` passed with 24 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow --test check_cli --test data_cli --test evolve_cli` passed with 62 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow --test evolve_cli --test data_cli` passed with 45 tests after L14-R001 and L14-R002 were fixed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow --test evolve_cli` passed with 19 tests after the final assertion-shape cleanup.
- Lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - A concurrent fmt/clippy run against the same lane target dir was discarded as evidence; both commands above were rerun sequentially with the explicit target dir.
- Soundness review: passed first review on `86ff6e4f4647e5f88f2ea8abe9a0598961fb5c94`, then passed re-review on `79adda94db4aae1e733f1d17a5582d971cce1293` after the idiom/spec findings were fixed. Reviewer reran the four focused CLI suites and verified only the five test/support files changed.
- Idiom/spec review: failed first review on L14-R001 and L14-R002, then passed re-review on `79adda94db4aae1e733f1d17a5582d971cce1293`.
- Fixed review findings:
  - L14-R001: two `evolve apply` tests still scraped `evolve.repair_required` and `run.schema_drift` from stderr. Fixed by running apply with `--format json` and asserting parsed JSON `code`.
  - L14-R002: `data_integrity_reports_an_orphan_problem_with_a_tooling_kind` hand-parsed stdout and duplicated problem lookup. Fixed by using the local `json` and `integrity_problem` helpers, while keeping leakage checks on serialized parsed JSON.
- Absence and sibling scans:
  - `git grep -n "unsafe" -- crates/marrow/tests/check_cli.rs crates/marrow/tests/check_project_cli.rs crates/marrow/tests/data_cli.rs crates/marrow/tests/evolve_cli.rs crates/marrow/tests/support/mod.rs` returned no matches.
  - `git diff -- Cargo.lock Cargo.toml crates/marrow/Cargo.toml` returned no output.
  - `git diff --check` returned no output.
  - `rg -n 'contains\(|stderr|stdout|message\(' ...changed files...` found only structured JSON helper use, rendered text checks, usage/help checks, explicit path/value display checks, and negative leakage or absence checks after L14-R001 and L14-R002.
  - Untouched L14 source and sibling CLI/LSP/serve tests remain unreviewed and stay owned by L14 backlog scope.
- Integration gates:
  - `git -C /Users/scottwilliams/Dev/marrow fetch origin` completed before integration; `git -C /Users/scottwilliams/Dev/marrow rev-parse HEAD` and `git -C /Users/scottwilliams/Dev/marrow rev-parse origin/main` both returned `13852686ed8317e1b567f941ff160de345738d3b`.
  - `git -C /Users/scottwilliams/Dev/marrow status --short --branch` showed `## main...origin/main` plus unrelated untracked `docs/roadmap/release-hardening-operating-plan.md`.
  - `git -C /Users/scottwilliams/Dev/marrow cherry-pick -x 86ff6e4f4647e5f88f2ea8abe9a0598961fb5c94 79adda94db4aae1e733f1d17a5582d971cce1293` produced `14f71ea` and `1b22287`.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.

## L14 Trace JSONL Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli`.
  - Initial soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-trace-1`.
  - Initial idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-trace-1`.
  - Re-review soundness: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-trace-2`.
  - Re-review idiom/spec: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-trace-2`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-main-integration`.
- Base/head:
  - Initial lane base: `f54157c4fa3eddcdc4ac102bd5a346488cc64592`.
  - Live rebase/integration base: `a38c74afdd8456ceecef5deb230e087349c5269c`.
  - Final lane and main source commit: `4a3e473bbf6a9465c46e8d0fd05f9966e1fe2f1a`.
- Changed files:
  - `crates/marrow/tests/trace_cli.rs`
- Failing-or-focused checks:
  - Initial RED: after changing `run_trace_json_emits_step_and_write_records` to call a not-yet-added `trace_record` helper, the focused test failed with `E0425 cannot find function trace_record in this scope`.
  - GREEN: after adding structured JSONL assertions and the local mixed-output helper, the focused JSONL run trace and test trace label tests passed.
  - Review fix: idiom/spec review found the JSONL run trace test selected records by kind and could miss write-before-step ordering regressions. The test was fixed to destructure the records as `[step, write, summary]` and assert each record's kind at that position.
- Source/test changes:
  - Replaced ad hoc JSONL parsing in `run_trace_json_emits_step_and_write_records` with `support::jsonl` and exact structured assertions for the ordered step, write, and summary records.
  - Added exact checks for trace label, source line, depth, write op/path, base64 value, target kind/store/identity/path, and summary event count.
  - Migrated `test_trace_labels_each_test` to `marrow test --trace --format jsonl` and asserted structured step and summary trace labels for both tests.
  - Left text `contains` checks in `trace_cli.rs` only for render-boundary coverage: text trace ordering, bool/int value rendering, delete rendering, help text, and plain-run no-trace behavior.
  - No production code, shared CLI support, manifests, lockfile, or docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml -p marrow --test trace_cli run_trace_json_emits_step_and_write_records` passed after the initial implementation and again after the order fix.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml -p marrow --test trace_cli test_trace_labels_each_test` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml -p marrow --test trace_cli` passed with 8 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml -p marrow` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-cli cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-cli/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer reran the two focused migrated tests and the full `trace_cli` binary, verified only `trace_cli.rs` changed, and confirmed no old ad hoc parser or test-label substring assertions remained.
  - Idiom/spec review: first failed on the missing JSONL event-order assertion. After the test destructured the records as `[step, write, summary]`, re-review passed with no findings. Reviewer reran fmt, clippy, and the full `trace_cli` binary.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only` listed only `crates/marrow/tests/trace_cli.rs`.
  - `git diff --name-only -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'trace_record|serde_json::from_str|let kinds|combined|contains\("::first"|contains\("::second"|\bunsafe\b' crates/marrow/tests/trace_cli.rs` found only the retained `jsonl_trace_records` helper and its call site, with no old ad hoc parser, label substring checks, or unsafe usage.
  - `git diff -- crates/marrow/tests/trace_cli.rs | rg -n '^\+\s*//'` returned no matches, so the slice introduced no comments.
- Integration gates:
  - Before integration, `origin/main` had advanced to `a38c74afdd8456ceecef5deb230e087349c5269c`; the reviewed source commit rebased cleanly to `4a3e473bbf6a9465c46e8d0fd05f9966e1fe2f1a`.
  - Main fast-forwarded to `origin/main` `a38c74afdd8456ceecef5deb230e087349c5269c`, then fast-forwarded to `4a3e473bbf6a9465c46e8d0fd05f9966e1fe2f1a`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main rust-hardening-l14-trace-cli` pushed `main` from `a38c74a` to `4a3e473` and created `origin/rust-hardening-l14-trace-cli` at `4a3e473`.

## L14 Usage And V01 CLI Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-usage-v01`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-usage-v01`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-usage-v01`.
- Base/head:
  - No-source lane base/head: `d732ad3a0d395a713f741c8882e0111748763284`.
  - No source commit was created.
- Reviewed files:
  - `crates/marrow/tests/usage_cli.rs`
  - `crates/marrow/tests/v01_cli.rs`
- Scope decision:
  - `usage_cli.rs` is CLI usage-failure boundary coverage. The retained `stderr.contains` assertions check user-facing usage strings where no structured output surface exists, and they are paired with exit code 2, empty stdout for top-level/run usage cases, and no-store-created coverage for the unknown `data` subcommand.
  - `v01_cli.rs` is an end-to-end durable v0.1 fixture test through the production CLI pipeline. It checks the project, seeds the native store through `marrow run`, and asserts exact final stdout from the print entry.
  - No production code, tests, shared support, manifests, lockfile, or docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-usage-v01 cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01/Cargo.toml -p marrow --test usage_cli --test v01_cli` passed with 5 `usage_cli` tests and 1 `v01_cli` test.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-usage-v01 cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-usage-v01 cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer reran the focused `usage_cli` and `v01_cli` tests, verified the worktree had no source diff, confirmed no manifest/lock/source churn, and checked that the usage text assertions are render-boundary checks rather than semantic diagnostics.
  - Idiom/spec review: pass, no findings. Reviewer reran fmt, clippy, and the focused tests, confirmed `usage_cli.rs` pairs usage text checks with exit/no-output/no-store assertions, and confirmed `v01_cli.rs` is a production-pipeline fixture test with exact final stdout.
- Absence scans:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01 status --short` returned no output.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l14-usage-v01 diff --name-status d732ad3a0d395a713f741c8882e0111748763284 HEAD` returned no output.
  - `rg -n 'message\.contains|stderr\.contains|stdout\.contains|contains\(|\bunsafe\b' crates/marrow/tests/usage_cli.rs crates/marrow/tests/v01_cli.rs` found only the five intended `stderr.contains` assertions in `usage_cli.rs`, no matches in `v01_cli.rs`, and no unsafe usage.
  - `rg -n 'legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge' crates/marrow/tests/usage_cli.rs crates/marrow/tests/v01_cli.rs` returned no matches.

## L14 Fmt And Test CLI Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-cli`.
  - Initial soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-fmt-test`.
  - Initial idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-fmt-test`.
  - Re-review soundness: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-fmt-test-2`.
  - Re-review idiom/spec: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-fmt-test-2`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-main-integration`.
- Base/head:
  - Lane base: `c1e97e1986d736377e0b0e4e2e2e146c5d2747e5`.
  - Final lane and main source commit: `d276c6a0841264b7a61b75c9ac7f77a7282139b9`.
- Changed files:
  - `crates/marrow/tests/fmt_cli.rs`
  - `crates/marrow/tests/test_cli.rs`
- Source/test changes:
  - `fmt_cli.rs` project-directory fixtures now accept multiple source files.
  - `fmt_check_on_a_project_directory_passes_when_all_files_are_formatted` covers two formatted project files.
  - `fmt_write_on_a_project_directory_rewrites_each_changed_file` rewrites and asserts both changed source files.
  - `fmt_on_a_directory_with_no_config_reports_io_read_for_config` asserts stable `io.read` and `marrow.json` text instead of a negative OS-prose substring.
  - Low-value restatement comments in the reviewed fmt/test tests were removed.
  - No production code, shared CLI support, manifests, lockfile, or non-tracker docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli/Cargo.toml -p marrow --test fmt_cli --test test_cli` passed with 15 `fmt_cli` tests and 7 `test_cli` tests after review fixes.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-cli cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-cli cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-cli cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-fmt-test-cli/Cargo.toml -p marrow` passed.
- Source review:
  - Initial soundness review: pass, no findings. Reviewer verified `marrow fmt --format json` is rejected, `marrow test --format json` still emits text result lines, and `marrow test --trace --format json` shapes only trace output.
  - Initial idiom/spec review: fail. L14-IDIOM-001 found project-directory fmt tests claimed all-file behavior while using one-file fixtures. L14-IDIOM-002 found a negative OS-prose assertion in the no-config directory test. L14-IDIOM-003 found low-value comments in the reviewed tests.
  - Re-review soundness: pass, no findings. Reviewer reran the focused tests, probed a two-file project where only `src/lib.mw` was unformatted, and verified the no-config directory reports `io.read` plus `marrow.json`.
  - Re-review idiom/spec: pass, no findings. Reviewer reran fmt, clippy, and the focused tests; verified L14-IDIOM-001 through L14-IDIOM-003 were fixed; and found no new helper overreach or sediment.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only c1e97e1986d736377e0b0e4e2e2e146c5d2747e5..d276c6a0841264b7a61b75c9ac7f77a7282139b9` listed only `crates/marrow/tests/fmt_cli.rs` and `crates/marrow/tests/test_cli.rs`.
  - `git diff --name-only c1e97e1986d736377e0b0e4e2e2e146c5d2747e5..d276c6a0841264b7a61b75c9ac7f77a7282139b9 -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge' crates/marrow/tests/fmt_cli.rs crates/marrow/tests/test_cli.rs` returned no matches.
  - `rg -n 'Is a directory|raw OS|not_is_a_directory' crates/marrow/tests/fmt_cli.rs crates/marrow/tests/test_cli.rs` returned no matches.
  - The retained `contains` assertions were reviewed as render-boundary checks: `cmd_fmt` has no structured output mode, and `cmd_test` uses `--format` only for trace output.
- Integration gates:
  - Before integration, `origin/main` was `c1e97e1986d736377e0b0e4e2e2e146c5d2747e5`; the reviewed source branch was a direct descendant and `main` fast-forwarded to `d276c6a0841264b7a61b75c9ac7f77a7282139b9`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n '\bunsafe\b' --glob '*.rs'` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-fmt-test-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.

## L14 LSP Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp`.
  - Initial soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-lsp`.
  - Initial idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-lsp`.
  - Re-review soundness: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-lsp-2`.
  - Re-review idiom/spec: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-lsp-2`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp-main-integration`.
- Base/head:
  - Lane base: `3d4929a2ab199b503f8534526e17260d65f097cb`.
  - Final lane and main source commit: `71cf0defe84e0604734a06cd2d013fed511387cf`.
- Changed files:
  - `crates/marrow/src/lsp.rs`
  - `crates/marrow/tests/lsp_cli.rs`
- Failing-or-focused checks:
  - Initial no-source LSP checks passed before review: `cargo test -p marrow --test lsp_cli`, `cargo test -p marrow lsp::tests::positions_count_utf16_code_units`, formatter, and clippy were rerun sequentially with explicit target dirs after an invalid shared-target concurrent run was discarded.
  - LSP-001 RED: after adding `rejects_an_oversized_header_line_before_reading_a_body`, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml -p marrow rejects_an_oversized_header_line_before_reading_a_body -- --nocapture` failed because the unbounded header reader accepted the oversized line.
  - LSP-002 was covered by adding an exact checker diagnostic range assertion to `did_open_in_project_publishes_checker_diagnostics`; the existing production behavior already supplied the expected range.
- Source/test changes:
  - `read_message` now bounds LSP header parsing with an 8 KiB per-line cap and 16 KiB total header cap before allocating a body.
  - LSP body allocation uses the named 64 MiB `MAX_MESSAGE_BYTES` cap.
  - New unit tests reject oversized header lines and oversized header blocks before a body is read.
  - `did_open_in_project_publishes_checker_diagnostics` now finds the `check.assignment_type` diagnostic and asserts its exact UTF-16 LSP range along with the existing code/source/severity checks.
  - No manifests, lockfile, non-LSP CLI tests, or non-tracker docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml -p marrow rejects_an_oversized_header -- --nocapture` passed with both header tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml -p marrow --test lsp_cli` passed with 20 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-lsp/Cargo.toml -p marrow` passed.
- Source review:
  - Initial soundness review: fail. LSP-001 found unbounded header-line and header-block parsing before body allocation; LSP-002 found checker diagnostics lacked representative range assertions.
  - Initial idiom/spec review: pass, no findings.
  - Re-review soundness: pass, no findings. Reviewer verified oversized header line, oversized header block, oversized body, unterminated header, malformed JSON, unknown request, and invalid-root diagnostic probes; confirmed no unbounded `read_line` path remains in `read_message`; and noted `serve` has its own separate bounded reader.
  - Re-review idiom/spec: pass, no findings. Reviewer verified the focused reader helper shape, named caps, no prose-message assertions, no dependency churn, and no sediment in the touched files.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only 3d4929a2ab199b503f8534526e17260d65f097cb..71cf0defe84e0604734a06cd2d013fed511387cf` listed only `crates/marrow/src/lsp.rs` and `crates/marrow/tests/lsp_cli.rs`.
  - `git diff --name-only 3d4929a2ab199b503f8534526e17260d65f097cb..71cf0defe84e0604734a06cd2d013fed511387cf -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains|stderr\.contains|stdout\.contains|contains\(|\bunsafe\b' crates/marrow/src/lsp.rs crates/marrow/tests/lsp_cli.rs` returned no matches.
  - `rg -n 'legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge|raw' crates/marrow/src/lsp.rs crates/marrow/tests/lsp_cli.rs` returned no matches.
- Integration gates:
  - Before integration, local `main` and `origin/main` were at `3d4929a2ab199b503f8534526e17260d65f097cb`; the reviewed source branch was a direct descendant and `main` fast-forwarded to `71cf0defe84e0604734a06cd2d013fed511387cf`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n '\bunsafe\b' --glob '*.rs'` returned no matches.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-lsp-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.

## L14 Trace Source Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-trace-source`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-trace-source`.
- Base/head:
  - No-source lane base/head: `de9c2f52a5e72d1c808c876ef9cf2c6952a20d62`.
  - No source commit was created.
- Reviewed files:
  - `crates/marrow/src/trace.rs`
  - Context: `crates/marrow/tests/trace_cli.rs`, `crates/marrow/src/dry_run.rs`, `crates/marrow/src/cmd_run.rs`, `crates/marrow/src/cmd_test.rs`.
- Scope decision:
  - `trace.rs` is a presentation boundary over typed runtime `StepHook`, frame, write-target, saved-key, and saved-value structures.
  - Text trace output streams to stderr and leaves program stdout untouched; JSON/JSONL trace output is collected until `flush`, then emitted as structured records plus a JSONL summary.
  - Write-target rendering uses stable checked facts for human store/member/index names where available while retaining typed runtime/store structures before rendering; key rendering is trace presentation, not storage identity.
  - Leaf value rendering decodes only bool fields for text readability; non-bool scalar, enum, identity, and byte values render from stored bytes for text, while JSON keeps the exact written bytes in `value_b64`.
  - No production code, tests, manifests, lockfile, or non-tracker docs changed in the source lane.
- Focused and lane gates:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source/Cargo.toml -p marrow --test trace_cli` passed with 8 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source/Cargo.toml -p marrow trace` passed, including `dry_run_composes_with_trace` and all 8 `trace_cli` tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-trace-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source/Cargo.toml -p marrow` passed.
- Source review:
  - Soundness review: pass, no findings. Reviewer inspected `trace.rs`, `trace_cli.rs`, production run/test/dry-run callers, and runtime write paths; reran focused trace tests with the soundness target dir; and used temporary CLI probes for string/date/duration/instant/bytes keys, data writes, index writes, bool text rendering, non-UTF8 bytes rendering, int rendering, and `value_b64` byte preservation.
  - Idiom/spec review: pass, no findings. Reviewer inspected `trace.rs`, `trace_cli.rs`, and production callers; reran formatter, clippy, and `trace_cli`; verified `pub(crate)` trace surfaces have production callers; and found no oversized dispatcher, duplicate semantic classifier, dependency churn, or comment sediment.
- Absence and sibling scans:
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source status --short` returned no output.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source diff --name-status de9c2f52a5e72d1c808c876ef9cf2c6952a20d62 HEAD` returned no output.
  - `git -C /Users/scottwilliams/Dev/marrow-rust-hardening-l14-trace-source diff --name-only origin/main..HEAD -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains|stderr\.contains|stdout\.contains|contains\(|\bunsafe\b' crates/marrow/src/trace.rs crates/marrow/tests/trace_cli.rs` found no matches in `trace.rs`; retained `trace_cli.rs` matches are render-boundary/help checks already reviewed in the trace CLI slice.
  - `rg -n 'legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge|raw' crates/marrow/src/trace.rs crates/marrow/tests/trace_cli.rs` found only durable `trace.rs` raw-byte rendering comments; no legacy, prototype, compatibility, temporary, TODO, FIXME, fallback, or bridge surface.
- Integration notes:
  - No source integration was required.
  - Tracker-only evidence was recorded on `main` after both source reviewers returned pass.

## L14 Cmd Fmt Source Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source`.
  - Initial soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-cmd-fmt-source`.
  - Initial idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-cmd-fmt-source`.
  - Re-review soundness: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-cmd-fmt-source-2`.
  - Re-review idiom/spec: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-cmd-fmt-source-2`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-main-integration`.
- Base/head:
  - Lane base: `103d39a446ecb26d6e4b4d6c051b272c5b02ad15`.
  - Final lane and main source commit: `f67f1f20972217b8c0c5b4348a410513ab51d046`.
- Changed files:
  - `crates/marrow/src/cmd_fmt.rs`
  - `crates/marrow/tests/fmt_cli.rs`
- Failing-or-focused checks:
  - Initial focused baseline passed before review: `fmt_cli` had 15 tests, `cargo test -p marrow cmd_fmt`, formatter, clippy, and `cargo test -p marrow` all passed with the lane target dir.
  - Soundness review then found write failures were reported as `io.read`. After adding the two regression tests and before the production fix, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml -p marrow --test fmt_cli fmt_write -- --nocapture` failed in both new tests with `io.read: failed to read ... Permission denied`.
  - After the production fix, the same focused filter passed with 5 selected tests, including both new write-failure regressions.
- Source/test changes:
  - `fmt_one` now reports `std::fs::write` failures through `report_simple_error("io.write", "failed to write ...", Text)` instead of the read-specific `report_io_error`.
  - Added Unix-only readonly-file regression coverage for single-file `marrow fmt --write` failures.
  - Added Unix-only project-directory `--write` regression coverage proving a readonly source reports `io.write` and a writable sibling is still formatted before the command fails overall.
  - No manifests, lockfile, shared CLI support, or non-tracker docs changed in the source lane.
- Focused and lane gates after fix:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml -p marrow --test fmt_cli fmt_write -- --nocapture` passed with 5 selected tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml -p marrow --test fmt_cli` passed with 17 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-cmd-fmt-source/Cargo.toml -p marrow` passed.
- Source review:
  - Initial soundness review: fail. Reviewer found `fmt_one` reported write failures with `io.read`, reprobed single-file and project-directory `--write`, and required typed `io.write` coverage for both paths.
  - Initial idiom/spec review: pass, no findings. Reviewer verified the source shape and judged the existing raw-OS directory comment as durable CLI error-contract rationale, not sediment.
  - Re-review soundness: pass, no findings. Reviewer reran the `fmt_write` filter, full `fmt_cli`, direct readonly-file and readonly-project probes, missing-file and missing-config `io.read` probes, duplicate-mode/stdin usage probes, and directory source-root/per-file-failure probes.
  - Re-review idiom/spec: pass, no findings. Reviewer reran formatter, clippy, and `fmt_cli`; verified the Unix-only test cfg shape; and found no compatibility shim, fallback path, dependency churn, or test-only production entrypoint.
- Absence and sibling scans:
  - `git diff --check` returned no output.
  - `git diff --name-only 103d39a446ecb26d6e4b4d6c051b272c5b02ad15..f67f1f20972217b8c0c5b4348a410513ab51d046` listed only `crates/marrow/src/cmd_fmt.rs` and `crates/marrow/tests/fmt_cli.rs`.
  - `git diff --name-only 103d39a446ecb26d6e4b4d6c051b272c5b02ad15..f67f1f20972217b8c0c5b4348a410513ab51d046 -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n 'message\.contains|stderr\.contains|stdout\.contains|contains\(|\bunsafe\b|legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge|raw' crates/marrow/src/cmd_fmt.rs` found only the existing durable raw-OS directory comment.
  - Retained `fmt_cli.rs` text assertions were reviewed as render and usage boundary checks because `marrow fmt` has no structured output mode.
- Integration gates:
  - Before integration, local `main` and `origin/main` were at `103d39a446ecb26d6e4b4d6c051b272c5b02ad15`; the reviewed source branch was a direct descendant and `main` fast-forwarded to `f67f1f20972217b8c0c5b4348a410513ab51d046`.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On the combined head, `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On the combined head, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cmd-fmt-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.

## L14 Dry Run Source Blocker Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-dry-run-source`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-dry-run-source`.
- Base/head:
  - Lane base: `40767f054cb522af74062d9ac27665a13225715a`.
  - No source commit was created because source review failed and the blocking fix overlaps an active dirty worktree.
- Reviewed files:
  - `crates/marrow/src/dry_run.rs`
  - `crates/marrow/tests/dry_run_cli.rs`
  - Context: `crates/marrow/src/cmd_run.rs`, `crates/marrow/src/trace.rs`, and `crates/marrow/tests/run_cli.rs`.
- Focused and lane gates before review:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml -p marrow --test dry_run_cli` passed with 7 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml -p marrow dry_run` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml -p marrow --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml -p marrow` passed.
- Source review:
  - Soundness review: fail. Reviewer found `--dry-run --trace --format jsonl` mixes program stdout with JSONL trace/dry-run records on stdout, so stdout is neither parseable JSONL tooling output nor pure program output. Required fix: keep program stdout separate from JSON/JSONL tooling reports and add a regression with `print(...)` plus `--dry-run --trace --format jsonl`.
  - Soundness review also found dry-run's byte-for-byte wording too strong for native redb store files: a traced maintenance dry-run root delete changed `.data` file hashes while `data dump` stayed unchanged. Required fix: implement true native file byte stability or narrow wording/tests to logical saved data and cell contents.
  - Idiom/spec review: fail. Reviewer found JSON dry-run plan coverage asserted `path.contains("title")` and `path.contains("pages")` instead of the structured `target` object, and found a `PlannedWrite` doc comment describing a human path even though it stores a typed `WriteTarget`.
- Partial local cleanup, not integrated:
  - The lane locally changed `dry_run_cli.rs` to assert structured target kind, store, identity, and path member for `title` and `pages`.
  - The lane locally changed the `PlannedWrite` doc comment to describe a typed target and trace-shared rendering.
  - After those local edits, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-dry-run-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-dry-run-source/Cargo.toml -p marrow --test dry_run_cli`, formatter, and clippy all passed. These edits remain unintegrated because the soundness-critical `cmd_run.rs` fix is blocked.
- Blocking dependency:
  - `git -C /Users/scottwilliams/Dev/marrow-engine-resident-catalog status --short -- crates/marrow/src/cmd_run.rs` reported `M  crates/marrow/src/cmd_run.rs`.
  - The active `marrow-engine-resident-catalog` worktree has staged `cmd_run.rs` changes around dry-run transaction handling. The L14 dry-run source lane cannot safely own the JSON/JSONL stream-separation fix until that worktree integrates or releases the file.
- Absence and cleanup evidence:
  - `git diff --check` in the dry-run lane passed with no output before retirement.
  - `git diff --stat` listed only `crates/marrow/src/dry_run.rs` and `crates/marrow/tests/dry_run_cli.rs`, both partial local cleanup edits.
  - The blocker is tracked as B005; `cmd_run.rs`, `dry_run.rs`, and `dry_run_cli.rs` are blocked in the file inventory until the stream-separation lane can run.

## L10/L14 Base64 Canonicality And Serve Protocol Evidence

- Worktree: `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source`.
- Target dirs:
  - Lane: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source`.
  - Soundness review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-serve-protocol-source`.
  - Idiom/spec review: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-serve-protocol-source`.
  - Main integration: `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration`.
- Base/head:
  - Lane base: `451df5e4256dde7a04a1a015930a99e8fa348fdb`.
  - Source and main commit: `1735ff618513bc54dcdba99ef5e43c216efe1396`.
- Changed files:
  - `crates/marrow-run/src/base64.rs`
  - `crates/marrow/src/serve/protocol/tests.rs`
- Reviewed files and context:
  - Source reviewed clean before the finding: `crates/marrow/src/serve/protocol.rs`, `crates/marrow/src/serve/protocol/codec.rs`, `crates/marrow/src/serve/protocol/cursor.rs`, `crates/marrow/src/serve/protocol/data.rs`, `crates/marrow/src/serve/protocol/tests.rs`, and `crates/marrow/src/serve/protocol/walk.rs`.
  - Context: `crates/marrow/src/serve/mod.rs`, `crates/marrow/tests/serve_cli.rs`, and shared `crates/marrow-run/src/base64.rs`.
- Failing-or-focused checks:
  - Baseline before review passed: `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow serve::protocol` passed with 48 tests; `cargo test -p marrow --test serve_cli`, formatter, clippy, and `cargo test -p marrow` also passed.
  - Soundness review then found `marrow_run::base64::decode` accepted non-canonical padded spellings with non-zero unused bits: `Zh==` decoded as `f` and `Zm9=` decoded as `fo`.
  - After adding the owner-level regression and before the production fix, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow-run base64 -- --nocapture` failed in `base64::tests::rejects_non_canonical_padding` on `Zh==`.
- Source/test changes:
  - `marrow_run::base64::decode` now rejects non-zero unused bits when the final group is padded: `xx==` requires the lower four bits of the second sextet to be zero, and `xxx=` requires the lower two bits of the third sextet to be zero.
  - `marrow-run` base64 tests now cover `Zh==` and `Zm9=` as rejected non-canonical forms.
  - Serve protocol base64 tests also cover `Zh==` and `Zm9=` through `decode_base64_field`, proving bytes keys and cursor envelopes inherit the shared strict decoder.
  - No manifests, lockfile, dependencies, public API, or non-tracker docs changed.
- Focused and lane gates after fix:
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow-run base64 -- --nocapture` passed, including 3 base64 unit tests and 3 selected runtime eval tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow serve::protocol::tests::serve_base64_decode_rejects_non_canonical_padding -- --nocapture` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow serve::protocol -- --nocapture` passed with 48 tests.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml --all -- --check` passed with no output.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow-run` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml -p marrow` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-serve-protocol-source cargo test --manifest-path /Users/scottwilliams/Dev/marrow-rust-hardening-l14-serve-protocol-source/Cargo.toml --workspace` passed.
- Source review:
  - Initial soundness review: fail. Reviewer found the non-canonical base64 pad-bit issue and an out-of-scope high issue where an oversized serve request line can leave a suffix to be parsed as a follow-up request.
  - Initial idiom/spec review: pass for the no-source serve protocol shape.
  - Re-review soundness: pass for the in-scope base64 fix. Reviewer reran focused tests, probed canonical `Zg==`/`Zm8=` acceptance and `Zh==`/`Zm9=` rejection, and exhaustively checked canonical one-byte/two-byte padded encodings versus non-zero pad-bit variants.
  - Re-review idiom/spec: pass. Reviewer verified strict base64 remains owned by `marrow_run::base64`, serve still delegates through `decode_base64_field`, tests assert structured behavior, and no dependency churn, unsafe, prototype, fallback, compatibility shim, or raw production surface was introduced.
- Deferred blocker:
  - The oversized-line finding is tracked as B006 because `git -C /Users/scottwilliams/Dev/marrow-engine-resident-catalog status --short -- crates/marrow/src/serve/mod.rs` reported `M  crates/marrow/src/serve/mod.rs`.
  - The active `marrow-engine-resident-catalog` worktree has staged `serve/mod.rs` changes, so this slice did not edit connection framing.
- Absence and sibling scans:
  - `git diff --check` passed with no output.
  - `git diff --stat` listed only `crates/marrow-run/src/base64.rs` and `crates/marrow/src/serve/protocol/tests.rs`.
  - `git diff --name-only -- Cargo.lock ':(glob)**/Cargo.toml'` returned no output.
  - `rg -n '\bunsafe\b|message\.contains|stderr\.contains|stdout\.contains|error\.to_string\(\)\.contains|assert!\([^\n]*contains|legacy|prototype|compat|temporary|TODO|FIXME|fallback|bridge|raw' crates/marrow-run/src/base64.rs crates/marrow/src/serve/protocol/tests.rs` found only the existing `raw_data_ops_are_not_production_protocol_ops` absence test.
- Integration gates:
  - Before integration, local `main` and `origin/main` were at `451df5e4256dde7a04a1a015930a99e8fa348fdb`; `main` fast-forwarded to `1735ff618513bc54dcdba99ef5e43c216efe1396`.
  - On main at `1735ff618513bc54dcdba99ef5e43c216efe1396`, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration cargo fmt --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --all -- --check` passed with no output.
  - On main, `rg -n '\bunsafe\b' --glob '*.rs' /Users/scottwilliams/Dev/marrow` returned no matches through a wrapper that treats no matches as success.
  - On main, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration cargo build --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On main, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration cargo test --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace` passed.
  - On main, `CARGO_TARGET_DIR=/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-base64-main-integration cargo clippy --manifest-path /Users/scottwilliams/Dev/marrow/Cargo.toml --workspace --all-targets -- -D warnings` passed.
  - `git -C /Users/scottwilliams/Dev/marrow push origin main` pushed `451df5e..1735ff6`.
