# check/catalog — stable durable identity

After type analysis builds a `CheckedProgram`, this layer gives every durable declaration (resource, store, store index, enum, enum member, resource member) an opaque stable id that survives renames and reshapes, and derives the source digests the store stamps and the activation fence enforces. It is read-only: `bind_catalog` reconciles source against the persisted accepted catalog and *proposes* an advanced catalog, but `check` never writes the catalog. A separate `rejected_surface` pass rejects v0.1-forbidden saved-traversal calls and saved `inout`.

## The big idea

Identity is path-independent. A stable id is a random 128-bit `cat_<32hex>` minted from OS entropy, not derived from the source path — so renaming an entity carries its id forward instead of inventing a new one, and branch-parallel allocation cannot collide the way a counter would. Reconciliation matches source entries to accepted entries by `(kind, path)`, relocates renamed entries (recording the old path as an alias), reserves retired entries, and mints fresh ids only for genuinely new ones. Structural change is detected from recorded signatures, not source spelling, so a re-key, a group↔keyed-group reshape, or a value retype advances the proposal even when names are unchanged.

## Parts

- **Binding** (`catalog/mod.rs`): `bind_catalog` orchestrates carry-forward, rename relocation, retire reservation, and fresh-id mint, then records store-key/member signatures, builds the proposal, and binds accepted-only ids onto facts.
- **Source digest** (`catalog/source_digest.rs`): two sha256 digests over re-read, re-parsed, canonically-formatted durable declarations. The shape digest excludes the `evolve` block (a consumed block is deletable); the evolution digest includes it.
- **Id allocator** (`catalog/stable_id.rs`): opaque random id minting, re-rolling against every recorded id of any lifecycle so retired ids are never reissued.
- **Rejected surface** (`rejected_surface.rs`): walks function/const/transform bodies and rejects forbidden saved-traversal methods and saved `inout`.

## Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow-check/src/catalog/mod.rs` | Reconcile source vs accepted catalog; carry-forward, rename, retire, mint; record signatures; build and bind the proposal. |
| `crates/marrow-check/src/catalog/source_digest.rs` | Compute shape and shape-plus-evolve sha256 digests by rendering durable declarations through the canonical formatter. |
| `crates/marrow-check/src/catalog/stable_id.rs` | `StableIdAllocator`: random `cat_<32hex>` ids from OS entropy, collision-retried against recorded ids. |
| `crates/marrow-check/src/rejected_surface.rs` | Reject v0.1-forbidden saved-traversal method calls and saved `inout`, emitting `check.rejected_surface`. |

## Contracts that bite

- **Two id maps by design.** `ids` (accepted-only) binds onto live facts; `leaf_token_ids` / the proposal-inclusive map covers freshly-minted and renamed referents but is used *only* for leaf-token resolution and never binds onto facts — a proposal-only identity cannot leak into the program.
- **Proposal advances only on real change.** Exact source-vs-accepted match returns `None`. The proposal is validated at check time (`proposal.validate()`), so an id collision the binding produced fails closed here, not at apply. Backfilling a signature for an entry that never had one recorded is not change.
- **Rename is identity-preserving and injective.** A rename relocates the accepted entry to its new path and keeps it Active; resolution is rejected if the source path is still declared, the target already names a live entity, or no active accepted entry backs the source path. A retire only reserves once the source declaration is gone.
- **A pending entity is a Warning, not a failure.** Source not yet accepted and not renamed stays informational; identity is frozen only when run/apply commits.
- **The formatter is a frozen anchor.** Reformatting an unchanged shape must not drift the digest. This subsystem documents the invariant but does not assert a golden of its own (the formatter golden lives elsewhere), so a formatter change is an unguarded drift risk here. An unreadable/unparsable module contributes a path-tagged `Unreadable` marker rather than colliding with a clean rendering.

## Read next

- `catalog/mod.rs` → `bind_against_accepted`, `bind_source_entries`, `resolve_renames` — the core of carry-forward, rename relocation, retire, and mint.
- `catalog/mod.rs` → `record_signatures`, `store_key_shapes`, `member_structs` — how reshape/re-key/retype is detected independent of spelling.
- `catalog/mod.rs` → `CatalogBinding`, `CatalogKey`, `active_proposal_id_map`, `rebind_activation_resume_program` — the binding result, the `(kind, path)` index, and the activation-resume rebind path.
- `catalog/source_digest.rs` → `render_declarations`, `digest_of`, `analyzed_source_digest`, `evolution_digest` — the shape vs shape-plus-evolve fences.
- `catalog/stable_id.rs` → `StableIdAllocator::allocate`, `over` — path-independence and retired-id exclusion.
- `rejected_surface.rs` → `check_rejected_surface`, `REJECTED_TRAVERSAL_METHODS` — the single owner of the rejected operator vocabulary.
