# marrow-schema — Agent Notes

Compiles one resource, store, or enum declaration into the typed tree/store/enum shape downstream code
pattern-matches instead of re-parsing. Distinct from the parser (AST) and the checker (project-wide
resolution).

Keep one owner per concept: `classify_key_type` is the single orderable-key verdict shared by identity
keys, key params, index args, and local keys; `Type::optional` is the one flattening constructor;
`NodeKind::Slot` is the one durable-leaf choke-point with a fail-closed `debug_assert`. Derive a
diagnostic's code from its typed kind; validate at the boundary so an illegal shape is unrepresentable
(C-VALIDATE). Every module carries a `//!` header — hold that.

Map: [docs/implementation/schema.md](../../docs/implementation/schema.md).
