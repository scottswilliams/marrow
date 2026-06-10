# check (marrow-check)

The semantic core of the toolchain. It consumes the parsed AST (`marrow-syntax`) and compiled schemas (`marrow-schema`) and produces a `CheckedProgram`: the single structured view of a checked project that the runtime, evolution, catalog, and LSP all read against. Everything downstream resolves names and types against this artifact, never against source spelling.

## Phase order

`analysis::analyze_source_project` sequences the whole pipeline; each phase mutates or extends the program in place:

1. **discover + parse** — overlay-or-disk read of source roots and tests into parse trees; error files are retained so editors keep working on broken buffers.
2. **normalize named signatures** — `normalize_program_named_types` re-resolves every named signature slot (params/returns/constants) against the whole program, *before any pass reads parameter types*, so cross-module enum/resource identity compares like-for-like at calls.
3. **resolve + type-check** — `check_resolved_files` runs import resolution and the statement/expression type pass together: it resolves every reference to a `Def`, walks each body inferring `MarrowType` (best-effort, total; unresolvable becomes `Unknown` and defers every rule), and emits typed `CheckDiagnostic`s.
4. **facts** — `rebuild_facts_with_sources` assembles the read-only `CheckedFacts` tables: newtyped ids over modules/functions/resources/stores/indexes/members/enums, durable-key decoding, and direct-effect summaries.
5. **catalog bind** — `bind_catalog` reconciles every durable declaration against the persisted accepted catalog, carrying stable ids forward across renames and proposing an advanced catalog.
6. **lower** — `lower_runtime_bodies` turns checked bodies into the syntax-free `Checked*` IR the runtime evaluates, filling each function's and transform's `runtime_body`.
7. **presence** — `check_presence`, a flow-sensitive pass over the lowered IR, proves every read of maybe-present saved data is justified before runtime (after the evolution transform-effects check).

Evolution discharge and the analysis/tooling surface sit beside this spine, consuming the finished `CheckedProgram` read-only.

## The artifact

`CheckedProgram` (`Vec<CheckedModule>` + `CheckedFacts` + `ProgramCatalog`) is the artifact; `CheckedRuntimeProgram` is its syntax-free execution view with lowered bodies and resolved entries. [types.md](types.md) owns its full shape and the positional function-to-declaration alignment rule.

## Sub-areas

| Page | Owns |
| --- | --- |
| [types.md](types.md) | Name resolution, the `MarrowType` lattice and its rules, the type-check driver, typed fact tables, enum `match`/`is`, durable-path classification, and the `CheckedProgram` artifact. |
| [presence.md](presence.md) | Flow-sensitive presence proofs over the lowered IR (`check_presence`) and body-local direct-effect summaries (`direct_effects_for_block`). |
| [catalog.md](catalog.md) | Stable opaque identity: catalog binding across renames/reshapes, the source-shape digest fence, and the rejected v0.1 source-surface pass. |
| [lowering.md](lowering.md) | The one-way bridge from checker resolution into the `Checked*` executable IR: call targets, precomputed saved places, runtime value types. |
| [evolution.md](evolution.md) | The check side of schema evolution: evolve intents, read-only discharge against the live store, and the `EvolutionWitness` that crosses into apply. |
| [analysis.md](analysis.md) | The transport-free editor/CLI surface: the IDE `AnalysisSnapshot` and cursor queries, plus typed saved-data tooling facts (path queries, paged traversal, integrity, metadata). |

## Read next

- `crates/marrow-check/src/lib.rs` — the crate root: module declarations and the public re-export surface, nothing else.
- `crates/marrow-check/src/driver.rs` — `check_project` and `check_tests*`, the per-file structural check, and the name/path/builtin resolution helpers shared with the type passes.
- `crates/marrow-check/src/diagnostics.rs` — the diagnostic vocabulary: the `check.*` codes, the typed `DiagnosticPayload`, and `CheckDiagnostic` / `CheckReport`.
- `crates/marrow-check/src/analysis.rs` — `analyze_source_project`, the phase orchestrator.
- `crates/marrow-check/src/program.rs` — `CheckedProgram`, `MarrowType`, `lower_runtime_bodies`.
- `crates/marrow-check/src/resolve.rs` — `resolve`, the single name resolver every consumer routes through.
