# marrow-syntax — Agent Notes

Source text to AST, plus the formatter and parse diagnostics. A leaf crate: its only dependency is
marrow-codes, and no edge points up into check/run/catalog/store — keep it that way.

Parsing is total. `parse_source` returns `ParsedSource { file, diagnostics }`, never a `Result`; a
failure rides as an `Expression::Error` / `Statement::Error` node beside a `Vec<Diagnostic>`, and AST
accessors return `Option` for absent children. `literal.rs` owns string/bytes escapes and `parse_type`
owns type spelling — decode each grammar once so no downstream crate re-reads it. Prefer typed frame
inputs over boolean flags (`parse_decl/body.rs` `DocComments` / `StrayBlock`). Precedent: the
rust-analyzer `syntax` crate.

Map: [docs/implementation/syntax.md](../../docs/implementation/syntax.md).
