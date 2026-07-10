# marrow-schema Contributor Notes

This crate lowers one resource, store, or enum declaration into the current
typed shape used by checker and runtime code. It is distinct from the syntax
AST and project-wide semantic resolution.

`classify_key_type` is the single orderable-key verdict. `Type::optional` is the
one optionality-flattening constructor, and `NodeKind::Slot` is the durable-leaf
choke point. Reject illegal shapes at construction so downstream code can
pattern-match typed states. Every module retains its ownership-level `//!`
documentation.

Schema shapes feed semantic-path construction. This crate does not own stable
schema path identity, URI spelling, authorization scope, graph-version
evolution relations, or physical storage keys.

Map: [docs/implementation/compiler.md](../../docs/implementation/compiler.md).
