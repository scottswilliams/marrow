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
| Message-parsing logic | L03 syntax, L04 schema, L05 project-model, L12 store, and L13 backup/restore have no `message.contains` semantic assertions after integration. L06 schema-payload, duplicate-root, duplicate-declaration, duplicate-module, module-path, and rejected-surface slices migrated the checker assertions they touched; remaining checker/runtime/tooling areas still need lane-local migration. | needs-lane | L06-L11, L14 |
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
| L06 checker-core | `/Users/scottwilliams/Dev/marrow-rust-hardening-l06-checker-core` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-checker-core`; schema reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-1` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-1`; duplicate-root reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-2` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-2`; duplicate-declaration reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-3` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-3`; duplicate-module reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-4` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-4`; module-path reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-5` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-5`; rejected-surface reviews `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-soundness-6` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l06-review-idiom-6`; main targets recorded per slice | first slice base `580f92ad684840c30833dc025d3a908a5aaadc2c`; latest slice base `9e03d4290468858a99ef1e9bccc22420e7b94328` | latest lane and main `b7402e173580c6ff4e2532ed4a9fd1eeb6c34064` | in-lane | focused schema payload, duplicate-root, duplicate-declaration, duplicate-module, module-path, and rejected-surface tests, package test, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings for integrated slices | pass, no findings for integrated slices | no review findings | schema diagnostic payload, duplicate-root owner payload, duplicate-declaration payload, duplicate-module payload, module-path payload, and rejected-surface payload slices integrated; broader L06 files remain in lane |
| L07 checker-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l07-checker-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L08 checker-presence | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l08-checker-presence` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L09 checker-tooling | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l09-checker-tooling` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L10 runtime-core | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l10-runtime-core` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L11 runtime-evolution | pending | `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l11-runtime-evolution` | pending | pending | unreviewed | pending | pending | pending | pending | pending |
| L12 store | `/Users/scottwilliams/Dev/marrow-rust-hardening-l12-store` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-store`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-soundness` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-review-idiom`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l12-main-integration` | `e3690d46d5cebb760728dfb20b49cd52d0806c2b` | no source commit; tracker evidence recorded | complete | focused store/default/native checks, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings | pass, no findings | no review findings | no source cherry-pick required; main integration gates passed |
| L13 backup-restore | `/Users/scottwilliams/Dev/marrow-rust-hardening-l13-backup-restore` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-backup-restore`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-soundness` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-review-idiom`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l13-main-integration` | `2215296a4de471bf051e15990158e558b9d51bd6` | lane `fdbc324e025b5cd81b7bd97354544552c8e02bb5`; main `b1f0112ed36908535c0d4ef1dc09f198835134c1` | complete | focused backup tests, workspace build/test, workspace clippy, and fmt gates passed | fail on typed wrong-type manifest payload, then pass after fix | pass, then pass after re-review | L13-R001 fixed and re-reviewed | integrated on main after live-main recheck; tracker evidence recorded |
| L14 cli-tools-server | `/Users/scottwilliams/Dev/marrow-rust-hardening-l14-cli-tools-server` | lane `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-cli-tools-server`; review `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-soundness-4` and `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-review-idiom-4`; main `/Users/scottwilliams/Dev/.build/marrow-targets/rust-hardening-l14-main-integration` | `13852686ed8317e1b567f941ff160de345738d3b` | lane `86ff6e4f4647e5f88f2ea8abe9a0598961fb5c94` and `79adda94db4aae1e733f1d17a5582d971cce1293`; main `14f71ea` and `1b22287` | in-lane | focused CLI tests, package test, workspace build/test, workspace clippy, and fmt gates passed | pass, no findings after review-fix | fail on semantic text assertions and duplicate JSON parsing, then pass after fix | L14-R001 and L14-R002 fixed and re-reviewed | CLI diagnostic test-support slice integrated; untouched L14 source and sibling CLI files remain unreviewed |

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
- `crates/marrow-check/src/catalog.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/src/checks.rs` - status: in-lane; owner: L06 checker-core; notes: schema diagnostic, duplicate-root, duplicate-declaration, duplicate-module, module-path, and rejected-surface payload suppression matches integrated; broader checker-core review remains.
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
- `crates/marrow-check/src/lib.rs` - status: in-lane; owner: L06 checker-core; notes: schema diagnostic, duplicate-root, duplicate-declaration, duplicate-module, module-path, and rejected-surface payload variants integrated; broader checker-core review remains.

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
- `crates/marrow-check/tests/catalog_presence.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/checked_program.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/durable_path.rs` - status: unreviewed; owner: L06 checker-core; notes: initial inventory.
- `crates/marrow-check/tests/evolution_discharge.rs` - status: unreviewed; owner: L07 checker-evolution; notes: initial inventory.
- `crates/marrow-check/tests/presence_architecture.rs` - status: unreviewed; owner: L08 checker-presence; notes: initial inventory.
- `crates/marrow-check/tests/project.rs` - status: in-lane; owner: L06 checker-core; notes: early schema-family, duplicate-root, duplicate-declaration, duplicate-module, module-path, and rejected-surface assertions now use exact diagnostic payloads for the integrated slices; broader checker-core test cleanup remains.
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
- `crates/marrow/src/cmd_fmt.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/src/cmd_restore.rs` - status: complete; owner: L13 backup-restore; notes: reviewed-clean CLI render boundary; no source change.
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
- `crates/marrow/tests/backup_cli.rs` - status: complete; owner: L13 backup-restore; notes: semantic restore error-code assertions moved to JSON `code`; remaining `contains` checks are render/effect assertions.
- `crates/marrow/tests/check_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: semantic check diagnostics moved to JSON/JSONL code and span assertions where structured output exists; remaining text checks are render or usage boundaries.
- `crates/marrow/tests/check_project_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: project diagnostics and summaries assert JSONL codes, paths, and status instead of prose fragments.
- `crates/marrow/tests/data_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: data integrity semantic problem assertions use JSON problem records, stable codes, source paths, tooling kind, and serialized JSON leakage checks.
- `crates/marrow/tests/dry_run_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/evolve_cli.rs` - status: complete; owner: L14 cli-tools-server; notes: semantic evolution diagnostics assert JSON codes, catalog IDs, populated counts, repair-required, approval-required, and schema-drift facts.
- `crates/marrow/tests/fmt_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/lsp_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/run_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/serve_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/support/mod.rs` - status: complete; owner: L14 cli-tools-server; notes: shared CLI support provides small JSON/JSONL helpers and production catalog commit fixture helper; function-scoped dead-code allowances are integration-test-crate local.
- `crates/marrow/tests/test_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/tooling_architecture.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/trace_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/usage_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.
- `crates/marrow/tests/v01_cli.rs` - status: unreviewed; owner: L14 cli-tools-server; notes: initial inventory.

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
  - ``rg -n 'check\.rejected_surface|RejectedSurface|payload: DiagnosticPayload::None|saved `inout`|take|window|after|from|until|resume|reverse|message\.contains' crates/marrow-check/src/rejected_surface.rs crates/marrow-check/tests/project.rs crates/marrow-check/src/lib.rs crates/marrow-check/src/checks.rs`` found the new payload sites and remaining unrelated checker prose assertions, with no rendered-message assertions in the migrated rejected-surface family and no `DiagnosticPayload::None` in the rejected-surface emitter.
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
