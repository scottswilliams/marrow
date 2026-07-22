# BND02 — Representation rings for production scale · design map

**Lane artifact, not for integration.** Committed to `lane/bnd02` before any
implementation (the H00f/H00a owner-map precedent); deleted on integration. Base:
`origin/beta` @ `89335652` (G03). All file:line evidence read at this tip.

**Scope (from the packet + `phases.md` BND02 + board note `607`).** Two representation
ceilings BND01 could not move by constants, plus the four BND01 carry-forwards:

- **C1** — retire eager per-field operation-site emission (~84 image B/field), so durable
  width tracks `MAX_RECORD_FIELDS` (4096); includes the `MAX_IDS_ROWS` (~4091) true binder.
- **C2** — design the u32-count image ring for beyond-u16 tables (owner: millions of
  functions) as a versioned format decision, reject-only v0→v1 with a new digest kind.
- **CF1** — proof-fork narrowing (immutable shared baseline + per-template append-only overlay).
- **CF2** — per-mint metadata-directory rebuild → session-scoped/incremental.
- **CF3** — collection-dedup scan → keyed index.
- **CF4** — the eager per-field-site coupling (owned by C1) + `MAX_IDS_ROWS`.

**Preservation laws (hard, from the packet).** Verifier-independent effect reconstruction
and the path-kernel demand model stay intact; no second executor or parallel format;
byte-identity where the format version does not change; diagnostic identity (the 4096-limit
byte-exact tests); the frozen GENPERF01/BND01 recurrence KATs flip only with the repairs
they fence, in the same commits.

---

## Evidence base — how sites, counts, and the ledger are represented today

**Site emission (C1/CF4).** A durable "operation site" is a `SiteDef { path: SemanticPath,
target: SemanticTarget }` (`marrow-image/src/draft.rs:221-273`), appended to the image
`sites: Vec<SiteDef>` via `add_site` (`draft.rs:517-521`), encoded into the DURABLE section
0x03 as `u8(step_count) ‖ step* ‖ target_byte` per site (`encode.rs:290-300, 604-615`). A
field-leaf site path is `[application, root, field]` = 3 steps × (1 kind byte + 16 id
bytes) + 1 count + 1 target = **~52 bytes**, plus the member-tree field entry (~19 B) and the
interned name — the measured ~83.82 B/declared field (`bounds.rs:53-62`).

Sites are minted **eagerly, one FieldLeaf per declared field**, in three declaration-width
loops in `marrow-compile/src/durable.rs`: `emit_root_member_sites` (`:1627-1631`),
`emit_branch_sites` (`:1676-1680`), and the parked `emit_subtree_sites` "for identity
completeness" (`:1715-1719`) — all through `emit_field_site` (`:1590-1599`). One
`WholePayload` per root (`:701-705`), one Index scan/lookup per managed index (`:718-723`).
The lowerer does **not** emit; it resolves an executable field to its pre-minted site index
by **field name** (`lower/durable.rs:234-247` → `DurableField.site`, wired positionally at
`durable.rs:797-807`, `1853-1861`), and bakes that `u16` into `Instr::DurReadField(site)` etc.

**Site consumption — the preservation-law proof (verified by independent trace).**
- The verifier reconstructs the graph node set **independently from the decoded member
  tree** — `durable_descriptor(application, &roots).semantic_nodes()`
  (`marrow-verify/src/verify/durable.rs:152-153`; nodes at `semantic.rs:181-188`). It
  iterates *emitted* sites and resolves each against that node set
  (`durable.rs:167-177, 275-281`) but **never requires every node to carry a site**.
- The contract id is recomputed from the graph, not from sites
  (`verify/durable.rs:194`; encoder `durable_descriptor` at `encode.rs:314-333` — over
  roots/members, **sites contribute no contract-id byte**).
- Demand atoms are `(SemanticPath, OperationClass)` built **only from instructions that
  reference a site** (`verify/context.rs:60-76`; `image/src/demand.rs:11-19`); kernel
  authority is `demand ⊆ ceiling ∩ grant` over that referenced-atom set
  (`kernel/src/durable/store/handle.rs:241-253`).
- Runtime resolves the `u16` operand as a **direct index** into a table index-aligned with
  the image sites (`kernel/.../read_session.rs:28-30`; built `vm/src/attach.rs:285-293`).

