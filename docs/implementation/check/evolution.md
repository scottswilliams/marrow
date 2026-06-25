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
- **A populated rename is real re-address work.** A rename moves a store root, member, or enum-value spelling without rewriting record bytes, but re-addressing populated cells is a non-additive identity change the run-time auto-apply set excludes. `discharge_root` counts the records under any place a rename touches into `DischargeCounts.records_to_readdress`, so the run fences and the preview reports pending work; a rename against an empty store re-addresses nothing and stays a free auto-apply.
- **A populated enum re-parent orphans its values.** An enum member's saved identity is keyed on its full ancestor path, so re-parenting a leaf under a fresh `category` mints a new identity the proposal never carries onto the facts. `EnumMembers` therefore treats a snapshotted enum not covered by an identity-preserving rename this cycle as constrained: a stored value naming a member absent from its current set is orphaned and `leaf_value_valid` fails it closed, so a plain run fences toward `evolve apply`. An empty store, a reorder (paths unchanged), or a re-parent carried by an explicit `rename` stays a free activation.
- **Records stream, never materialize.** One paged `for_each_place_record` scan per root fuses presence probing and unique-index key derivation. Required-leaf state is bounded; each unique-index probe keeps a seen set proportional to the number of distinct populated key tuples and reports distinct colliding tuples, not duplicate-record counts.
- **A transform body is a pure total function of `old`.** Reading saved data directly bypasses the per-member decodability proof and fails closed; the transform target is excluded from the presence scan and its verdict carries the decodability proof.
- **A transform is suppressed once applied.** A shape-neutral in-place transform moves no structural signature, so without a durable mark its data work would re-derive on every preview and re-execute forever. The catalog binding stamps the transform's own identity — a hash of the target's stable id and the transform body — into the target `CatalogEntry.applied_transform` (advancing the epoch on apply), and `pending_transform_ids` treats a transform whose accepted target already records that identity as consumed: it leaves the presence scan to classify the now-settled member and stages no transform write. Keying on the transform's own target and body, not the whole-program shape, means a later unrelated durable edit cannot re-open a discharged transform, while a changed body computes a different identity and reads as a fresh obligation.
- **A default must be a checker-evaluable constant on a scalar leaf.** `const_default` is the single interpreter; apply writes the encoded bytes verbatim and never re-reads source, so a non-scalar or per-record-varying fill is a typed `RejectedDefault`. An `ErrorCode` leaf additionally validates the literal through the one grammar owner `marrow_schema::error`, so a non-conforming default is a `RejectedDefault` rather than pre-encoded invalid bytes.
- **Baselines are typed by durable surface.** A store/member with no recorded accepted token/struct/key-shape places no obligation; the proposal freezes the current shape forward so a later change has a baseline. A store index must have an accepted declaration shape; a missing shape is invalid accepted metadata rather than an evolution obligation.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/evolution/mod.rs` | Module root: declares submodules and re-exports the public preview/witness surface. |
| `crates/marrow-check/src/evolution/intents.rs` | Intent extraction and evolve-body type checking: collects rename/retire/default/transform intents, maps target spellings to catalog paths, type-checks defaults and transform bodies, enforces transform read restrictions and purity. |
| `crates/marrow-check/src/evolution/discharge/mod.rs` | The discharge root: the `discharge` driver that orders the families, the `Accumulator` that collects verdicts/counts/changed-id partitions/diagnostics and resolves defaults, `RepairDiagnostic`, and the shared id/label helpers. |
| `crates/marrow-check/src/evolution/discharge/leaf_obligations.rs` | The per-record presence scan: `discharge_root` and the keyed-layer scan, the leaf-obligation walkers, and `classify_leaf`/`classify_member_leaf` — the per-leaf verdict (retype-vs-populated, required/optional/renamed, default fill, enum validity). |
| `crates/marrow-check/src/evolution/discharge/index.rs` | Unique-index probing: builds the per-record key-tuple plan, accumulates collisions streaming, and classifies each index to a derived rebuild or a fail-closed repair (collision or unprobeable). |
| `crates/marrow-check/src/evolution/discharge/transforms.rs` | Discharges `evolve transform` targets: `pending_transform_ids` drops transforms the accepted target already records as applied, then for each pending one proves every read member decodes, else fails the target closed. |
| `crates/marrow-check/src/evolution/discharge/structural_backstop.rs` | The default-deny backstop: fails closed any populated member whose identity-aware structural signature diverged and that no targeted classifier claimed. |
| `crates/marrow-check/src/evolution/discharge/enum_shrink.rs` | Selectable enum members (current and accepted), shrunk-enum and constrained-enum detection, and the stored-value validity check `leaf_value_valid` that fails an orphaned (re-parented) member closed. |
| `crates/marrow-check/src/evolution/discharge/accepted_state.rs` | Reads the accepted catalog baselines (changed ids, renamed catalog ids of any kind, the enum ids a renamed member moves and the enums an identity-preserving rename covers, accepted leaf tokens/structs, and store-key shapes), infers the single-candidate same-shape enum-member rename that turns an orphaned-value repair into rename guidance, and the store-key-shape re-key backstop. |
| `crates/marrow-check/src/evolution/discharge/absent_source.rs` | Classifies accepted catalog entries current source no longer declares (`absent_source_entries`: the proposal's reserved retires plus the accepted rows the bare-removal projection dropped): retire over data, source-dropped index (deletes derived cells), a populated dependency-free member dropped with no retire (fail-closed), a populated whole store dropped with the resource (fail-closed, one fence per root), the same drops over an empty member or store (no-op), a member an index still reads, nested retire. |
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
