# Type-Checking Spine

The semantic core of `marrow-check`. It consumes the parsed AST (`marrow-syntax`) and compiled schemas (`marrow-schema`) and produces the typed diagnostics plus the `CheckedProgram` / `CheckedRuntimeProgram` artifacts every downstream crate (runtime, evolution, catalog, LSP) reads. One resolver, one best-effort type lattice, one statement/expression check driver, one set of read-only fact tables.

The orchestration that sequences the passes lives in `analysis.rs`, outside this spine — the full pass sequence is owned by [check/README.md](README.md) — and calls in through `normalize_program_named_types`, `check_resolved_files`, fact rebuild, `bind_catalog`, and `lower_runtime_bodies`.

## The shape

`MarrowType` (`program.rs`) is the lattice every rule runs on. Inference is best-effort and total: anything unresolvable becomes `Unknown`, which *defers* every type rule, so a check never false-positives on an uncertain operand. `Invalid` marks an already-diagnosed expression; `Error` is a concrete checker-only type handled as a real mismatch. Nominal types compare by identity — resources by module-qualified name, identities by store root, enums by `{module, name}` — never by spelling.

`CheckedProgram` (`program.rs`) is the artifact: a `Vec<CheckedModule>` aligned 1:1 with files, plus `CheckedFacts` and `ProgramCatalog`. Checked functions, facts, and parse declarations align *positionally* in source order (a by-name lookup would mis-attribute a duplicate-named function's body); `lower_runtime_bodies` guards this with a `debug_assert_eq`.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/lib.rs` | Crate root: diagnostic codes/payloads, `ConversionTarget` table, per-file structural check, `check_project`/`check_tests` entrypoints, accepted-catalog writer, public re-exports. |
| `crates/marrow-check/src/program.rs` | The `CheckedProgram`/`CheckedRuntimeProgram` artifacts, the `MarrowType` lattice and `from_resolved` placement, `FileId`, runtime-body lowering. |
| `crates/marrow-check/src/resolve.rs` | The one module/visibility-aware name resolver: `resolve` → `Resolution`/`Def`/`DefItem`; `resolve_store_by_root` for project-wide saved roots. |
| `crates/marrow-check/src/checks/` | The type-check driver, split by concern: `driver` (resolved-file pass, file prelude, type-annotation checks), `statements` (`StatementCheck` dispatch, block/function scope), `calls` (`check_call`: builtin/std/constructor/user), `operators` (operator/condition/assign/return/throw checks), `ranges` (range-for step/direction rules), `collections` (for-loop frames, saved-path/index-branch key and value typing), `saved_keys` (key-argument typing), `returns` (return placement, divergence), and `diagnostics` (the shared error constructors). `mod.rs` re-exports the cross-crate API. |
| `crates/marrow-check/src/infer.rs` | Expression type inference (`infer_type`/`infer_only`): literals, scope lookup, saved-path/leaf/group/index resolution (`saved_call_type`), enum member-path typing. |
| `crates/marrow-check/src/typerules.rs` | Pure lattice rules: `type_compatible`, `expects_conversion`, `as_primitive`, numeric/ordered/steppable predicates, literal-range envelope, mismatch display. |
| `crates/marrow-check/src/rules.rs` | Structural (syntax-only) rules: try-handler presence, finally-escape, loop control-flow, catch-type, assignment-target validity, read-only inout, const-constant-expr. |
| `crates/marrow-check/src/facts.rs` | Read-only typed fact tables (`CheckedFacts`) with newtyped ids, `CheckedType`, `StoredValueMeaning` (durable-key decoding), presence proofs, effect summaries, catalog binding. |
| `crates/marrow-check/src/enums.rs` | Enum resolution and `match`/`is` checking: cross-module signature normalization, `resolve_enum_member_path`, exhaustiveness/duplicate-arm, `resolve_type`. |
| `crates/marrow-check/src/binding.rs` | The editor binding index: definition→reference map with scope/shadowing/alias awareness, reusing resolve/infer; `RenameSafety` (SourceOnly vs SavedDataBacked). |
| `crates/marrow-check/src/durable_path.rs` | Classification of decoded `^root(key).field` store-path text: `parse_path`/`display_path`, `SavedKey` parsing, `StoreLeafKind`, `identity_leaf_key_mismatch`. |
| `crates/marrow-check/src/walk.rs` | Single owner of immediate-child enumeration of an `Expression` (`for_each_child_expr`), so read-only passes recurse without re-spelling tree shape. |

## Invariants worth knowing

- **One resolver.** `resolve` is the only module/visibility-aware resolver; checker, runtime, and LSP all route through it. A bare name resolves in its own module first regardless of `pub`; `use` imports module names, not the names inside them. Saved roots are project-wide; source names are module-scoped — the two namespaces never collapse.
- **Strict typing across conversion boundaries.** An `Unknown` value flowing into a concrete typed place with a conversion boundary (`expects_conversion`) is `check.untyped_value`, not silent acceptance.
- **Catalog is the durable ABI.** `write_accepted_catalog` is the single all-or-nothing writer; `commit_pending_identity` only freezes a baseline. Later identity changes flow through evolve apply's witness, not here. Check never mutates the catalog.

## Code-reality notes

- `resolve.rs` `is_public` treats every `Resource` as cross-module visible and `pub`-gates functions (enum visibility is separate, owned by `enums.rs`). A non-`pub` resource is still reachable by qualified path — resources are not yet visibility-gated.
- `type_compatible` returns `None` (defer) for a cross-module `Identity`/`Resource` the checker placed as `Unknown` — a documented soundness gap ("permissive until the type IR is unified"), so some cross-module nominal mismatches are not caught at check time.
- `durable_path.rs` is a self-contained, publicly exported classifier whose consumers are tooling/runtime outside the crate; it couples to the spine only via `resolve_store_by_root` and `EnumId`. Read it as an adjacent utility, not part of the inference core.

## Read next

- `program.rs` — `CheckedProgram`, `MarrowType`, `MarrowType::from_resolved`, `lower_runtime_bodies` (the artifact and lattice every downstream crate reads against).
- `checks/calls.rs` and `checks/statements.rs` — `check_call`, `StatementCheck::check` (the dispatch heart; how every type diagnostic is produced and how scope threads through blocks).
- `infer.rs` — `infer_type`, `saved_call_type` (layered `^root(key).layer(key).field` typing — the trickiest part of the lattice).
- `facts.rs` — `CheckedFacts::from_modules`, `StoredValueMeaning::stored_key` (id-assignment order and the single owner of durable member-byte → `SavedKey` decoding).
- `resolve.rs` — `resolve`, `resolve_store_by_root` (the resolution outcome shared by checker, runtime, and LSP).
