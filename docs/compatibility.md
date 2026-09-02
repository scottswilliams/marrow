# Compatibility

Marrow is unreleased. This revision promises the interfaces below and nothing
beyond them.

## Versioning

The package version is `0.1.0`. `marrow --version` prints it:

```text
marrow 0.1.0
```

There is no release tag, crates.io package, signed binary, or prebuilt
distribution. A build is identified by the source revision it was built from. A
store records the format version it was provisioned with, and a build that does
not support that version refuses to open it (`store.format_version`).

## Platforms

The source builds on Linux and macOS with Rust 1.89. Opening a store on disk is
narrower than the build; [install](install.md#running-against-a-store) names the
platforms.

## Stable interfaces

Three interfaces are written for machines. A diagnostic carries a dotted code
such as `check.type`; [error codes](error-codes.md) lists every code, generated
from the registry. The `marrow` command exits `0`, `1`, or `2` ([exit
codes](tools/cli.md#exit-codes)). With `--format jsonl`, `run` and `test` print
one JSON object per line with fixed field names. Every object carries `kind`. A
`run` or `test` outcome carries `outcome`; a returned value carries `data`; a
failure carries `code`; a test carries `file`, `name`, and `span`; an interrupted
invocation carries `durable`; and the `test` summary carries `selected`, `total`,
`passed`, `failed`, and `errored`.

Human-readable message text may change between revisions. Until a release policy
exists, a structured interface may also change with the implementation; the
reference records the change in the same revision.

## Unstable interfaces

The Rust crates are internal. The public interface is the `marrow` command line,
its exit codes, its JSONL records, and the dotted diagnostic codes. A program
that links a crate directly has no compatibility promise.

Raw store files are private implementation data with no documented format. A
store is bound to one program;
[operations](operations/README.md#changing-the-program) states which program
changes it accepts. Backup, restore, and schema evolution are future work
([status](status.md#not-yet-available)).
