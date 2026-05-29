# Resources And Saved Data

Future counterpart of
[`../../language/resources-and-storage.md`](../../language/resources-and-storage.md).

## Assigned stable element IDs

Today an element's stable identity is a name-derived string token written by
hand, `@id("book.title")`, which marks a field or layer's rename identity. The
approved redesign replaces that string with an assigned, opaque stable id: an
id-typed token allocated by the LSP rather than derived from the element name.
Because the token is assigned and not name-shaped, a rename never desyncs the
identity, and one uniform marker covers every element. The current name-derived
`@id("...")` remains the implemented form until this lands.

## GUID identity allocation

Saved-root identities are allocated as a single auto-incrementing `int`. A
designed extension adds a GUID allocation policy, written `^x(id: guid)`,
alongside that single `int` policy, for identities that must be unique without a
central counter.
