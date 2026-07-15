# Tests

`marrow test` discovers every `test "name"` declaration in the project at the
working directory, compiles them into a separately verified image, and runs each
one storeless through the bytecode VM. It reports each test's outcome and a final
summary.

The `test` declaration and the `assert` statement it runs are defined in
[language/tests.md](../language/tests.md).

## Usage

```text
marrow test [--format text | jsonl] [--filter <substring>]
```

`--filter` selects tests whose name contains the given substring and fails when
none match. `--format` chooses human text (the default) or typed JSONL.

## Outcomes

A test has one of three outcomes:

- **passed** — the body ran to completion with every `assert` condition true;
- **failed** — an `assert` condition was false, reported as `run.assert`;
- **errored** — any other runtime fault (an overflow, an exhausted budget, and
  so on).

`marrow test` exits `0` when every selected test passes, `1` when any fails or
errors, and `2` on a usage error such as a filter that matches nothing.

## Reports

Text output prints one line per test and a summary line. JSONL output emits one
`kind: "test"` object per test and a final `kind: "summary"` object, with each
object's keys in ascending byte order:

```text
{"file":"src/math.mw","kind":"test","name":"double doubles","outcome":"passed","span":{"column":6,"line":5}}
{"code":"run.assert","file":"src/math.mw","kind":"test","name":"off by one","outcome":"failed","span":{"column":5,"line":9}}
{"errored":0,"failed":1,"kind":"summary","passed":1,"selected":2,"total":2}
```

A passed test's `span` is its declaration; a failed or errored test's `span` and
`code` are the fault's. In the summary, `total` counts the discovered tests and
`selected` counts those run after any filter. Dotted codes are defined in the
[Error Code Reference](../error-codes.md).