**Conclusion (C1 is safe):** un-emitted untouched-field sites contribute nothing to the node
set, the contract id, demand, or authority. Retiring eager emission changes **only** which
sites populate the (shrunk) SITES table and the `SiteId` operands baked into durable
bytecode. The sole coupling is the *cardinality assertions* in
`crates/marrow/tests/operation_sites.rs` (e.g. `sites().len() == 1 + field_count`,
`:322-341`; `+1 site per appended field`, `:288-320`) — the very invariant the lane retires;
`durable_demand.rs` is already touched-field-lazy and unaffected.

**u16 counts (C2).** Every image table count and cross-table operand is `push_u16`
(`encode.rs`): strings/types/fields/enums/variants/collections/roots/sites/functions/
exports/test-entries/members/indexes/value-leaves, plus code operands `Call(func)`,
`RecordNew(type)`, `FieldGet(field)`, `Dur*(site)`, `EnumConstruct{enum,variant}`,
`LocalGet(local)`. Container `VERSION = 0x00` (`encode.rs:26`), digest kind
`marrow.image.v0` (`digest.rs:12`), verifier rejects any other version with a typed
`Envelope` refusal (`verify/container.rs:16-38`). The seam for a versioned ring **already
exists** and is enforced.

**Ledger cap (C1/CF4).** `MAX_IDS_ROWS = 4096`, `MAX_IDS_BYTES = 1 MiB`
(`marrow-project/src/ids.rs:38-39`), enforced at `ids.rs:398`. Each declared durable field
resolves exactly one `IdentityKind::Field` anchor (`durable.rs:598-601`, `1317`); a
4096-field resource needs ~4096 field rows + application + product + root + key ≈ **4100+
rows > 4096**, so `MAX_IDS_ROWS` — **not** `MAX_IMAGE_BYTES` — is the binder that caps the
reachable durable width at ~4091 today.

**Carry-forward owners (post-GF01/BND01/NEST01 tip).**
- CF1 proof fork: `TypeRegistry::clone_for_generic_check` (`compile/src/types.rs:760`) +
  `draft.clone()` per template (`lower/mod.rs:535-536`), entered once per generic *template*
  (`compile.rs:1045`); cost `O(templates × (registry rows + draft size))`
  (`proof_clone_rows = proof_clones × type_insts.len()`).
- CF2 metadata directory: `MetadataScratch::try_new` (`types.rs:1144-1160`). The **mint**
  path is already reused/incremental (NEST01: `row_directory`, `types.rs:970-978`); the
  surviving quadratic is the **per-field-projection presentation rebuild**, frozen by KAT
  `type_axis_scan_is_linear_field_projection_rebuild_still_quadratic`
  (`generic_scaling_counts_tests.rs:193-237`, `row_ratio >= 3.5`).
- CF3 collection dedup: `instantiate_collection` linear scan
  `collections.iter().position(|c| *c == spec)` (`types.rs:4457-4476`), `O(collections²)`,
  reachable under `MAX_COLLECTIONS = 4096`. Type/fn instantiation dedup is **already** a keyed
  `HashMap<(template,args), row>` with drift detection (`types.rs:773, 780`, BND01) — CF3 is
  the same family, unrepaired for collections.

**Reachability shift since GENPERF01.** GENPERF01 closed measurement-only because
`MAX_TYPES/ENUMS/FUNCTIONS = 64` capped reachable instantiations far below every threshold.
BND01 raised those to **4096**, so the CF quadratics are now reachable at 512→2048→4095 and
the KAT header itself states "the 512/1024/2048 doubling points are now reachable." CF2/CF3
are genuine reachable reds now, not moot.

---

## C1 — Retire eager per-field site emission + raise the ledger binder

**Current owner.** `marrow-compile/src/durable.rs` (emission) + `lower/durable.rs`
(name→`.site` resolution); `marrow-image/src/draft.rs` (`add_site`); `marrow-project/src/ids.rs`
(`MAX_IDS_ROWS`).

**Proposed representation.**
1. **Lazy, deduplicating site allocation.** Replace eager per-field emission with an
   on-demand allocator on the draft/lowering context: a `HashMap<(SemanticPath,
   SemanticTarget), SiteId>` that mints-or-returns a `SiteId` the first time an instruction
   references a node/target. The three `emit_field_site` loops and the positional
   `DurableField.site` wiring are deleted; the lowerer computes a field's `SemanticPath` from
   its ledger id + parent step chain at reference time (the same data it already holds) and
   calls `alloc_site(path, FieldLeaf)`. `WholePayload`/`Index*` sites flow through the same
   allocator (uniform). The SITES table then contains exactly the sites code references, in
   reference order.
