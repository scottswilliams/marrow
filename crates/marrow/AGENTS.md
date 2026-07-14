# marrow CLI Contributor Notes

On the beta line this is a thin CLI with three implemented commands. `marrow
init` creates a project, and `marrow fmt` formats a single `.mw` file or every
module of a project directory through the retained formatter. `marrow run
<export>` drives the production path: capture the project at the working
directory, compile it to canonical image bytes, verify them into a sealed
image, open a store in-process when the export's verified demand is nonempty
(`--store <path>`; an interim seam that dies at D00), execute on the VM, and
render the value or the first failure as text or, under `--format jsonl`, as
one canonical typed record per line. Every other command name (`check`,
`test`, `data`, `doctor`, `evolve`, `serve`, `client`, `backup`, `restore`) is
recognized and reports a typed `cli.command_unsupported` response until its
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
