# Semantic paths

This page is future direction. Current catalog identity is useful prototype
behavior but is not the final package-aware semantic-path model.

## Goal

The compiler should own one typed graph of durable declarations. Related
identities remain separate:

| Identity or representation | Role |
|---|---|
| Package locator | Where acquisition can request source bytes |
| Package lineage | Stable nominal origin across ordinary updates and repository moves |
| Package snapshot | Exact canonical source content |
| Declaration identity | Stable package-owned identity where compatibility or durable meaning requires it |
| Source spelling | Name in one source revision |
| Durable representation | Concrete value/key shape and codec meaning |
| Semantic path | Stable durable declaration in the program contract |
| Concrete address | Semantic path instantiated with typed key values |
| Store identity | One actual durable module instance |
| Executable binding | Exact code, graph, effects, limits, and accepted authority for a store |
| Public path | Later external representation of selected behavior or addresses |
| Physical key | Private kernel/engine encoding |

The current language's `Id(^root)` value fits this taxonomy as a typed key
value: it names one record within one root and instantiates a semantic path
into a concrete address. It is not a declaration identity, a store identity, or
a physical key.

Private implementation helpers use image-local identity unless they cross a
public, durable, wire, or accepted-authority boundary. Stable identities are not
minted during check/build, derived from URLs or source order, or invented by a
live store.

The compiler also distinguishes an exact finite-value place from a keyed child
branch. These are typed designations, not fetched proxies, local collections,
serializable path values, or authority. General first-class and public generic
path values are not required for the beta.

## Constraints

- Stable identity provenance is an explicit reproducible source input.
- The source-controlled identity ledger records provenance and continuity, not
  a second schema, and opaque stable IDs do not appear in ordinary business
  source.
- Removed stable IDs are not silently reused.
- Each conversion boundary has one typed owner.
- The compiler never emits a physical key; the engine never interprets Marrow
  source meaning.
- A checked rename may preserve one identity and representation. Copy, split,
  merge, retype, retirement, and ambiguous manual edits require a fresh identity,
  an explicitly supported transition, or rejection.

Identity continuity cannot establish that a maintainer has preserved human
meaning. Reinterpreting unchanged retained bytes is a review error rather than
a compiler theorem; an intentional new meaning receives a fresh identity.

## Evidence target

Reorder, rename, package move/update, image rebuild, activation, backup, and
restore fixtures must demonstrate which identities remain stable and which
change, without relying on a populated store during compilation.
