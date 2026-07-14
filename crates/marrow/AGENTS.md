# marrow CLI Contributor Notes

On the beta line this is a thin CLI: `marrow init` creates a project, and
`marrow fmt` formats a single `.mw` file or every module of a project directory
through the retained formatter, plus `--version`/`--help`. Every other command
name (`check`, `run`, `test`, `data`, `doctor`, `evolve`, `serve`, `client`,
`backup`, `restore`) is recognized and reports a typed
`cli.command_unsupported` response until its refounding lane lands it. The
binary depends only on `marrow-codes`, `marrow-project`, and `marrow-syntax`;
the physical project-capture adapter here feeds the pure `marrow-project`
owner and never rebuilds discovery or identity.

`term_style` is the single painting owner, and one named usage-exit owner
handles command-line usage failures. Output is text only until a command that
needs structured output returns; prefer typed state over behavior-selecting
booleans and keep reusable logic below the binary.

The binary owns no language, semantic path, public URI, or authorization
meaning. A refounded command consumes compiler-owned semantics; it does not
reconstruct them here.
