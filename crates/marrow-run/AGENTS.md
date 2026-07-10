# marrow-run Contributor Notes

This crate currently contains the tree-walking interpreter, managed write
planning, durable reads and iteration, evolution execution, standard-library
host boundary, and project sessions. These are implementation facts, not a
promise that the final compiler target remains an interpreter.

Never re-resolve names or parse operation strings at runtime. Consume typed
call targets, conversions, paths, capabilities, and value shapes from the
checker. `Value` is the single owner of scalar conversion to and from durable
form. Build one complete typed `WritePlan` before atomic commit, return typed
errors, and page or stream potentially unbounded internal work.

Project surface sessions and operation-tag admission are legacy. Do not expand
them. The target invariant is one typed data-session/path kernel through which
every logical tree access—including embedded, served, evolution, inspection,
logical repair, and logical backup or restore—must pass. Embedded execution
supplies explicit root authority rather than bypassing enforcement. Physical
substrate recovery belongs to a separately typed trusted component in
`marrow-store`; application principals cannot reach it, and a recovered store
cannot return to service until complete validation and admission succeed.

Internal resource limits protect the process; they are not a user-facing cost,
query, or planning model.

Map: [docs/implementation/runtime/](../../docs/implementation/runtime/README.md).
