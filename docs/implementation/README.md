# Marrow Implementation Map

Marrow is a typed language whose durable saved data is part of the program: the
same compiler that checks `.mw` source also governs what is already in the
store, so a schema change is checked against existing data before it can
activate.

This map says what each part does and points at the real code. Read this page
first; every subsystem page hangs off it. For language semantics see
[`docs/language/`](../language/); for the store byte contract see
[`docs/backend-contract.md`](../backend-contract.md). Those are law; this map
describes the code that implements them.

## The kernel

Nine crates stacked in dependency order, lowest first:

- **catalog** — the accepted-catalog snapshot model: the committed record of
  durable identity (epoch, digest, entries), its validation invariants, and
  structural-signature decode. Both store and check read it.
- **store** — the byte-level tree-cell contract under the language: order-preserving
  keys, canonical values, two ordered-byte engines. Knows no `.mw`.
- **syntax** — the front end: text to AST plus typed diagnostics, and back to
  canonical source. Decides shape only, no meaning.
- **schema** — first semantic pass: one declaration to a typed tree shape
  (resource/store/enum) downstream code pattern-matches instead of re-parsing.
- **check** — the semantic core: name resolution, type inference, presence/effect
  proofs, durable identity (catalog), schema-evolution discharge, and lowering
  to the executable `Checked*` form. Produces the `CheckedProgram` everything
  else consumes.
- **run** — a tree-walking interpreter over the checked program: evaluates entries,
  drives managed saved writes inside transactions, applies schema evolution, and
  owns linked-Rust project surface sessions over accepted native stores.
- **json** — JSON DTOs for entry return values, saved-key leaves, data snapshot
  stamps, surface ABI descriptors, surface reads, sparse update request bodies,
  action request/results, and operation envelopes. It preserves existing
  machine-readable CLI shapes and owns checked surface read request-parameter
  decode, project update/action execution wrappers, accepted-catalog surface
  action value rendering, and context-aware cursor-boundary rendering; routes,
  opaque cursor tokens, generated clients, and create/delete body decode are not
  owned here.
- **project / cli** — `marrow.json`, discovery, and the operator binary that
  wires the above together and renders results.

`check` is the spine. It is the one owner of resolution, types, facts, identity,
and lowering; `run`, the CLI, editor tooling, and backup/restore are all
downstream of its `CheckedProgram` / `CheckedRuntimeProgram` artifacts and never
re-derive semantics.

## The pipeline

End to end, source text to committed saved data:

```
.mw text
  │  parse_source                         (marrow-syntax)
  ▼  ParsedSource (AST + diagnostics)
  │  compile_resource / compile_store / compile_enum   (marrow-schema)
  ▼  ResourceSchema / StoreSchema / EnumSchema
  │  check_project (marrow-check):
  │     normalize named signatures + keyed layers
  │     → facts → resolve+infer → facts → evolve intents
  │     → bind_catalog → check evolve types
  │     → lower → transform effects → presence
  ▼  CheckedProgram / CheckedRuntimeProgram
  │  run_entry  (marrow-run, over a TreeStore + Host)
  ▼  evaluate entry; managed writes → WritePlan → commit
  │  TreeStore transaction (data + index cells + epoch stamp)  (marrow-store)
  ▼  committed saved data
```

Schema evolution is a side branch of the same spine: `evolution::preview`
(check side) discharges every obligation against the live store read-only and
emits an `EvolutionWitness`; `evolution::apply` (run side) re-verifies that
witness byte-for-byte and commits the data rewrite atomically. Nothing activates
over data the checker has not proven safe.

## Crate map

| Crate | Role | Page |
|---|---|---|
| `marrow-syntax` | Lexer, parser, AST, formatter, typed diagnostics | [syntax.md](syntax.md) |
| `marrow-schema` | Resource/store/enum compilation; stdlib + Error tables | [schema.md](schema.md) |
| `marrow-check` | Resolution, types, facts, catalog identity, evolution, lowering | [check/](check/README.md) |
| `marrow-store` | Tree-cell storage contract; key/value codecs; mem + redb engines | [store.md](store.md) |
| `marrow-run` | Tree-walking interpreter; saved reads/writes; evolution apply; read-only surface session | [runtime/](runtime/README.md) |
| `marrow-json` | JSON for entry returns, tooling keys, data stamps, and checked surface read/update/action DTOs | [json.md](json.md) |
| `marrow-catalog` | Accepted-catalog model: epoch/digest/entries, validation, structural-signature decode | [check/](check/README.md) |
| `marrow-project` | `marrow.json` schema, discovery, the project digest | [cli.md](cli.md) |
| `marrow` | CLI dispatch, run/test/fmt, data/backup/evolve | [cli.md](cli.md) |

`marrow-check` and `marrow-run` are large enough to split into a directory of
pages: [check/](check/README.md) covers the type core, presence/effects, catalog
identity, the executable IR, evolution discharge, and the analysis/tooling
surface; [runtime/](runtime/README.md) covers the evaluator core, saved reads,
managed writes, evolution apply, and the stdlib boundary. [cli.md](cli.md)
documents both `marrow-project` and the `marrow` CLI.

## How to navigate

- Start from the crate that owns the concept you care about, using the table.
- Cross-crate contracts are owned by the upstream crate: types and diagnostics
  in `marrow-syntax`, schema shapes in `marrow-schema`, the `CheckedProgram` and
  all identity/fact tables in `marrow-check`, the byte forms in `marrow-store`.
  If a fact is missing downstream, it is added upstream, not patched around.
- Each leaf page ends with a short Read next list: the best file plus symbol to
  open. Follow those, not this page, to reach code.

## Read next

Open the subsystem page for what you are changing; each ends in file-plus-symbol
pointers into the code.

- [check/](check/README.md) — `check_project`, `CheckedProgram` (`crates/marrow-check/src/lib.rs`, `program.rs`)
- [runtime/](runtime/README.md) — `run_entry` (`crates/marrow-run/src/entry.rs`)
- [syntax.md](syntax.md) — `parse_source` (`crates/marrow-syntax/src/lib.rs`)
- [schema.md](schema.md) — `compile_resource` (`crates/marrow-schema/src/lib.rs`)
- [store.md](store.md) — `TreeStore` (`crates/marrow-store/src/tree.rs`)
- [cli.md](cli.md) — `main` (`crates/marrow/src/main.rs`)
