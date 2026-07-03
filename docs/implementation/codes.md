# marrow-codes

The diagnostic code registry. Every dotted error-code string in the toolchain —
`parse.syntax`, `check.unknown_type`, `run.overflow`, `store.corruption`, and the
rest — is one variant of the `Code` enum, defined once in the `codes!` table in
`crates/marrow-codes/src/lib.rs`. The table carries each code's family, severity
class, catchability, lifecycle, and documented meaning. Lifecycle marks how a code
reaches a user: `Active` codes are emitted with a public product surface;
`Internal` codes are emitted only as defense-in-depth fail-closed guards with no
public product repro (a lower layer classifies every reachable case first), and
drive the reference's internal-codes section; a `Reserved` code is not emitted at
all, documenting a future contract, and a tidy gate proves no production emit site
names it until it flips to `Active`.

Every crate that emits a code depends on `marrow-codes` and renders the wire
string through `Code::as_str`, so a code string is spelled in exactly one place.
The named code constants across the crates (`CHECK_UNKNOWN_TYPE`, `RUN_TYPE`, …)
derive their value from the registry and cannot drift from it.

`Family` fixes the tooling `kind` a code reports; `kind_for_code` classifies any
string (including reserved look-alikes) and is what generic string consumers such
as the language server call.

`docs/error-codes.md` is generated output: `generate` (`src/docs.rs`) renders the
page from the narrative segments plus a table row per code from `Code::meaning`.
The drift test in `tests/error_codes_doc.rs` keeps the committed page byte-identical
to the registry; regenerate with `MARROW_UPDATE_ERROR_CODES=1` and review the diff
as a documented-meaning contract change.

## Read next

- `crates/marrow-codes/src/lib.rs` — the `codes!` table and `Code`.
- `crates/marrow-codes/src/docs.rs` — `generate`, the `docs/error-codes.md` renderer.