2. **Raise `MAX_IDS_ROWS`** from 4096 → **8192** (monotone bound widen, matching
   `MAX_DURABLE_MEMBERS`), leaving `MAX_IDS_BYTES = 1 MiB` as the true allocation bound —
   the exact pattern the image bounds already follow (`bounds.rs` module header). 8192 rows ×
   ~60 B ≈ 500 KB < 1 MiB. This makes the full `MAX_RECORD_FIELDS = 4096` field guard
   reachable (a 4096-field resource → ~4100 rows < 8192).

**Byte-identity ledger.**
- *Storeless images*: byte-identical (no sites, no ledger rows).
- *Durable images*: **intended change** — the SITES table shrinks to referenced sites and
  `SiteId` operands renumber to reference order. Not gated by a format-version bump (the
  SITES section grammar is unchanged; only its contents). The contract id, member tree,
  record types, demand-set ids, and every non-site byte are unchanged. The differential
  oracle compares typed outcomes (outcome/code/span/data), never image bytes — unaffected.
- *Diagnostics*: unchanged. The `MAX_RECORD_FIELDS` 4096-limit byte-exact tests are about
  field count (member tree / record type), not sites — untouched. `MAX_SITES` refusal bytes
  unchanged (only its reachability changes).
- The `const _` decoupling assert `MAX_SITES >= MAX_RECORD_FIELDS` (`bounds.rs:266-269`)
  becomes obsolete — `MAX_SITES` now tracks referenced-op count (bounded by code size), not
  field width. Replace with `MAX_IDS_ROWS >= MAX_RECORD_FIELDS + overhead` reasoning
  (project-crate assert/KAT).

**KAT plan.**
- Rewrite `operation_sites.rs` from "one site per declared field" to "a site per *referenced*
  (path,target); zero sites for an untouched field." Red-first: the new assertion fails on
  the eager producer.
- A `#[cfg(test)]` op-count/byte KAT: image bytes of a wide resource (say 2000 fields) with a
  body touching 3 fields is O(3 sites), not O(2000) — flips with the repair.
- A reachability fixture: a **4096-field** durable resource compiles cleanly end-to-end
  (currently refused at ~4091 by `MAX_IDS_ROWS`) — the measured exit gate.
- A ledger KAT pinning `MAX_IDS_ROWS = 8192` with `MAX_IDS_BYTES` as the binding bound.

**Verifier/kernel impact.** None to soundness (proven above). The verifier keeps iterating
emitted sites and resolving each against its independent node set; it now sees fewer sites.
The VM `u16→AuthorizedSite` direct index is unaffected (operands still point at their site's
row in the shrunk table). Update: the emission-side eager loops in `durable.rs`; the
`operation_sites.rs` cardinality tests; the design comment `durable.rs:685-696` (sites now
scale with referenced ops, not the graph).

**Verdict: PROCEED (reachable, resolvable, preservation-safe). The C1 headline.**

---

## C2 — u32-count image ring (versioned format decision)

**Current owner.** `marrow-image/src/encode.rs` (all `push_u16` counts + operands),
`digest.rs` (`VERSION`, `IMAGE_DIGEST_KIND`), `marrow-verify/src/verify/container.rs`
(version/kind reject seam).

**Proposed representation (the design, per FR01 §6 + the compatibility posture).** A future
**image v1**: container `VERSION = 0x01`, a **new digest kind `marrow.image.v1`** (the version
is already baked into the digest tag, so a v0 digest can never validate v1 bytes even by
confusion — FR01 §6 finding F-2), table counts and cross-table code operands widened
u16→u32 (or LEB128 varint), everything else structurally identical. **Reject-only v0→v1**: a
toolchain reads exactly its own image version; no read-old shim, ever (images are
exact-toolchain-private, regenerated-and-rebound from source across a toolchain update — FR01
§6). v0 and v1 share **no** byte-identity (distinct digest domain) — which is correct and
required, not a violation.

**What stays byte-identical vs changes.** Under v0 (everything today): byte-identical — this
lane writes **no** v1 bytes. A v1 bump changes *every* image byte (new digest domain); there
is deliberately no cross-version byte-identity to preserve.

