# CLI Reference

## Dry-run

```
marrow run --dry-run [--entry <module::function>] [--maintenance] \
  [--trace] [--format text|json|jsonl] <projectdir>
```

`--dry-run` runs the entry, reports the saved-data writes it would commit, then
rolls them back. The store is left byte-for-byte unchanged: the run rides one
outer savepoint that is always rolled back, so its managed writes — including
those inside a `transaction` block — stage and then discard together.

Only saved data is rewound. Side effects outside saved data (a `std::io` file
write, a `std::log` line) are not rolled back.

Output is a tooling report and takes `--format`. The program's own `print`/`write`
output stays on stdout; under text the planned writes are reported on stderr as
`would write <path>` / `would delete <path>` lines and a `dry run: N write(s),
M delete(s) (rolled back)` summary, and under `json`/`jsonl` as a
`{"committed": false, "planned": […]}` object whose entries carry the op, the
human path, and base64 of the value bytes.

Dry-run is the preview a maintenance migration wants: run a migration under
`--dry-run --maintenance` to see exactly what it would write before committing it.
See [migrations.md](../migrations.md).

## Execution trace

```
marrow run --trace [--format text|json|jsonl] <projectdir>
marrow test --trace [--format text|json|jsonl] <projectdir>
```

`--trace` reports each statement as it runs — file, line, call depth, and the
visible locals — and each managed write or delete, in execution order. It is
opt-in; an untraced `run`/`test` pays nothing and its output is unchanged.

It takes `--format`. Under text the trace is an indented, depth-aware stream on
stderr, leaving the program's own output on stdout; a write line nests under the
statement that produced it. Under `json`/`jsonl` it emits one `step` record per
statement (file, line, depth, locals) and one `write` record per managed write
(op, path, base64 value, depth). `test --trace` attributes each event to the test
it belongs to by name.

`--trace` composes with `--dry-run`: the run is traced and its writes are then
discarded.

## explain

```
marrow explain [--format text|json|jsonl] <projectdir> <target>
```

`marrow explain` statically explains a target with no run. It is read-only.

A `^path` target reports its path/index plan: the root and resource it names, the
resolved class — a scalar leaf and its type, a generated index entry, a key-type
mismatch, or an orphan — and, for a field, the indexes it participates in. The
classification is the same one `data integrity` applies per record, so explain
and the integrity check agree on what each path means.

A name target reports its resolution — found (with the owning module and kind),
ambiguous (with the candidate modules), not visible (a non-`pub` name reached by a
qualified path), or unresolved — through the same resolver the checker and runtime
use, so explain can never disagree with them.

## Non-empty `marrow restore` (replace, merge, repair)

`marrow restore` writes into an empty target only; a non-empty target fails with
`restore.not_empty`. Restoring over existing data — the replace, merge, and
repair modes — is an explicit maintenance action routed through the maintenance
capability, not a relaxation of the empty-target guard. Until they ship, empty
the target first with a maintenance run, then restore into the empty store.

See [`../cli.md`](../cli.md) for the restore command as it works today.
