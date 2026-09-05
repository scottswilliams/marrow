# marrow CLI Contributor Notes

On the beta line this is a thin CLI. `marrow
init` creates a project, and `marrow fmt` formats a single `.mw` file or every
captured source file in a project directory through the retained formatter.
`marrow check [projectdir]` captures the project and drives the compiler once,
tests included, for the complete diagnostic set (printing each diagnostic with
its span); when the project checks clean, the test-inclusive image from that same
drive is encoded once and verified to describe each export's
verifier-reconstructed durable demand in source spelling through the
compiler-owned `DurableNaming` join. It opens no store and runs no code; the
sentence describes access and never grants it.
`marrow run
<export>` drives the production path: capture the project at the working
directory (including the machine-written `.marrow/ids` identity ledger), compile
it to canonical image bytes — minting missing durable identities from OS
entropy into `.marrow/ids` first (a `run`-only convenience; every other path
fails precisely, and the accepted apply action supersedes it) through the
adapter's publication owner, which `run` also asks to settle any interrupted
publication before it captures the project or draws entropy, verify them into
a sealed image, open a store in-process when the export's verified demand is
nonempty (`--store <path>`; an interim seam that dies with the durable-run
trough), execute on the VM, and
render the value or the first failure as text or, under `--format jsonl`, as
one canonical typed record per line. `marrow test` drives the same path over
the project's `test` declarations: it compiles them into a separately verified
image carrying the closed TEST-ENTRY table and runs each one — storeless, against
a fresh ephemeral attachment for a direct durable test, or as a driver whose
export calls are each their own invocation boundary — reporting pass/fail/error
per test plus a summary. `marrow client typescript`
compiles and verifies the project, reconstructs its wire interface (the one
transfer/identity owner is `marrow-image`), and emits the deterministic strict
TypeScript client beside the pinned Node supervision module (`src/supervisor/`,
emitted verbatim and drift-gated). Every other command name (`data`,
`doctor`, `evolve`, `serve`, `backup`, `restore`) is recognized and reports a
typed `cli.command_unsupported` response until its refounding lane lands it.
The `marrow-lsp` package is a separate downstream tool. The CLI has no direct or
transitive dependency on `marrow-lsp` or `lsp-types`; a dev-only, std-only
`serde_json` edge in `tests/lsp_stdio.rs` decodes Cargo metadata to enforce that
direction under all features. The server's production stdio suite lives in
`crates/marrow-lsp/tests/lsp_stdio.rs`. The physical project-capture adapter is the separate
`marrow-project-fs` crate; the CLI captures each project through its
`capture_project` with an empty overlay and renders any capture failure through
the adapter's presentation facade, rebuilding no discovery, identity, or capture
classification here.

`outcome` is the typed CLI outcome owner: the four failure families (source
diagnostic, artifact rejection, source-mapped runtime fault, operational
error) stay distinct variants and never collapse into one channel.
`term_style` is the single painting owner, and one named usage-exit owner
handles command-line usage failures. Prefer typed state over
behavior-selecting booleans and keep reusable logic below the binary.

The binary owns no language, semantic path, public URI, or authorization
meaning. A refounded command consumes compiler-owned semantics; it does not
reconstruct them here.
