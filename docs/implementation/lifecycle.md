# Lifecycle implementation

Current lifecycle behavior is distributed across compiler catalog binding,
runtime project sessions, CLI commands, and store metadata. Compilation,
attachment, evolution, execution, and lock projection are coupled in this flow.

## Current flow

1. Project discovery reads `marrow.json` and source.
2. The compiler checks source against an accepted store catalog or committed
   `marrow.lock` projection.
3. `ProjectSession` opens a memory or native store and may establish a first
   baseline or auto-apply a zero-mutation schema change.
4. `marrow evolve preview` discharges supported changes read-only and emits a
   state-bound `EvolutionWitness`.
5. `marrow evolve apply` revalidates the witness and commits supported data and
   catalog changes.
6. CLI code reprojects the committed catalog into `marrow.lock`.

The checker side lives under `marrow-check/src/catalog/` and
`marrow-check/src/evolution/`. Apply logic lives under
`marrow-run/src/evolution/`. CLI orchestration is in `cmd_evolve/`, `cmd_run.rs`,
and backup/restore modules.

## Current invariants

- Plain checking does not create or mutate a store.
- A valid live store outranks the lock projection for write-time identity.
- Preview does not apply data changes.
- Apply rechecks its witness against the observed store state.
- Catalog and data changes commit together.
- Restore validates source/catalog compatibility and rebuilds derived indexes.

## Current coupling

Catalog IDs, implicit baselining, zero-mutation auto-apply, lock projection, and
project sessions form one lifecycle family. Changes in this area must trace the
read-only check path, commit path, lock projection, backup/restore, and recovery
together so the accepted state cannot diverge between them.
