# Compiled programs

This page is future direction. Marrow currently executes a checked in-memory
representation with a tree-walking interpreter.

## Goal

Compilation should produce a reproducible, immutable program image without
opening a user store. A loader verifies the image before execution, and a
portable reference VM executes only verified images.

The likely first target is compact bytecode rather than native code or a JIT.
Bytecode provides a concrete compiled artifact, deterministic portable
semantics, explicit host imports, source mapping, bounded verification, and a
tractable reference implementation.

## Constraints

- Source validation completes before lowering.
- The same explicit build inputs produce the same canonical image bytes.
- Image identity is independent of store location and deployment settings.
- Malformed, overlarge, or version-incompatible images fail before execution.
- The image records types, callables, semantic paths, effects, source maps, and
  required host imports through versioned sections.
- Optimizations may not change source-observable behavior and are not required
  for the first target.
- Compilation remains storeless; a durable compiler cache must not become
  semantic authority.

## Open work

Instruction forms, number representation, module initialization, linking,
debug mappings, loader compatibility, optimization validation, and resource
budgets must be learned from the compiler and reference-machine implementation.
They should be documented canonically as they become current rather than
frozen in a separate speculative format document.
