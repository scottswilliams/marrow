# marrow-codes Contributor Notes

This crate is the single registry for dotted diagnostic codes, families,
severity, catchability, and documented meaning. Add or change a code in the
`codes!` table in `src/lib.rs`, regenerate `docs/error-codes.md` with
`MARROW_UPDATE_ERROR_CODES=1`, and review the exact diff.

Do not spell registered code strings elsewhere. Semantic code uses typed
`Code`; renderers produce prose. The generated-document drift gate and per-code
coverage are the enforcement artifacts for this ownership boundary.