**KAT plan (seam-readiness, implementable now).** Pin the version/digest-kind seam so a
future v1 is a clean bump: KATs that `VERSION == 0x00`, digest kind is exactly
`marrow.image.v0` (already at `digest.rs:105-147`), and the verifier rejects `VERSION != 0x00`
with the typed `Envelope` refusal (`container.rs:37-38`). A v1-design note recorded under
`docs/future/` (versioned-ring rule), **no** v1 encoder/decoder KATs (no v1 bytes exist).

**Verifier/kernel impact.** None today (seam already enforced). A future v1 mints a parallel
decoder gated on the version byte — a *replacement path* for beyond-u16 programs, never a
second concurrent format for the same bytes.

**The fork the evidence exposes — and why this item STOPs.**
- The **design** is fully determined (no fork): FR01 §6 + the posture fix the version bump,
  the new digest kind, reject-only, u32 widening. Clean and obvious.
- The **implementation timing** is a fork evidence favors deferring but cannot unilaterally
  close against the owner directive:
  - *Nothing reaches u16.* Shipped bounds are 4096–8192 (`bounds.rs`); the u16 ceiling is
    65,535 (8–16× headroom). `MAX_IMAGE_BYTES = 512 KiB` binds first at ~6,200 durable fields
    (FR01 §4, measured) — and even C1's site retirement moves the byte ceiling to ~17k
    fields, still far below u16. **No compilable beta program can populate a table past u16.**
  - *Monotone widen already covers everything ≤ 65,535* with **zero** format change
    (`bounds.rs` header). Crossing u16 is the only event needing v1.
  - *Implementing v1 now is a full second encoder+decoder+verifier path exercised by no
    reachable program* — dead code on credit (law 8) that law 11 says must be deleted until a
    real caller exists. GENPERF01 closed the identical asymptotic question measurement-only.
  - *Against this*: the owner directive "millions of functions" and the foundation-first /
    version-extensible-seam mandate. The packet **pre-armed exactly this as the one named
    maintainer escalation point.**

**Verdict: STOP this item for the maintainer (pre-armed u32-ring STOP).** Recommendation with
the evidence: **record the v1 design + pin the v0 seam (implementable now); DEFER the v1
encoder/decoder to the lane that first has a reachable beyond-u16 program** (which itself
requires first raising `MAX_IMAGE_BYTES` and the table bounds past u16 — a separate,
later, evidence-driven decision). The maintainer decides whether the scale directive
justifies building v1 now instead.

---

## CF1 — Proof-fork narrowing

**Current owner.** `clone_for_generic_check` (`compile/src/types.rs:760`) + `draft.clone()`
(`lower/mod.rs:535-536`), per generic template.

**Proposed representation.** An immutable shared baseline (the settled registry + draft
prefix) borrowed read-only by every template proof, plus a per-template **append-only
overlay** holding only the rows/draft entries that template's proof appends — discarded when
the proof ends. The proof pass reads through baseline-then-overlay; it never clones the
prefix. Cost `O(templates × overlay)` instead of `O(templates × full size)`.

**Byte-identity.** Zero image-byte change: the proof pass already **discards** its throwaway
draft (`lower/mod.rs:564`); only diagnostics are kept. The overlay reproduces identical
proof diagnostics.

**KAT plan.** A new op-count gate on proof-clone **cost** (`proof_clone_rows`), not entry
count (`proof_clones`, already per-template and unchanged): assert the per-template replayed
row count is O(overlay), not O(all prior rows). The manual report already surfaces
`proof_clone_rows` (`generic_scaling_counts_tests.rs:334-355`).

**Verifier/kernel impact.** None (proof pass is compiler-internal, pre-image).

**Verdict: PROCEED — but a candidate split.** The overlay is the most invasive change
(`TypeRegistry`/`ImageDraft` read-through seams). Per the phase protocol's "two-or-more
surviving red families exceeding the effort-L ceiling mint a second serialized row," if CF1's
overlay + C1 together exceed the lane budget, CF1 splits to a serialized sibling row.

---

## CF2 — Metadata-directory rebuild → session-scoped/incremental

**Current owner.** The per-field-**projection** presentation session rebuilds a fresh
`MetadataScratch` over every prior row (`types.rs:1144-1160`; the mint path is already
incremental via `row_directory`, NEST01).

