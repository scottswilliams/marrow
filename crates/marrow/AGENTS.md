# marrow — Agent Notes

The CLI binary and long-running surfaces: command dispatch, check/run/test/fmt,
data, evolve, backup/restore, the LSP server, and `marrow serve`.
Maps: [docs/implementation/cli.md](../../docs/implementation/cli.md) and
[docs/implementation/serve-lsp.md](../../docs/implementation/serve-lsp.md).

**You MUST keep these maps current.** On any high-level change here — a command,
module, type, or invariant added, removed, renamed, or reshaped — review the
matching page (cli.md or serve-lsp.md) and update it IN PLACE in the same
change, as concisely as possible: rewrite the affected lines and delete what
went stale. It is imperative the maps never accrue agentic sediment — no
appended notes, history, or duplicate lines; they are thin maps, not changelogs.
Trivial edits that change nothing at the map's altitude need no update.
