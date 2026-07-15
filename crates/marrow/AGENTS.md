# marrow CLI Contributor Notes

On the beta line this is a thin CLI with four implemented commands. `marrow
init` creates a project, and `marrow fmt` formats a single `.mw` file or every
module of a project directory through the retained formatter. `marrow run
<export>` drives the production path: capture the project at the working
directory (including the machine-written `marrow.ids` identity ledger), compile
it to canonical image bytes — minting missing durable identities from OS
entropy into `marrow.ids` first (a `run`-only convenience; every other path
fails precisely, and the accepted apply action supersedes it), verify them into
a sealed image, open a store in-process when the export's verified demand is
nonempty (`--store <path>`; an interim seam that dies with the durable-run
trough), execute on the VM, and
render the value or the first failure as text or, under `--format jsonl`, as
one canonical typed record per line. `marrow test` drives the same path over
the project's `test` declarations: it compiles them into a separately verified
image carrying the closed TEST-ENTRY table and runs each one storeless,
reporting pass/fail/error per test plus a summary. Every other command name
(`check`, `data`, `doctor`, `evolve`, `serve`, `client`, `backup`, `restore`)
is recognized and reports a typed `cli.command_unsupported` response until its
refounding lane lands it. The binary depends on `marrow-codes`,
`marrow-project`, `marrow-syntax`, `marrow-compile`, `marrow-verify`,
`marrow-vm`, and `marrow-kernel`; the physical project-capture adapter here
feeds the pure `marrow-project` owner and never rebuilds discovery or
identity.

`outcome` is the typed CLI outcome owner: the four failure families (source
diagnostic, artifact rejection, source-mapped runtime fault, operational
error) stay distinct variants and never collapse into one channel.
`term_style` is the single painting owner, and one named usage-exit owner
handles command-line usage failures. Prefer typed state over
behavior-selecting booleans and keep reusable logic below the binary.

The binary owns no language, semantic path, public URI, or authorization
meaning. A refounded command consumes compiler-owned semantics; it does not
reconstruct them here.
