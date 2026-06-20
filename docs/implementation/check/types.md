# Type-Checking Spine

The semantic core of `marrow-check`. It consumes the parsed AST
(`marrow-syntax`) and compiled schemas (`marrow-schema`) and produces the typed
diagnostics plus the `CheckedProgram` / `CheckedRuntimeProgram` artifacts every
downstream crate (runtime, evolution, catalog, editor tooling) reads. One
resolver, one best-effort type lattice, one statement/expression check driver,
one set of read-only fact tables.

The orchestration that sequences the passes lives in `analysis.rs`, outside this spine — the full pass sequence is owned by [check/README.md](README.md) — and calls in through `normalize_program_named_types`, `check_resolved_files`, fact rebuild, `bind_catalog`, and `lower_runtime_bodies`.

## The shape

`MarrowType` (`program.rs`) is the lattice every rule runs on. Inference is best-effort and total: anything unresolvable becomes `Unknown`, which *defers* every type rule, so a check never false-positives on an uncertain operand. `Invalid` marks an already-diagnosed expression; `Error` is a concrete checker-only type handled as a real mismatch. Nominal types compare by identity — resources by module-qualified name, identities by store root, enums by `{module, name}` — never by spelling.

`CheckedProgram` (`program.rs`) is the artifact: a `Vec<CheckedModule>` aligned 1:1 with files, plus `CheckedFacts` and `ProgramCatalog`. Checked functions carry both their return type and the typed `marrow_schema::ReturnPresence` marker that tells downstream passes whether a value-returning call can be absent. Checked functions, facts, and parse declarations align *positionally* in source order (a by-name lookup would mis-attribute a duplicate-named function's body); `lower_runtime_bodies` guards this with a `debug_assert_eq`.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/lib.rs` | Crate root: module declarations and the public re-export surface, nothing else. |
| `crates/marrow-check/src/driver.rs` | `check_project`/`check_tests*` entrypoints, the editor source overlay, per-file structural checks for source and surface namespaces, and the name/path/builtin resolution helpers shared with the type passes. |
| `crates/marrow-check/src/diagnostics.rs` | The diagnostic vocabulary: `check.*` codes, the typed `DiagnosticPayload`, `CheckDiagnostic`/`CheckReport`, and the `ConversionTarget` table. |
| `crates/marrow-check/src/program.rs` | The `CheckedProgram`/`CheckedRuntimeProgram` artifacts, the `MarrowType` lattice and `from_resolved` placement, `FileId`, runtime-body lowering. |
| `crates/marrow-check/src/resolve.rs` | The one module/visibility-aware name resolver: `resolve` → `Resolution`/`Def`/`DefItem`; `resolve_store_by_root` for project-wide saved roots. |
| `crates/marrow-check/src/checks/` | Type-check driver modules by concern; `checks/mod.rs` gathers the per-concern checks into the crate-internal check API. |
| `crates/marrow-check/src/infer.rs` | Expression type inference (`infer_type`/`infer_only`): literals, scope lookup, saved-path/leaf/group/index resolution (`saved_call_type`), enum member-path typing. |
| `crates/marrow-check/src/typerules.rs` | Pure lattice rules: `type_compatible`, `expects_conversion`, `as_primitive`, numeric/ordered/steppable predicates, literal-range envelope, mismatch display. |
| `crates/marrow-check/src/rules.rs` | Structural (syntax-only) rules: loop control-flow, loop saved-write cost warnings, catch-type, assignment-target validity, immutable-binding reassignment (params, `const`, loop variables, `if const`), same-block redeclaration, key-aware loop-mutation of the traversed layer, and const-constant-expr. |
| `crates/marrow-check/src/facts.rs` | Read-only typed fact tables (`CheckedFacts`) with newtyped ids, `CheckedType`, `StoredValueMeaning` (durable-key decoding), presence proofs, effect summaries, catalog binding. |
| `crates/marrow-check/src/backing_validity.rs` | Source-time backing invalidations resolved once into typed fact-id sets before surface fact emission. |
| `crates/marrow-check/src/entry_abi.rs` | Checker-owned entry invocation descriptors: resolves public entries to `entry.invoke.v1` identities over parameter shapes, return shape, accepted catalog identities, and return presence; runtime consumes these descriptors instead of reconstructing ABI ownership. |
| `crates/marrow-check/src/surface.rs` | Application-surface checker pass: resolves `surface` declarations to checker-valid store, top-level field, index, and public action facts after catalog binding; records transport-neutral `SurfaceFact`s with derived `SurfaceReadOperationFact`s for node reads, collection pages, unique-index lookups, backing-record footprint, projection, render alias, create fields, sparse update fields, delete declarations, actions, and source-only/stable catalog status; emits `check.surface_target`, `check.surface_field`, and `check.surface_action`. |
| `crates/marrow-check/src/surface_abi.rs` | Checker-owned surface ABI descriptors and digest framing: renders accepted-catalog read, create, sparse-update, delete, and action operation descriptors for stable surfaces; computes the `surface.read.v1` / `surface.create.v1` / `surface.update.v1` / `surface.delete.v1` operation tags consumed by runtime and JSON boundaries; carries read-operation aliases as render metadata for later route/client profiles; and reuses `entry.invoke.v1` identity over parameters and return shape for actions. Shared value-shape classification lives here so read, create, and update descriptors do not drift. |
| `crates/marrow-check/src/enums.rs` | Enum resolution and `match`/`is` checking: cross-module signature normalization, `resolve_enum_member_path`, exhaustiveness/duplicate-arm, `resolve_type`. |
| `crates/marrow-check/src/keyed_entries.rs` | Project-aware keyed resource-layer normalization, plus named enum field validation and diagnostics that schema compilation cannot decide alone. |
| `crates/marrow-check/src/binding.rs` | The editor binding index: definition→reference map with scope/shadowing/alias awareness, reusing resolve/infer; `RenameSafety` (SourceOnly vs SavedDataBacked). |
| `crates/marrow-check/src/durable_path.rs` | Classification of decoded `^root(key).field` store-path text: `parse_path`/`display_path`, `SavedKey` parsing, `StoreLeafKind`, `identity_leaf_key_mismatch`. |
| `crates/marrow-check/src/walk.rs` | Single owner of immediate-child enumeration of an `Expression` (`for_each_child_expr`), so read-only passes recurse without re-spelling tree shape. |

## Invariants worth knowing

- **One resolver.** `resolve` is the only module/visibility-aware resolver;
  checker, runtime, and binding tooling all route through it. A bare name
  resolves in its own module first regardless of `pub`; `use` imports module
  names, not the names inside them. Saved roots are project-wide; source names
  are module-scoped — the two namespaces never collapse.
- **Strict typing across conversion boundaries.** An `Unknown` value flowing into a concrete typed place with a conversion boundary (`expects_conversion`) is `check.untyped_value`, not silent acceptance.
- **Catalog identity is committed as a file artifact.** Production durable identity is the fixed `marrow.catalog.json` artifact. The store keeps a private copy as the crash bridge and transaction participant: state-establishing commands commit catalog rows with the data they describe, then render the file from that committed snapshot. Ordinary check reads only the file artifact and never opens the store to repair, create, or rewind catalog renders.

## Code-reality notes

- `resolve.rs` `is_public` treats every `Resource` as cross-module visible and
  `pub`-gates functions. Enum visibility is separate, owned by
  `crates/marrow-check/src/enums.rs`. A non-`pub` resource remains reachable by
  qualified path.
- `durable_path.rs` is a self-contained, publicly exported classifier whose consumers are tooling/runtime outside the crate; it couples to the spine only via `resolve_store_by_root` and `EnumId`. Read it as an adjacent utility, not part of the inference core.

## Read next

- `program.rs` — `CheckedProgram`, `MarrowType`, `MarrowType::from_resolved`, `lower_runtime_bodies` (the artifact and lattice every downstream crate reads against).
- `checks/calls.rs` and `checks/statements.rs` — `check_call`, `StatementCheck::check` (the dispatch heart; how every type diagnostic is produced and how scope threads through blocks).
- `infer.rs` — `infer_type`, `saved_call_type` (layered `^root(key).layer(key).field` typing — the trickiest part of the lattice).
- `facts.rs` — `CheckedFacts::from_modules`, `StoredValueMeaning::stored_key` (id-assignment order and the single owner of durable member-byte → `SavedKey` decoding).
- `resolve.rs` — `resolve`, `resolve_store_by_root` (the resolution outcome shared by checker, runtime, and binding tooling).
