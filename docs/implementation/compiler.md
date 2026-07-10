# Compiler implementation

The current compiler spans `marrow-schema`, `marrow-check`, and
`marrow-catalog`. It produces a `CheckedProgram` and a syntax-free
`CheckedRuntimeProgram`; it does not yet emit a portable program image.

## Schema shapes

`marrow-schema` compiles resource, store, enum, builtin, error, and
standard-library declarations into typed shapes. Downstream passes pattern
match those shapes rather than interpreting AST spelling repeatedly.

## Semantic checking

`marrow-check` owns:

- project/module discovery and uniqueness;
- declaration and use-site identity arenas;
- name and type resolution;
- expression and statement checking;
- presence narrowing;
- direct and transitive durable/host effects;
- index and write validity;
- evolution intent checking and read-only discharge;
- lowering into runtime bodies; and
- snapshot-aware analysis facts for editor and tool consumers.

The central artifacts and IDs live in `program.rs`, `model/`, and `facts.rs`.
The main driver is in `driver.rs`; focused passes live under `checks/`,
`presence/`, `evolution/`, and `tooling/`.

## Current durable identity

The `catalog/` pass reconciles source declarations with an accepted catalog
snapshot or committed lock projection. It proposes stable `CatalogId` values,
rename aliases, retirement, fingerprints, and catalog epochs. This is current
behavior but not the target semantic-path architecture; see
[Legacy implementation](legacy.md).

## Analysis API

`analysis.rs` builds an `AnalysisSnapshot` without mutating a store. Typed
cursor facts, symbols, hovers, completions, semantic tokens, saved-data path
facts, entry effects, and evolution previews are exposed through compiler-owned
views. The downstream LSP should request missing facts here rather than infer
them from syntax or diagnostic text.
