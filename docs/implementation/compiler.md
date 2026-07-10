# Compiler implementation

The current front end and checker span `marrow-syntax`, `marrow-project`,
`marrow-schema`, `marrow-check`, and `marrow-catalog`. They produce a
`CheckedProgram` and a syntax-free `CheckedRuntimeProgram` for the interpreter;
they do not yet emit a portable program image.

## Source and project inputs

`marrow-syntax` owns tokens, the AST, parse diagnostics, and formatting.
`marrow-project` owns configuration and filesystem discovery: it parses and
validates `marrow.json`, enumerates configured `.mw` files, and derives each
expected module name from its path. It does not read source text, apply editor
overlays, or establish semantic uniqueness.

`marrow-check` orchestrates that discovery. It resolves source overlays before
disk reads, parses source through `marrow-syntax`, checks declared module paths,
enforces semantic module uniqueness across the program, and computes the
checked source digest.

## Schema shapes

`marrow-schema` compiles resource, store, enum, builtin, error, and
standard-library declarations into typed shapes. Downstream passes pattern
match those shapes rather than interpreting AST spelling repeatedly.

## Semantic checking

`marrow-check` owns:

- discovery orchestration, source overlays, module-path validation, and
  semantic uniqueness;
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

## Transitional store dependency

`marrow-schema` and `marrow-check` depend directly on `marrow-store` for the
current scalar type and codec. The checker also consumes its saved keys,
catalog identifiers, read-only tree access for evolution discharge, and data
tooling. This is transitional compiler/storage coupling; the checker does not
enable the native redb backend.

## Analysis API

`analysis.rs` builds an `AnalysisSnapshot` without mutating a store. Typed
cursor facts, symbols, hovers, completions, semantic tokens, saved-data path
facts, entry effects, and evolution previews are exposed through compiler-owned
views. The downstream LSP should request missing facts here rather than infer
them from syntax or diagnostic text.
