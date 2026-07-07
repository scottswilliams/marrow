# marrow-check — Agent Notes

The semantic spine: resolution, types, presence/effects, catalog identity, lowering, data-evolution
proofs, and the analysis API. It produces the `CheckedProgram`; run, the CLI, and tooling never
re-derive semantics from it.

Diagnostics are a typed `Code` plus a `DiagnosticPayload`, with prose owned only by
`diagnostic_render.rs`; secondary locations are payload fields, not sentence fragments. Nominal
identity (types, catalog ids) is typed/interned and compared by value, not by a formatted string. The
analysis API hands back POD snapshot facts with version semantics — the marrow-lsp repo consumes it, so
add facts here before adapting the LSP. Exemplars: `facts.rs` id newtypes, `DiagnosticAnchor` (a
zeroed span is unrepresentable). Decompose a dispatcher by invariant before it grows a second concern.

Map: [docs/implementation/check/](../../docs/implementation/check/README.md).
