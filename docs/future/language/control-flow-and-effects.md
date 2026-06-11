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

if const title = ^books(id).title
    write(title)
```

The `else` block must not fall through to the following statement. Every path
through it must leave the current continuation with `return`, `throw`, `break`,
`continue`, or another non-fallthrough construct the checker understands.
Because false exits, facts proven by the condition are available after the
statement; `require exists(path) else ...` narrows `path` the same way the true
branch of an `if exists(path)` does.

`require` is not an assertion that can be disabled. It is ordinary checked
control flow and its `else` block uses the same effect and transaction rules
as any other block.
