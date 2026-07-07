# marrow-json — Agent Notes

Bounded, machine-readable DTOs: serialize `CheckedProgram` and runtime facts into typed JSON envelopes
and decode surface requests. It is not a general `Value` codec and does not define HTTP serving.

Wire twins mirror their upstream owner through an exhaustive `From`, so a new upstream variant is a
compile error here, never silent drift. Prose lives only in `Display`; read a typed `code` and typed
`message`, never string-strip one field out of another's rendered form. Bounded materialization must
not scan omitted data. `resource_schema.rs` and `serve.rs` are the live query-native analysis API the
marrow-lsp repo consumes — keep them and add facts here before adapting the LSP. Organize a
serialization module by verb (define the DTO, convert into it, emit it), and keep the behavioral suite
under `tests/`, not inline. Precedent: serde's `value/` split; BurntSushi typed errors.

Map: [docs/implementation/json.md](../../docs/implementation/json.md).
