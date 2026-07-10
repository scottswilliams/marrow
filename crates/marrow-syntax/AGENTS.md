# marrow-syntax Contributor Notes

This leaf crate owns source text, AST construction, formatting, and parse
diagnostics. Its only dependency is `marrow-codes`; no edge points upward into
checking, runtime, catalog, or storage.

Parsing is total. A failure remains an error node with a diagnostic rather than
dropping syntax. Decode literal and type grammar once, return `Option` for
absent children, and prefer typed parser state over booleans.

Syntax owns source spelling only. It never defines stable schema path identity,
URI text, authority scope, graph-version evolution relations, or physical
encoding.

Map: [docs/implementation/syntax.md](../../docs/implementation/syntax.md).
