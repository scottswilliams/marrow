# Type-Checking Spine

The semantic core of `marrow-check`. It consumes the parsed AST
(`marrow-syntax`) and compiled schemas (`marrow-schema`) and produces the typed
diagnostics plus the `CheckedProgram` / `CheckedRuntimeProgram` artifacts every
downstream crate (runtime, evolution, catalog, editor tooling) reads. One
resolver, one best-effort type lattice, one statement/expression check driver,
one set of read-only fact tables.

The orchestration that sequences the passes lives in `analysis.rs`, outside this spine — the full pass sequence is owned by [check/README.md](README.md) — and calls in through `normalize_program_named_types`, `check_resolved_files`, fact rebuild, `bind_catalog`, and `lower_runtime_bodies`.

## The shape

`MarrowType` (`program.rs`) is the lattice every rule runs on. Inference is best-effort and total: anything unresolvable becomes `Unknown`, which *defers* every type rule, so a check never false-positives on an uncertain operand. `Invalid` marks an already-diagnosed expression; `Error` is a concrete checker-only type handled as a real mismatch. `Optional(T)` (built through the flattening `MarrowType::optional`, so it never nests) and the empty-optional `Absent` carry presence in the type: the one rule rejects an optional where a non-optional is required (`CHECK_UNRESOLVED_OPTIONAL`), matched ahead of `Unknown` deferral in `type_compatible` and the concreteness gates so a degraded-to-`Unknown` slot cannot silently admit it. Nominal types compare by identity — resources by module-qualified name, identities by store root, enums by `{module, name}` — never by spelling.

