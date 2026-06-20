# Data-Evolution Check

Decides whether a schema change can activate against data already in the store, and if not, names the exact manual work required. Nothing here mutates the store: discharge stays crate-internal, and `preview`'s `EvolutionWitness` is the only artifact that crosses the check→run boundary into [apply](../runtime/evolution.md) (`crates/marrow-run/src/evolution/`).

The shape is three stages. **Intents** turn each `evolve` block step into a typed declaration and type-check its default/transform body against current source. **Discharge** streams the live store snapshot read-only and classifies every obligation that current source plus the catalog proposal imply into one `Verdict` per `CatalogId`. **Preview** assembles those verdicts plus store fingerprints into the `EvolutionWitness`. Apply re-verifies every witness field before writing.

## The verdict vocabulary

`Verdict` (in `witness.rs`) is the transport contract that crosses crates.
The current verdict variants are `NoOp`, `CatalogOnly`, `IndexDropped`,
`DataProof`, `Default`, `DerivedRebuild`, `Transform`,
`DestructiveDecisionRequired`, and `RepairRequired`. Discharge may activate only
outcomes whose obligation apply can derive from the witness: no-op, identity-only
rename, dropped derived index, proven data claim, checked constant default,
derived index rebuild, or checked transform. `is_activatable` is false for the
two blocking variants: destructive approval still needs scoped operator input,
and repair still needs the snapshot fixed before activation.

`RepairReason` is the typed fail-closed cause: missing required member, rejected default, invalid stored value, unique-index collision or unprobeability, retire required (a dropped member an index still reads, or `PopulatedDropRequiresRetire` for a populated member/store drop with no retire intent), undecodable transform input, a type/key-shape change that needs a transform, and `StructuralDivergence` — the default-deny backstop.

## Load-bearing invariants

