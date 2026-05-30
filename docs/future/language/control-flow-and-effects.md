# Control Flow And Errors

Future counterpart of
[`../../language/control-flow-and-effects.md`](../../language/control-flow-and-effects.md).

## `require ... else`

`require` is a guard statement. It checks a boolean condition, continues when
the condition is true, and runs the `else` block when the condition is false:

```mw
require exists(^books(id)) else
    throw Error(
        code: "book.absent",
        message: $"Book {id} does not exist.",
    )

write(^books(id).title)
```

The `else` block must not fall through to the following statement. Every path
through it must leave the current continuation with `return`, `throw`, `break`,
`continue`, or another non-fallthrough construct the checker understands.
Because false exits, facts proven by the condition are available after the
statement; `require exists(path) else ...` narrows `path` the same way the true
branch of an `if exists(path)` does.

`require` is not an assertion that can be disabled. It is ordinary checked
control flow and its `else` block uses the same effect, transaction, and
`finally` rules as any other block.

## Catchable Evaluator Faults

Deterministic evaluator faults are reported as `Error` values and can be caught
by `try` / `catch` when the runtime can recover without corrupting the current
VM or tool state. These include application-visible faults such as absent reads,
failed supported conversions, numeric overflow, divide-by-zero, invalid
temporal parsing, traversal mutation, and late capability failures.

Fatal conditions remain outside `try` / `catch`: VM corruption, violated
internal invariants, store corruption that prevents a typed result, tooling
protocol failure, process termination, and host failures that leave no
well-formed Marrow `Error`. A `catch` handles program faults, not broken
execution machinery.
