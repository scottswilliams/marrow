# check (marrow-check)

The semantic core of the toolchain. It consumes the parsed AST (`marrow-syntax`)
and compiled schemas (`marrow-schema`) and produces a `CheckedProgram`: the
single structured view of a checked project that the runtime, evolution,
catalog, and editor tooling all read against. Everything downstream resolves
names and types against this artifact, never against source spelling.

## Pipeline order

`analysis::analyze_source_project` sequences the whole pipeline; each step mutates or extends the program in place:

1. **discover + parse** — overlay-or-disk read of source roots and tests into parse trees; error files are retained so editors keep working on broken buffers.
2. **normalize named signatures and keyed layers** — `normalize_program_named_types` re-resolves every named signature slot (params/returns/constants) against the whole program, *before any pass reads parameter types*, so cross-module enum/resource identity compares like-for-like at calls. `normalize_resource_layers` then rewrites explicit keyed fields whose value type names a resource into keyed resource layers before facts or saved-place checks read the tree shape.
3. **resolve + type-check** — `check_resolved_files` runs import resolution and the statement/expression type pass together: it resolves every reference to a `Def`, walks each body inferring `MarrowType` (best-effort, total; unresolvable becomes `Unknown` and defers every rule), and emits typed `CheckDiagnostic`s.
4. **facts, twice** — `rebuild_facts_with_sources` assembles the read-only `CheckedFacts` tables before resolution so name/type passes can query durable ids and schema paths, then rebuilds them after `check_resolved_files` so direct effects, entry footprints, and typed places reflect the resolved bodies.
5. **evolve intents** — `collect_evolve_intents` extracts rename/retire/default/transform declarations from the parsed sources before catalog binding, so identity reconciliation sees the requested lifecycle moves.
6. **catalog bind** — `bind_catalog` reconciles every durable declaration against the persisted accepted catalog, carrying stable ids forward across renames and proposing an advanced catalog.
7. **evolve-intent type check** — `check_evolve_types` types every `evolve` block's `default` and `transform` steps against current source.
8. **lower and transform effects** — `lower_runtime_bodies` turns checked bodies into the syntax-free `Checked*` IR the runtime evaluates, filling each function's and transform's `runtime_body`; `check_transform_effects` then proves transform bodies stay inside their allowed pure read surface.
9. **presence** — `check_presence`, a flow-sensitive pass over the lowered IR, proves every read of maybe-present saved data is justified before runtime.

Evolution discharge and the analysis/tooling surface sit beside this spine, consuming the finished `CheckedProgram` read-only.

## The artifact

`CheckedProgram` (`Vec<CheckedModule>` + `CheckedFacts` + `ProgramCatalog`) is the artifact; `CheckedRuntimeProgram` is its syntax-free execution view with lowered bodies and resolved entries. [types.md](types.md) owns its full shape and the positional function-to-declaration alignment rule.

## Sub-areas

| Page | Owns |
| --- | --- |
| [types.md](types.md) | Name resolution, the `MarrowType` lattice and its rules, the type-check driver, typed fact tables, enum `match`/`is`, durable-path classification, and the `CheckedProgram` artifact. |
| [presence.md](presence.md) | Flow-sensitive presence proofs over the lowered IR (`check_presence`) and body-local direct-effect summaries (`direct_effects_for_block`). |
| [catalog.md](catalog.md) | Stable opaque identity: the `marrow-catalog` accepted-snapshot model, catalog binding across renames/reshapes, the source-shape digest fence, and the rejected v0.1 source-surface pass. |
| [lowering.md](lowering.md) | The one-way bridge from checker resolution into the `Checked*` executable IR: call targets, precomputed saved places, runtime value types. |
| [evolution.md](evolution.md) | The check side of schema evolution: evolve intents, read-only discharge against the live store, and the `EvolutionWitness` that crosses into apply. |
| [analysis.md](analysis.md) | The transport-free editor/CLI surface: the IDE `AnalysisSnapshot` and cursor queries, plus typed saved-data tooling facts (path queries, paged traversal, integrity, metadata). |

## Read next

- `crates/marrow-check/src/lib.rs` — the crate root: module declarations and the public re-export surface, nothing else.
- `crates/marrow-check/src/driver.rs` — `check_project` and `check_tests*`, the per-file structural check, and the name/path/builtin resolution helpers shared with the type passes.
- `crates/marrow-check/src/diagnostics.rs` — the diagnostic vocabulary: the `check.*` codes, the typed `DiagnosticPayload`, and `CheckDiagnostic` / `CheckReport`.
- `crates/marrow-check/src/analysis.rs` — `analyze_source_project`, the pipeline orchestrator.
- `crates/marrow-check/src/program.rs` — `CheckedProgram`, `MarrowType`, `lower_runtime_bodies`.
- `crates/marrow-check/src/resolve.rs` — `resolve`, the single name resolver every consumer routes through.
