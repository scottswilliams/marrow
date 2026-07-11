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

`MarrowType` distinguishes the explicit source `unknown` boundary (`Dynamic`),
successful calls with no return value (`NoValue`), unresolved recovery
(`Unknown`), and diagnosed poison (`Invalid`). Source type hover facts are
available for concrete types and explicit dynamic values. No fact is produced
for no-value, unresolved, or invalid states. Poison inspection is recursive
through sequences, optionals, and local keyed-collection keys and values. Strict
value, key, predicate, range, and collection boundaries classify those states
before consulting structural type compatibility, so a dependent expression
propagates diagnosed poison without adding another diagnostic.

`SourceHoverFact` is the canonical combined hover classifier. It selects facts
in callable, module-path, store-root, schema, saved-place, operator, then type
order. Downstream tools consume that fact instead of recreating precedence.
Canonical Marrow type and callable renderers take a `CheckedProgram`, which owns
the nominal declaration identities needed to render module-qualified types.

## Compiler-development type audit

`marrow check --compiler-dev <projectdir>` enables an implementation-maintainer
audit after an otherwise error-free project analysis. It reports
`compiler.dev.unknown_type` as a non-fatal warning when the production function
type walk leaves a value expression at an unresolved-recovery
`MarrowType::Unknown` state. The trace records both an originating expression and
later expressions that propagate the same unresolved type. Explicit source
`unknown` values, no-return calls, diagnosed invalid expressions, and saved
addresses consumed as non-value builtin or traversal subjects are outside the
audit. Local expressions passed through those same collection-shaped positions
remain ordinary typed values and are audited.

The option is intentionally omitted from command help and is not part of the
ordinary project-check contract. Without it, the audit is not invoked and
ordinary output is unchanged. Source files are audited against the source-only
program that checked them. Configured test files are audited against their
combined source-and-test program, after which the analysis snapshot atomically
restores its normal source-only program. The audit reuses the checker's statement
scopes and recursive inference, tokenizes each file once per semantic snapshot,
and asks only the higher-precedence canonical hover owners for actual recovery
sites. Binding positions and common cursor-token/callable/store-root lookups use
indexed access rather than repeated project-wide scans. Snapshots containing
source or configured-test errors suppress the audit because recovery types are
expected after a failed check.

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
