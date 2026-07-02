# Testing Architecture

Tests exercise the production pipeline, never hand-built replicas of it. Each
suite names the layer it tests and uses that layer's typed oracle; diagnostic
prose is render-only and is never a semantic oracle.

## Layers

| Layer | Harness | Oracle |
|---|---|---|
| Conformance corpus | `.mw` fixture projects run through native `marrow test` | typed JSONL test report (name, outcome, fault code, span) |
| Crate suites | shared `tests/support` modules over `check_project` / `checked_program` / `ProjectSession` | typed `Checked*` facts, diagnostics (`code`, `reason`, `span`), runtime values, store effects |
| CLI boundary checks | `tests/support::marrow*` process invocation | exit codes, typed JSON/JSONL envelopes (`support::diagnostic_records`, `codes`) |
| Render contracts | golden files compared byte-for-byte | checked-in golden; an update is a review of the render contract change |
| Tidy gates | source scans in `crates/marrow/tests/cases/` | checked-in baselines that only shrink |

## Conformance corpus

Language and runtime behavior belongs in source-driven fixture projects under
`fixtures/v01/conformance/<name>/`, each a complete project (`marrow.json`
with a `tests` root, sources, `.mw` test functions asserting through
`std::assert`). `marrow test` runs every test over a fresh in-memory store, so
fixtures freely exercise saved reads and writes and leave the repository
untouched.

The driver `crates/marrow/tests/cases/conformance_corpus.rs` discovers every
corpus project, runs `marrow test --format jsonl` on it in place, and asserts
the typed report: at least one test, zero failed, zero errored. Discovery must
match the driver's checked-in `EXPECTED_FIXTURES` roster exactly, so a deleted
fixture directory cannot shrink coverage silently. Each fixture's
test module opens with a header comment naming its contract, which the driver
enforces:

```
; Layer: runtime (saved enum writes and match dispatch)
; Oracle: std::assert over entry return values
; Replaces: crates/marrow/tests/cases/run_cli_enum.rs behavior tests
```

`Layer:` names what the fixture exercises, `Oracle:` how it asserts, and
`Replaces:` the Rust test it migrated from (`none` for new coverage).

A CLI test that only proves ".mw behavior holds end to end" is a conformance
fixture, not a Rust test. Rust CLI suites keep only what needs the process
boundary: exit codes, envelope shapes, multi-invocation store lifecycle.

## Render contracts

Rendered prose (diagnostic messages, remedy guidance) is pinned by golden
files under `crates/marrow/tests/goldens/`, compared exactly via
`support::assert_matches_golden`. Semantic assertions stay on typed codes and
payload fields next to the golden. Editing a golden is editing a render
contract and gets reviewed as one. One golden per contract — a remedy phrasing
shared by many sites is pinned once, not re-asserted per test.

## Prose-assertion ratchet

`crates/marrow/tests/cases/tidy_prose_assertions.rs` scans every `.rs` file
under `crates/` for `message.contains(` and compares per-file counts against
`crates/marrow/tests/prose_assertion_baseline.txt`. Any drift fails the gate:
a higher count is a new prose assertion (assert the typed code, payload, span,
or a golden instead); a lower count means the baseline line must shrink in the
same change. The baseline only shrinks and ends at zero.

The gate catches only that literal spelling. Aliased bindings, `.find(`,
`starts_with`, regexes over message text, and `stderr`/`stdout`
`.contains(prose)` are equivalent prose assertions it does not see; reviewers
block them the same way, and a ratchet-down that merely respells the prose
match is not burn-down.

## Migration path

- `crates/marrow` CLI suites: behavior tests move to conformance fixtures;
  remaining boundary checks assert typed envelopes; render contracts move to
  goldens.
- `crates/marrow-check` suites: same treatment, scheduled after the checker
  hotspot lanes; prose assertions on checker diagnostics become `code`/payload
  asserts as typed payloads land.
- `crates/marrow-json` inline tests: the surface operation mass migrates to
  fixture-driven integration suites through this corpus when the wire layer
  moves to its own crate; until then new tests there follow the typed-oracle
  rules and the ratchet.
- `crates/marrow-run` / `marrow-syntax` inline prose asserts: burn down via
  the ratchet as their suites are touched; parser tests assert
  `DiagnosticReason` variants, not message text.
