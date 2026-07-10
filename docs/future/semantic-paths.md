# Semantic paths

This page is future direction. Current catalog metadata represents accepted
declaration identity, but it is not the final semantic-path model.

## Goal

The compiler should own one versioned graph of durable schema paths. Related
representations remain distinct:

| Representation | Role |
|---|---|
| Source spelling | A name in one program version. |
| Semantic path identity | A stable compiler identity for one declaration node. |
| Entry identity | Typed keys selecting one concrete entry, such as `Id(^orders)`. |
| Concrete address | A semantic path instantiated with entry keys. |
| Public URI | An explicit external projection of selected addresses. |
| Authority region | A typed set of addresses a principal may access. |
| Physical key | A private substrate encoding. |

Evolution relates graph versions. A rename may preserve identity; a split,
merge, retirement, or incompatible reshape may require an explicit transition.
Source names, public compatibility, and physical layout therefore cannot be
collapsed into one string or identifier.

## Constraints

- Compilation receives identity provenance as an explicit reproducible input.
- The live store and ambient entropy do not invent source semantics during
  compilation.
- Each representation boundary has one typed converter and one owner.
- The logical tree and physical substrate do not interpret compiler schema.
- Removed identities are not silently reused.
- Public path and authority projections derive from semantic paths without
  becoming their canonical identity.
