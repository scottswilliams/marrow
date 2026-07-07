# marrow-codes — Agent Notes

The diagnostic-code registry: one `Code` per dotted error code, the single owner of every code
string with its family, severity, catchability, and documented meaning. Add or change a code in the
`codes!` table in `src/lib.rs` only, then regenerate `docs/error-codes.md` with
`MARROW_UPDATE_ERROR_CODES=1` and review the diff. Never spell a code string as a literal elsewhere —
an absence test enforces it.

The byte-exact doc drift gate plus the per-code coverage test are the enforcement artifacts every
other crate imitates; this registry is the workspace's single-ownership exemplar (rust-analyzer's
`DiagnosticCode`). The raw-string `codes!` rows are a deliberate style, not lint debt.

Map: [docs/implementation/codes.md](../../docs/implementation/codes.md).
