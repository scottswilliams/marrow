# marrow-catalog Contributor Notes

This crate owns the current durable-identity catalog: accepted epochs, digests,
entries, lock projection, validation, and structural-signature decoding.
Checker and storage code consume its validated values.

Constructors must establish digest and self-consistency invariants before a
catalog value exists. Corruption returns a typed error and never panics.
Structural wire grammar has one reader, and filtered key sets derive from one
typed source rather than parallel string classifiers.

Catalog IDs and epochs are current implementation mechanisms. Do not collapse
them with entry identities or store UIDs, or use source paths, catalog paths,
or lock spellings as public URI identity, authorization scope, physical key
identity, or the future semantic-path model.

Map: [docs/implementation/check/](../../docs/implementation/check/README.md).