`CheckedProgram` (`program.rs`) is the artifact: a `Vec<CheckedModule>` aligned 1:1 with files, plus `CheckedFacts` and `ProgramCatalog`. Checked functions carry their return type only; optionality lives in that type (a maybe-present function returns `T?`, a `MarrowType::Optional`), so `returns_maybe_present()` is read off the type rather than a parallel presence marker. Checked functions, facts, and parse declarations align *positionally* in source order (a by-name lookup would mis-attribute a duplicate-named function's body); `lower_runtime_bodies` guards this with a `debug_assert_eq`.

Compound assignment is checked as one statement, not parser desugaring. The
statement checker validates the target with the same assignable-place rules as
plain assignment, infers the target again as a read operand, checks the matching
binary operator (`+=` uses `+`, `*=` uses `*`, and so on), then applies ordinary
assignment compatibility from the computed type back into the target type.
Lowering preserves the statement as `CheckedStmt::CompoundAssign` with a typed
`CheckedBinaryOp` so runtime can resolve the mutation target once.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/lib.rs` | Crate root: module declarations and the public re-export surface, nothing else. |
| `crates/marrow-check/src/driver.rs` | `check_project`/`check_tests*` entrypoints, the editor source overlay, per-file structural checks for source and surface namespaces, and the name/path/builtin resolution helpers shared with the type passes. |
| `crates/marrow-check/src/diagnostics.rs` | The diagnostic vocabulary: `check.*` codes, the typed `DiagnosticPayload`, `CheckDiagnostic`/`CheckReport`, and the `ConversionTarget` table. |
| `crates/marrow-check/src/program.rs` | The `CheckedProgram`/`CheckedRuntimeProgram` artifacts, the `MarrowType` lattice and `from_resolved` placement, `FileId`, runtime-body lowering. |
| `crates/marrow-check/src/resolve.rs` | The one module/visibility-aware name resolver: `resolve` → `Resolution`/`Def`/`DefItem`; `resolve_store_by_root` for project-wide saved roots. |
| `crates/marrow-check/src/checks/` | Type-check driver modules by concern; `checks/mod.rs` gathers the per-concern checks into the crate-internal check API. |
| `crates/marrow-check/src/infer.rs` | Expression type inference (`infer_type`/`infer_only`): literals, scope lookup, saved-path/leaf/group/index resolution (`saved_call_type`), enum member-path typing. An undeclared field is `check.unknown_field` both when read and when it is the terminal of a write target; a navigated base of a write target stays silent so the dedicated assignment-target rules own its errors. |
| `crates/marrow-check/src/typerules.rs` | Pure lattice rules: `type_compatible`, `expects_conversion`, `as_primitive`, numeric/ordered/steppable predicates, literal-range envelope, mismatch display. |
| `crates/marrow-check/src/rules.rs` | Structural (syntax-only) rules: loop control-flow, loop saved-write cost warnings, catch-type, assignment-target validity, immutable-binding reassignment (params, module and local `const`, loop variables, `if const`), same-block redeclaration, key-aware loop-mutation of the traversed layer, and const-constant-expr. |
| `crates/marrow-check/src/facts.rs` | Read-only typed fact tables (`CheckedFacts`) with newtyped ids, `CheckedType`, `StoredValueMeaning` (durable-key decoding), presence proofs, effect summaries, catalog binding. |
| `crates/marrow-check/src/backing_validity.rs` | Source-time backing invalidations resolved once into typed fact-id sets before surface fact emission. |
| `crates/marrow-check/src/entry_abi.rs` | Checker-owned callable ABI descriptors: resolves public entries to `entry.invoke.v1` identities over the single `EntryParameterShape` parameter carrier (`Present`/`Optional`, presence read off the parameter type and folded into the tag), the single `EntryResultShape` result carrier (`Void`/`Present`/`Optional`, presence read off the return type), and accepted catalog identities; owns the shared scalar, enum, identity, sequence, and resource result shape builder consumed by action and computed-read surface descriptors. |
| `crates/marrow-check/src/surface.rs` | Application-surface checker pass: resolves `surface` declarations to checker-valid store, top-level field, index, public action facts, and public read-only computed-read facts after catalog binding; records transport-neutral `SurfaceFact`s with derived `SurfaceReadOperationFact`s for node reads, collection pages, unique-index lookups, backing-record footprint, projection, render alias, create fields, sparse update fields, delete declarations, actions, computed reads, and source-only/stable catalog status; emits `check.surface_target`, `check.surface_field`, `check.surface_action`, and `check.surface_computed_read`. |
| `crates/marrow-check/src/surface_abi.rs` | Checker-owned surface ABI descriptors and digest framing: renders accepted-catalog read, computed-read, create, sparse-update, delete, and action operation descriptors for stable surfaces; computes the `surface.read.v1` / `surface.computed_read.v1` / `surface.create.v1` / `surface.update.v1` / `surface.delete.v1` operation tags consumed by runtime and JSON boundaries; carries operation aliases as render metadata for route/client profiles; reuses `entry_abi` callable descriptors for actions and computed reads; and owns only store-operation value shapes for read/create/update descriptors. |
| `crates/marrow-check/src/enums.rs` | Enum resolution and `match`/`is` checking: cross-module signature normalization, `resolve_enum_member_path`, exhaustiveness/duplicate-arm, `resolve_type`. |
| `crates/marrow-check/src/keyed_entries.rs` | Project-aware keyed resource-layer normalization, plus named enum field validation and diagnostics that schema compilation cannot decide alone. |
| `crates/marrow-check/src/binding.rs` | The editor binding index: definition→reference map with scope/shadowing/alias awareness, reusing resolve/infer; `RenameSafety` (SourceOnly vs SavedDataBacked). |
| `crates/marrow-check/src/durable_path.rs` | Classification of decoded `^root(key).field` store-path text: `parse_path`/`display_path`, `SavedKey` parsing, `StoreLeafKind`, `identity_leaf_key_mismatch`. |
| `crates/marrow-check/src/data_text.rs` | The `data` tools' text-format string codec, paired so the dump/get renderer and the saved-path key parser are inverses: the five `.mw` escapes plus `\xNN` for every other control byte, a total round-trippable vocabulary broader than a `.mw` string literal. |
| `crates/marrow-check/src/walk.rs` | Single owner of immediate-child enumeration of an `Expression` (`for_each_child_expr`) and the whole-tree `^root` visitor (`for_each_saved_root`), so read-only passes recurse without re-spelling tree shape. |

## Invariants worth knowing

- **One resolver.** `resolve` is the only module/visibility-aware resolver;
  checker, runtime, and binding tooling all route through it. A bare name
  resolves in its own module first regardless of `pub`; `use` imports module
  names, not the names inside them. Saved roots are project-wide; source names
  are module-scoped — the two namespaces never collapse.
- **Strict typing across conversion boundaries.** An `Unknown` value flowing into a concrete typed place with a conversion boundary (`expects_conversion`) is `check.untyped_value`, not silent acceptance.
- **The live store owns accepted identity; `marrow.lock` is its projection.** Production saved-data identity is the live store catalog family, written in the same transaction as the data it describes — the sole write-time authority. The committed `marrow.lock` is a one-way projection of that snapshot: state-establishing commands regenerate it after the store commit, and it seeds a fresh empty store for first-run adoption. Ordinary check binds the store snapshot when present and the committed lock otherwise; it never opens the store to repair or create it, and the lock can never override or rewrite the store.

## Code-reality notes

- `resolve.rs` `is_public` treats every `Resource` as cross-module visible and
  `pub`-gates functions. Enum visibility is separate, owned by
  `crates/marrow-check/src/enums.rs`. A non-`pub` resource remains reachable by
  qualified path.
- `durable_path.rs` is a self-contained, publicly exported classifier whose consumers are tooling/runtime outside the crate; it couples to the spine only via `resolve_store_by_root` and `EnumId`. Read it as an adjacent utility, not part of the inference core.

## Read next

- `program.rs` — `CheckedProgram`, `MarrowType`, `MarrowType::from_resolved`, `lower_runtime_bodies` (the artifact and lattice every downstream crate reads against).
- `checks/calls.rs` and `checks/statements.rs` — `check_call`, `StatementCheck::check` (the dispatch heart; how every type diagnostic is produced and how scope threads through blocks).
- `infer.rs` — `infer_type`, `saved_call_type` (layered `^root(key).layer(key).field` typing — the trickiest part of the lattice). A partially keyed composite layer names an iterable inner sub-layer, never a scalar: `ValuePosition` gates this, so a `.field`/child-layer descent or a bare value read (scalar bind, interpolation, plain call argument, return) of one is a `check.layer_not_value` error, while a non-value position (a `for` iterable, a `keys`/`values`/`entries`/`count` argument, or a write/delete target) skips the gate so its dedicated owner — streaming for the former, the invalid-target check for the latter — is the single root cause. A saved collection (a store root, saved keyed sub-layer, index branch, or one wrapped in a traversal combinator) is a stream with no materialized value, recognized place-based by `checks::calls::materializes_saved_collection_by_value`: it may only be iterated or counted, so every value position rejects it at check — a binary operator or `??` operand and a `print`/interpolation render in `infer.rs` (`check.operator_type`), and a `const`/`var` bind, local assignment, by-value argument, or declared collection return in `checks/statements.rs` and `checks/calls.rs`. Local `sequence[T]` renderability is type-based and recursive: it renders only when `T` renders, without making saved collection paths materialized values.
- `facts.rs` — `CheckedFacts::from_modules`, `StoredValueMeaning::stored_key` (id-assignment order and the single owner of durable member-byte → `SavedKey` decoding).
- `resolve.rs` — `resolve`, `resolve_store_by_root` (the resolution outcome shared by checker, runtime, and binding tooling).
