# marrow — Agent Notes

The operator binary: command dispatch, check/run/test/fmt, data, evolve, and backup/restore. Each
command parses arguments into a typed args struct, calls the operation, and renders the result.

One format-aware render owner routes text / json / jsonl for every diagnostic surface, and every exit
path of a `--format`-aware command goes through it — a json consumer never scrapes a bare stderr
string. `term_style` is the single painting owner. Prefer a typed state enum over a `bool`
(`RunObservation`, `ServeMode`, `LockStrictness`). A named usage-exit owner replaces bare
`ExitCode::from(2)`. Engine logic a second front-end could call belongs one layer down, not in a
command file. As a binary, this crate is not held to `deny(missing_docs)`, but non-trivial modules
still open with a `//!`.

Map: [docs/implementation/cli.md](../../docs/implementation/cli.md).
