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
them. New durable-access work belongs behind a typed runtime boundary shared by
ordinary execution and trusted tools; do not create a side path that evades
compiler facts or later authorization. Physical substrate recovery remains a
separate trusted component in `marrow-store` and does not establish application
validity by itself.

Internal resource limits protect the process; they are not a user-facing cost,
query, or planning model.

Map: [docs/implementation/runtime.md](../../docs/implementation/runtime.md).
