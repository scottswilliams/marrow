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
- `CatalogOnly`: update stable identity records, aliases, epochs, docs, or
  fingerprints without rewriting saved data.
- `DataProof`: inspect actual data to prove the change is valid without a
  rewrite.
- `DerivedRebuild`: rebuild generated indexes or other derived structures from
  base data.
- `StoreRecompile`: write the same typed data into a different physical layout,
  archive, backend, or store format.
- `TypedTransformRequired`: user meaning changed and requires checked old-to-new
  code.
- `DestructiveDecisionRequired`: populated data would be deleted, abandoned, or
  hidden.
- `RepairRequired`: saved data is already invalid under the catalog it claims to
  use.

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
dry-run explain output, and destructive approval where needed.

## Store Recompilation

Changing the storage engine is also compilation. Marrow should be able to read a
consistent typed snapshot from the old store target, validate it against source
and catalog, and write the same durable program data to the new target.

## Repair And Typed Artifacts

Future export/import artifacts carry enough catalog/source fingerprinting to
validate what their saved data means, and import compiles the artifact into the
target project/store.
