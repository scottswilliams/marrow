# Data Evolution And Project Compilation

Future counterpart of [`../data-evolution.md`](../data-evolution.md). This page
records the intended project-compilation model; it is not implemented today.

Durable data should eventually compile with source, catalog identity, actual
saved data, and the selected store target. The compiler/planner can then decide
whether a change affects real data, public compatibility, derived structures, or
only source spelling.

## Compilation Inputs

Planned project compilation has three levels:

| Mode | Inputs | Use |
|---|---|---|
| Source-only | `.mw` source | fast parse, format, typecheck, editor feedback |
| Catalog-bound | source + project catalog | stable identity, public aliases, schema epochs |
| Data-attached | source + catalog + saved-data snapshot + store target | integrity, evolution, repair, restore, store recompilation |

The storage engine owns durable bytes and ordered keys. Marrow owns their
meaning.

## Planned Outcomes

The future planner can emit outcomes such as:

- `NoOp`: source or catalog changed, but saved data and public compatibility are
  unaffected.
- `CatalogOnly`: refresh stable identity records, aliases, epochs, docs, or
  fingerprints without rewriting saved data.
- `DataProof`: inspect actual data to prove the change is valid without a
  rewrite.
- `DerivedRebuild`: rebuild generated indexes or other derived structures from
  base data.
- `StoreRecompile`: write the same typed data into a different physical layout,
  archive, backend, or store format.
- `Transform`: a checked `evolve transform` recomputes a top-level member per record
  from the record's other, still-decodable members through a body that is a pure
  function of `old` (no saved reads, saved writes, host effects, or transactions).
- `DestructiveDecisionRequired`: populated data would be deleted, abandoned, or
  hidden.
- `RepairRequired`: saved data is already invalid under the catalog it claims to
  use.

Developer tooling should render these details through a small activation
vocabulary: `safe`, `needs apply`, `needs job`, `blocked`, and `needs approval`.
Operator tooling may expose the internal job states, but those states are not a
second user model.

## Online Activation Jobs

Future OLTP activation is a compiler-owned job, not a migration script. The
source, accepted catalog, checked facts, engine profile, and data snapshot define
the semantics. A job records execution evidence derived from an exact preview
witness.

The intended protocol is:

1. `preview` produces the exact witness.
2. `start` creates a durable job from that witness.
3. `backfill` processes bounded, deterministic chunks.
4. `verify` proves required fields, transforms, derived indexes, uniqueness, and
   shadow-layout identity facts.
5. `publish` advances the readable catalog epoch in a small commit.
6. `close` drains old runtime generations, removes adapters, and purges retired
   physical state.

The v0.1 implementation may collapse those steps into one exact local apply, but
the public facts should not assume a future online system can hold a global
write fence for the entire backfill.

## Compatibility Windows

Future server runtimes admit compiled programs by catalog epoch and runtime
generation. The default v0.1 policy remains exact epoch/schema equality. A future
online compatibility window is finite and normally spans one old epoch to one new
epoch.

Old reads may use compiler-generated typed adapters. Old writes are rejected
unless the compiler proves an adapter lowers them to the latest write plan and
maintains every active or building durable fact. Adapters are named,
digest-stamped, visible to tooling, and deleted when the window closes.

## Shadow Decant

Changing a store's identity key shape, reshaping a populated resource/layer, or
moving between layouts/engines may require a shadow-decant workflow instead of
an in-place backfill. Shadow decant writes a new store or layout in chunks,
bridges a bounded set of writes, verifies identity/count/checksum facts, publishes
a small binding change, and then closes the compatibility window. It is the
Marrow-native version of online copy/cutover, still governed by source and
catalog facts rather than raw store rewrites.

## Stable Identity

Durable identity should be catalog-owned and opaque. Source names are authoring
spellings, public aliases are client/tool spellings, and physical key segments
are storage layout. Those concepts must not collapse into one string.

Source-level stable-id annotations are not the durable identity model. Future
rename tooling should read and write catalog identity instead of treating source
metadata as authoritative.

## Typed Transforms

Typed transforms are not the default answer to source changes. They are required
when stored meaning changes and the compiler cannot prove the existing data
remains valid.

A transform should operate over typed old and new schema views. It should run
with restricted effects, bounded traversal, cancellation/checkpoint behavior,
dry-run preview output, and destructive approval where needed.

## Store Recompilation

Changing the storage engine is also compilation. Marrow should be able to read a
consistent typed snapshot from the old store target, validate it against source
and catalog, and write the same durable program data to the new target.

## Repair And Typed Artifacts

Future export/import artifacts carry enough catalog/source fingerprinting to
validate what their saved data means, and import compiles the artifact into the
target project/store.
