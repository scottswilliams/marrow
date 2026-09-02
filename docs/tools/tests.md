# Tests

`marrow test` runs every `test` declaration in the project at the working directory and reports each outcome. The `test` declaration and `assert` are defined in [language/tests](../language/tests.md).

## Usage

```text
marrow test [--format text | jsonl] [--filter <substring>]
```

`--filter` runs the tests whose title contains the substring. `--format` chooses the text report, the default, or JSONL.

Tests run in ascending title order. A test that touches no durable place runs with no store. A test that reads or writes a durable place runs against its own ephemeral attachment: a fresh in-memory store that is discarded when the test ends.

The command exits `0` when every selected test passes, `1` when any test fails or errors, and `2` on a usage error. A filter that matches no test is a usage error:

```text
$ marrow test --filter zzz
no test matches the filter; run marrow --help for usage
```

## Reports

A test has one of four outcomes. It passes when its body runs to the end with every `assert` true. It fails when an `assert` is false, reported as `run.assert`. It errors on any other runtime fault. It is incomplete when a durable fault interrupts a commit; the report then adds the durable state, `known_old`, `known_new`, or `unknown`, and the summary counts it as errored.

The text report prints one line per test and a summary:

```text
$ marrow test
ok    add then read back
ERROR overflow (run.overflow at 29:24)
FAIL  shelf count (run.assert at 20:5)
1 passed, 1 failed, 1 errored (3/3 selected)
```

A passing line carries the title. A failing or erroring line adds the fault's code and position. An incomplete test prints as `ERROR` with `incomplete, durable <state>` after the position. The summary counts the selected tests against the total the project declares.

JSONL output emits one `kind: "test"` object per test and a final `kind: "summary"` object, with each object's keys in ascending byte order:

```text
$ marrow test --format jsonl
{"file":"src/docs/tests/report.mw","kind":"test","name":"add then read back","outcome":"passed","span":{"column":6,"line":23}}
{"code":"run.overflow","file":"src/docs/tests/report.mw","kind":"test","name":"overflow","outcome":"errored","span":{"column":24,"line":29}}
{"code":"run.assert","file":"src/docs/tests/report.mw","kind":"test","name":"shelf count","outcome":"failed","span":{"column":5,"line":20}}
{"errored":1,"failed":1,"kind":"summary","passed":1,"selected":3,"total":3}
```

A passed test's `span` is its declaration; a failed, errored, or incomplete test's `span` and `code` are the fault's. An incomplete test also carries a `durable` field. Dotted codes are defined in the [error code reference](../error-codes.md).

A project that does not compile runs no test. The command prints each diagnostic as a `kind: "run"` record and exits `1`. In text that is `check.type at 4:12`. In JSONL it is:

```text
{"code":"check.type","kind":"run","outcome":"diagnostic","span":{"column":12,"line":4}}
```

`marrow check .` shows the same diagnostic with its file and message.
