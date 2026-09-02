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
| Store identity | One actual durable module instance | Future |
| Executable binding | Exact code, graph, effects, limits, and accepted authority for a store | Current; authority is future |
| Public path | Later external representation of selected behavior or addresses | Future |
| Physical key | Private kernel/engine encoding | Current, private to the engine |

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

The compiler owns one typed graph of durable declarations, and package
lineage, store identity, and public paths join the identities above. Private
helpers keep image-local identity unless they cross a public, durable, wire, or
accepted-authority boundary. Stable identity provenance is an explicit
reproducible source input: the ledger records provenance and continuity, and
opaque stable identifiers stay out of ordinary business source.

A checked rename preserves one identity and representation. Copy, split,
merge, retype, retirement, and an ambiguous manual edit take a fresh identity,
an explicitly supported transition, or a rejection
([admission and activation](admission-and-activation.md)). A stable identity
says the bytes are the same; a change of meaning takes a new identity.

## Evidence

Reorder, rename, package move and update, image rebuild, activation, backup,
and restore fixtures show which identities stay stable and which change,
without a populated store during compilation.