- **Fail-closed is total by construction.** Targeted classifiers run first, then `classify_structural_backstop` fails closed any populated member whose identity-aware structural signature diverged and that no classifier claimed. An unforeseen transition cannot silently activate.
- **Identity, not spelling, decides retype.** Leaf and structural tokens name the referent's stable catalog id (enum/store) plus physical key shape (`leaf_type.rs`), so a pure rename is `CatalogOnly` while a real retype changes the token and fails closed.
- **Records stream, never materialize.** One paged `for_each_place_record` scan per root fuses presence probing and unique-index key derivation. Required-leaf state is bounded; each unique-index probe keeps a seen set proportional to the number of distinct populated key tuples and reports distinct colliding tuples, not duplicate-record counts.
- **A transform body is a pure total function of `old`.** Reading saved data directly bypasses the per-member decodability proof and fails closed; the transform target is excluded from the presence scan and its verdict carries the decodability proof.
- **A default must be a checker-evaluable constant on a scalar leaf.** `const_default` is the single interpreter; apply writes the encoded bytes verbatim and never re-reads source, so a non-scalar or per-record-varying fill is a typed `RejectedDefault`.
- **Baselines are typed by durable surface.** A store/member with no recorded accepted token/struct/key-shape places no obligation; the proposal freezes the current shape forward so a later change has a baseline. A store index must have an accepted declaration shape; a missing shape is invalid accepted metadata rather than an evolution obligation.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/evolution/mod.rs` | Module root: declares submodules and re-exports the public preview/witness surface. |
| `crates/marrow-check/src/evolution/intents.rs` | Intent extraction and evolve-body type checking: collects rename/retire/default/transform intents, maps target spellings to catalog paths, type-checks defaults and transform bodies, enforces transform read restrictions and purity. |
| `crates/marrow-check/src/evolution/discharge/mod.rs` | The discharge root: the `discharge` driver that orders the families, the `Accumulator` that collects verdicts/counts/changed-id partitions/diagnostics and resolves defaults, `RepairDiagnostic`, and the shared id/label helpers. |
| `crates/marrow-check/src/evolution/discharge/leaf_obligations.rs` | The per-record presence scan: `discharge_root` and the keyed-layer scan, the leaf-obligation walkers, and `classify_leaf`/`classify_member_leaf` — the per-leaf verdict (retype-vs-populated, required/optional/renamed, default fill, enum validity). |
| `crates/marrow-check/src/evolution/discharge/index.rs` | Unique-index probing: builds the per-record key-tuple plan, accumulates collisions streaming, and classifies each index to a derived rebuild or a fail-closed repair (collision or unprobeable). |
| `crates/marrow-check/src/evolution/discharge/transforms.rs` | Discharges `evolve transform` targets: excludes the target from the presence scan and proves every read member decodes, else fails the target closed. |
| `crates/marrow-check/src/evolution/discharge/structural_backstop.rs` | The default-deny backstop: fails closed any populated member whose identity-aware structural signature diverged and that no targeted classifier claimed. |
| `crates/marrow-check/src/evolution/discharge/enum_shrink.rs` | Selectable enum members (current and accepted), shrunk-enum detection, and the stored-value validity check `leaf_value_valid`. |
| `crates/marrow-check/src/evolution/discharge/accepted_state.rs` | Reads the accepted catalog baselines (changed ids, renamed members, accepted leaf tokens/structs, and store-key shapes) and the store-key-shape re-key backstop. |
| `crates/marrow-check/src/evolution/discharge/absent_source.rs` | Classifies accepted catalog entries current source no longer declares: retire over data, source-dropped index (deletes derived cells), a populated dependency-free member dropped with no retire (fail-closed), a populated whole store dropped with the resource (fail-closed, one fence per root), the same drops over an empty member or store (no-op), a member an index still reads, nested retire. |
| `crates/marrow-check/src/evolution/witness.rs` | The read-only witness types: `Verdict`, `RepairReason`, `DefaultValue`, `ObligationVerdict`, `CatalogFingerprint`, `DischargeCounts`, `EvolutionWitness`, plus `is_activatable`. Prose-free data. |
| `crates/marrow-check/src/evolution/const_default.rs` | Single interpreter of an evolve default literal: evaluates a constant scalar expression to an encoded `DefaultValue` or a typed `RejectedDefault`. |
| `crates/marrow-check/src/evolution/leaf_type.rs` | Identity-aware leaf and structural tokens: derives a member's leaf token and structural signature so a rename leaves the token unchanged and a retype/reshape changes it, and decodes an accepted leaf token back to the `StoreLeafKind` its bytes read as — both directions of the value-token convention live here. |
| `crates/marrow-check/src/evolution/preview.rs` | The analysis entry point: runs discharge, then composes the `EvolutionWitness` from the discharge result plus store commit metadata, engine-profile digest, layout epoch, and source/evolution digests. |
| `crates/marrow-check/src/evolution/transform_reads.rs` | Resolves a transform's read `CatalogId`s to `TransformReadMember` (name, leaf kind) — the one rule discharge and apply share so check and run never drift. |

The evolution module root is `crates/marrow-check/src/evolution/mod.rs`; there
is no separate top-level file for that module.

## Read next

- `crates/marrow-check/src/evolution/witness.rs` → `Verdict`, `RepairReason`, `is_activatable` — the cross-crate vocabulary; read this first.
- `crates/marrow-check/src/evolution/discharge/mod.rs` → `discharge` — top-level ordering: store-key-shape skip, per-root scan, `absent_source` classification, transform discharge, structural backstop.
- `crates/marrow-check/src/evolution/discharge/leaf_obligations.rs` → `classify_leaf` / `classify_member_leaf` — the per-leaf decision tree (retype-vs-populated, required/optional/renamed, default fill, enum validity).
- `crates/marrow-check/src/evolution/preview.rs` → `preview` — how verdicts plus fingerprints become the witness apply consumes.
- `crates/marrow-check/src/evolution/intents.rs` → `check_evolve_types`, `impurity_reason` — the check-time gate that decides which intents reach discharge.
