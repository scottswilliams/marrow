# Semantic paths

A durable declaration has several distinct identities. Each has one owner, and
the compiler keeps them apart even when they name the same logical location.

| Identity or representation | Role | Status |
|---|---|---|
| Package locator | Where acquisition can request source bytes | Future |
| Package lineage | Stable nominal origin across ordinary updates and repository moves | Future |
| Package snapshot | Exact canonical source content | Future |
| Declaration identity | Stable package-owned identity where compatibility or durable meaning requires it | Current: the `.marrow/ids` ledger |
| Source spelling | Name in one source revision | Current |
| Durable representation | Concrete value/key shape and codec meaning | Current |
| Semantic path | Stable durable declaration in the program contract | Current: the durable contract |
| Concrete address | Semantic path instantiated with typed key values | Current: `^books[7]` |
| Store identity | One actual durable store instance | Current: store UID; hostile file substitution is not authenticated |
| Executable binding | Exact code, graph, effects, limits, and accepted ceiling for a store | Current; fine-grained invocation authority is future |
| Evolution relation | An accepted transition between durable graph versions | Future beyond current contract-preserving rebind |
| Public path | Later external representation of selected behavior or addresses | Future |
| Physical key | Private kernel encoding consumed by the engine | Current |

## Today

A project's durable identities are the `.marrow/ids` ledger and the
durable-contract identity the verifier recomputes
([projects](../tools/projects.md)). `marrow run` mints identities into the
ledger; nothing else does. A removed identity keeps its ledger entry and is not
reused.

The current language's `Id(^root)` value fits this taxonomy as a typed key
value: it names one entry within one root and instantiates a semantic path
into a concrete address. It is not a declaration identity, a store identity, or
a physical key.

A place names a location; it is not a value. The compiler distinguishes an
exact finite-value place from a keyed child branch. The compiler emits no
physical key, and the engine interprets no Marrow source meaning.

## Direction

The compiler owns one typed graph of durable declarations. Future package
identity and public paths project from their own owners without replacing the
existing project ledger or store UID. Private
helpers keep image-local identity unless they cross a public, durable, wire, or
accepted-authority boundary. Stable identity provenance is an explicit
reproducible source input: the ledger records provenance and continuity, and
opaque stable identifiers stay out of ordinary business source.

A checked rename preserves one identity and representation. Copy, split,
merge, retype, retirement, and an ambiguous manual edit take a fresh identity,
an explicitly supported transition, or a rejection
([admission and activation](admission-and-activation.md)). A stable identity
records continuity of a declaration, not equality of source or stored bytes.
Compatibility depends separately on its representation and accepted evolution.

## Evidence

Reorder, rename, package move and update, image rebuild, activation, backup,
and restore fixtures show which identities stay stable and which change,
without a populated store during compilation.