**Proposed representation.** Extend the reused `row_directory` (or a projection-scoped
equivalent) to the field-projection/presentation path, so a projection classifies only newly
appended rows rather than rebuilding over the whole population — the same incremental
extension NEST01 applied to the mint path.

**Byte-identity.** Zero image/diagnostic change (a directory is a projection of the
append-only owners, never an authority — `types.rs:972-978`). Presentation output identical.

**KAT plan.** Flip `type_axis_scan_is_linear_field_projection_rebuild_still_quadratic`
(`generic_scaling_counts_tests.rs:193-237`): the frozen `row_ratio >= 3.5` assertion becomes
a `~2x linear` assertion **in the same commit** as the repair (the recurrence-KAT law).

**Verifier/kernel impact.** None (compiler-internal).

**Verdict: PROCEED. Reachable red (reachable at 512→2048 post-BND01).**

---

## CF3 — Collection-dedup scan → keyed index

**Current owner.** `instantiate_collection` (`types.rs:4457-4476`), linear
`collections.iter().position(|c| *c == spec)`.

**Proposed representation.** A lookup-only `HashMap<CollSpec, u16>` beside `collections`,
appended in lockstep with the same authority/drift discipline as the existing `type_index`/
`fn_index` (`types.rs:773, 780`) — the `collections` vector stays the sole image-order
authority; the index only accelerates the dedup probe; a row whose spec ≠ the looked-up key
is typed drift (mirroring `MintIndexDrift`), never silently trusted.

**Byte-identity.** Zero image change — the same COLLTYPES rows in the same order (dedup result
identical). Diagnostics unchanged.

**KAT plan.** A `#[cfg(test)]` op-count gate: the collection-dedup probe count is O(mint
attempts) (one keyed probe each), not O(collections²) — the same shape as the existing
type/fn scan KATs (`fn_axis_scan_is_linear`).

**Verifier/kernel impact.** None (compiler-internal).

**Verdict: PROCEED. Reachable red under `MAX_COLLECTIONS = 4096`. Smallest of the four.**

---

## Dependency order & lane-shape decision

Red-first, in dependency order:
1. **C1** (site-emission retirement + `MAX_IDS_ROWS`) — the durable-width headline; its
   width binders first. **Midpoint review touch 2 after C1 lands.**
2. **CF3** (collection index) then **CF2** (projection directory) — small, self-contained
   compiler-internal reds with frozen-KAT flips.
3. **CF1** (proof-fork overlay) — most invasive; **split to a serialized sibling row if C1 +
   CF1 exceed the effort-L / owned-file ceiling** (phase-protocol rule).
4. **C2** — **STOPPED for the maintainer** (design recorded, v0 seam pinned, v1 deferred).

**Reviews (three-touch, kernel tier, detached no-hardlink clones).** Touch 1: this map
(format/identity soundness + idiom/simplicity). Touch 2: after C1. Touch 3: at the end (dual;
soundness incl. hostile ring-version laundering probes on the pinned v0 seam + idiom).

**Measured exit.** The full 4096-field guard reachable in a compiled durable resource
(fixture); before/after image-size (dramatic shrink for sparse-access wide resources) and
compile-wall tables; format KATs for every frozen byte (v0 seam); the recurrence KATs flipped
with their repairs.

## Open risks / questions for review touch 1
- **R1 (C1 ownership seam).** Moving site minting from graph-build (`durable.rs`) to
  reference-time (`lower/durable.rs`) — does any consumer besides the durable lowerer read
  `DurableField.site` before lowering? (Trace says no; confirm under the soundness lens.)
- **R2 (C1 dedup key).** Is `(SemanticPath, SemanticTarget)` the correct dedup key, or must
  `WholePayload`/group/index sites keep a distinct allocation discipline? (They are already
  non-width-scaled; uniform allocation is the simplest, but confirm no ordering invariant in
  the verifier's per-site sealing depends on declaration-ordered sites.)
- **R3 (`MAX_IDS_ROWS` value).** 8192 (match `MAX_DURABLE_MEMBERS`) vs a tighter
  `MAX_RECORD_FIELDS + overhead` — is matching the member-tree bound the right scale-floor
  choice, or does the multi-root project shape (up to `MAX_ROOTS` roots) argue for more, with
  `MAX_IDS_BYTES` the true bound?
- **R4 (C2 STOP framing).** Is deferring v1 with the v0 seam pinned the correct
  recommendation, or does the "millions of functions" directive warrant building v1 now?
  (The pre-armed maintainer decision.)
