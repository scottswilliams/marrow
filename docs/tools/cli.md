# CLI

`marrow` creates, formats, checks, runs, and tests a [project](projects.md),
audits a store bound to it, and writes the artifacts a deployment ships.

```text
marrow init <projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow check [--demand] [projectdir]
marrow run <export> [--stdin] [--store <dir>] [--format text | jsonl] [-- <args>...]
marrow test [--format text | jsonl] [--filter <substring>]
marrow import --store <dir> --jsonl <path> --root <name> [--keys <key,...>]
marrow doctor --store <dir> [--format text | jsonl]
marrow image --out <dir> --accept-ceiling <id>
marrow client typescript [--out <dir>]
marrow --version
marrow --help
```

A flag takes its value as the next argument, as in `--store ./store`;
`--store=./store` is a usage error. `marrow --version` prints `marrow 0.1.0`.
The transcripts below come from one project holding this file at
`src/docs/cli/shelf.mw`:

```mw
module docs::cli::shelf

resource Book {
    required title: string
    required isbn: string
}

store ^books[id: int]: Book {
    index byIsbn[isbn] unique
}

pub fn put(id: int, title: string, isbn: string) {
    transaction {
        ^books[id] = Book(title: title, isbn: isbn)
    }
}

pub fn lookup(isbn: string): string? {
    if const id = ^books.byIsbn[isbn] {
        return ^books[id].title
    }
    return absent
}

pub fn greet(name: string): string {
    return $"Hello, {name}!"
}

test "put then lookup" {
    put(1, "Small Gods", "978-0552152976")
    assert lookup("978-0552152976") ?? "" == "Small Gods"
}
```

`greet` touches no durable place. `put` and `lookup` are durable exports.

## marrow init

`marrow init <projectdir>` creates a project: a `marrow.toml` manifest and a
`src/main.mw` script holding an empty `main`.

```text
$ marrow init shelf
created shelf
next steps:
  cd shelf
  marrow fmt --check shelf
```

A directory that already exists is `config.invalid`. `init` creates no store.

## marrow fmt

`marrow fmt` puts source in canonical form. With no flag it prints one file
formatted, or checks a project without writing. `--check` names each file that
is not canonical and exits `1`. `--write` rewrites those files in place.

```text
$ marrow fmt --check messy.mw
messy.mw: not formatted; run marrow fmt --write messy.mw to format it
$ marrow fmt --write messy.mw
$ cat messy.mw
pub fn add(a: int, b: int): int {
    return a + b
}
```

A file that does not parse is left as it is and reported with `parse.syntax`.
`fmt` does not read standard input.

## marrow check

`marrow check` type-checks the project and prints every diagnostic with its
file, 1-based line, and column, as in `src/docs/cli/shelf.mw:26:12: check.type:
found int where string is required`. It opens no store and runs no code.

`check` runs the compiler once over the project with its `test` declarations
included, so a diagnostic in a test body is reported beside the others, and
every stage's diagnostics over every module are reported together. A project
that checks clean has that same test-inclusive program encoded and verified,
and the demand below is reconstructed by the verifier from that image. A fixed
bound that only the test entries cross therefore refuses `check` while
`marrow run` and `marrow image`, whose image excludes tests, still succeed: a
project of one export and 257 `test` declarations reports
`cli.compiler_resource_limit: the compiler reached a fixed resource limit: the
test entry table is full` from `check` and runs its export. The editor's
snapshot fact retention bound is not consulted by `check`.

A project that checks clean prints its access demand: the durable places each
export reads and writes ([access
demand](../language/durable-places.md#access-demand)). The default form groups
exports by module and counts the places under each root:

```text
$ marrow check .
3 exports across 1 module

docs.cli.shelf: 3 exports
  lookup
    reads ^books (+2 places)
  put
    reads ^books
    writes ^books
  storeless: greet
```

`--demand` names every place, one line per export:

```text
$ marrow check --demand .
docs.cli.shelf.greet reads or writes no durable data
docs.cli.shelf.lookup reads ^books.byIsbn and ^books.title
docs.cli.shelf.put reads ^books; writes ^books
```

The two places `lookup` reads are the index and one field. Demand describes
the access a program requires; it grants nothing. A fresh durable project
reports `check.durable_identity` until one `marrow run` writes `.marrow/ids`
([identity ledger](projects.md#identity-ledger)).

## marrow run

`marrow run <export>` compiles and verifies the project, then runs one export,
named bare or by module: `greet` or `docs.cli.shelf.greet`. Arguments after
`--` are decoded in order against the export's scalar parameters: `int`, `bool`,
`string`, `bytes` as `0x`-prefixed lowercase hexadecimal, and `date`, `instant`,
and `duration` in canonical text. A struct parameter has no command-line
spelling. A wrong count, a value that does not decode, or an unknown export is a
usage error.

`--stdin` supplies one string from standard input instead of positional
arguments. The verified export must take exactly one nonoptional `string`
parameter; its return type is unrestricted. An empty `--` tail is allowed,
but actual positional arguments conflict with `--stdin`. The signature is
checked before input is read.

Input is UTF-8, limited to 65,536 bytes. Empty input, NUL, carriage returns and
trailing newlines are preserved. The argument reader materializes at most
65,537 bytes and refuses excess input without waiting for EOF. Standard input
buffering may read ahead beyond those bytes. Invalid UTF-8, excess input and
read failures report `io.read` and exit `1` before invocation. Within the limit,
the command waits for EOF; it imposes no input timeout. For example, from the
Graph Report project, `marrow run graph_report.report --stdin < graph.txt`
passes the file's contents to the ordinary report export.

```text
$ marrow run greet -- Ann
Hello, Ann!
$ marrow run greet --format jsonl -- Ann
{"data":"Hello, Ann!","kind":"run","outcome":"value"}
```

Text output is the returned value, or `absent` for an absent optional. A
nonempty rendering gains one LF; an empty string or unit emits no text. JSONL
output is one object whose `outcome` is `value`, `diagnostic`,
`artifact_rejected`, `fault`, `incomplete`, `outcome_unknown`, or `error`; a
diagnostic or fault carries its code and span
([error codes](../error-codes.md)).

A returned bare string is limited to 65,536 raw UTF-8 bytes in either format.
JSON escaping can expand each byte sixfold: the complete string value record
is at most 393,259 bytes including its terminating LF. This is not a bound on
arbitrary aggregate text rendering or on total diagnostic output. A rendering
refusal reports `io.write` and exits `1`. Write or flush failure also exits `1`,
with an `io.write` message on standard error if that channel remains writable.
Output may be partial. The invocation may already have completed; delivery
failure does not undo it or retry it.

A durable export runs against a store on disk named with `--store <dir>`. The
store is opened by the companion runner installed beside `marrow`; without that
layout the command stops with `cli.installation_damaged`
([install](../install.md#running-against-a-store)). A durable export run with
no `--store` prints `cli.durable_unsupported` and exits `1`. This transcript
is from an install with the layout, on the notes program of the
[quickstart](../quickstart.md); [operations](../operations/README.md) covers
the store between runs:

```text
$ marrow run textOf --store ./store -- 1
imported note
$ marrow run add --store ./store -- 3 "added via run"
true
```

`--stdin` also works with `--store`. Companion discovery retains precedence
over argument decoding and stdin consumption. Compilation and existing ledger
publication can occur before input is read; input refusal is not a guarantee
that project metadata was untouched.

The first storeless `marrow run` of a project with durable declarations also
writes `.marrow/ids`; commit that file. `marrow run --store` leaves it as it is.

## marrow test

`marrow test` runs every `test` declaration in the project and reports each
outcome. A test that touches a durable place runs against a fresh in-memory
store of its own ([tests](tests.md)).

```text
$ marrow test
ok    put then lookup
1 passed, 0 failed, 0 errored (1/1 selected)
```

`--filter <substring>` selects tests by title; a filter that matches nothing is
a usage error. `--format jsonl` prints one object per test and a summary.

## marrow import

`marrow import` creates a store and fills it from a file of JSON objects, one
entry per line. Each member is a scalar. Its name is either a key component of the root, named
in `--keys`, or a field of the stored resource. The project is compiled and verified first
and the new store is bound to it. An existing store is filled only when the
project is its active program: a code-only edit is `store.image_not_active`
until `marrow run --store` rebinds the store, and a changed durable contract
is `store.contract_changed`. Like `run --store`, `import` needs the
companion layout. The transcript is from the quickstart's notes program:

```text
$ marrow import --store ./store --jsonl seed.jsonl --root notes --keys id
provisioned a fresh store at ./store
{"batches_committed":1,"rows_imported":2}
```

The file is read and committed in bounded batches. `import` writes no
identity: a missing one is `check.durable_identity`. An existing store with an
unsupported format generation is `store.format_version`; import does not
convert it ([changing the program](../operations/README.md#changing-the-program)).

## marrow doctor

`marrow doctor --store <dir>` performs a read-only logical audit against the
project at the working directory, which must be the store's active program.
A code-only edit the store has not been rebound to is `store.image_not_active`;
a changed durable contract is `store.contract_changed`. The companion runner
holds the owner lock through admission and inspection, then releases it before
printing. The engine file, head, and envelope are unchanged.
An unsupported store format is `store.format_version`, refused before engine
open ([compatibility](../compatibility.md#versioning)).

The audit covers every cell, including an empty physical key and data beneath
absent parents. It checks keys and values against their declared types, entry
markers, required fields, managed-index correspondence, and the commit witness.
An index must agree with its source entry, and every present entry with a
complete index projection must have a corresponding index cell naming that
entry. An entry whose fields are all sparse may have no populated fields.

The report lists at most 256 findings, each with a stable `store.*` code and a
place ([error codes](../error-codes.md)). Findings follow deterministic scan
and node-closure order: missing required fields are reported when their node
closes. An index is named by its identity from `.marrow/ids`, because the
compiled program carries no index name.

`entries` counts concrete entry markers at every level; absent ancestors add
no entries. `index cells` counts managed-index cells, and `cells` counts stored
cells once each. The digest covers declared entry-family keys and values in
key order, including malformed cells in those families and children beneath
absent parents. Index, metadata, and undeclared-family cells are excluded;
undeclared cells still produce findings. It is reported, not persisted. An
unchanged content stream has the same digest; a same-value commit need not change it.

Physical integrity is not checked. A changed scalar that still has a valid
value can pass this audit even when its physical checksum is wrong. Inspection
does not repair the engine or clear the unclean-shutdown status inherited from
a prior owner, and it does not qualify the store for recovery
([operations](../operations/README.md#auditing-a-store)).
The text report starts with `Logical store audit:` and states
`Physical integrity was not checked.` Exit `0` means the walk found no logical
inconsistency; findings, engine errors, and refusals exit `1`.

`--format jsonl` prints one `doctor` record followed by one `finding` record per
listed finding. A completed walk reports `scope: "logical"`,
`physical_integrity: "not_checked"`, and `outcome: "clean"` or `"findings"`,
together with its counts, instance, image, and digest. `findings` counts all
findings and `listed` counts the records that follow; each finding carries
`code` and `place`. A lifecycle refusal or engine read failure reports
`outcome: "error"` and its `code`. Project compilation and companion-installation
failures go to standard error in either format; compiler resource and invariant
failures retain `cli.compiler_resource_limit` and `cli.compiler_invariant`.

## marrow image

`marrow image` compiles and verifies the project and writes `program.image`, the
artifact a deployment ships, into `--out <dir>`. The image's demand is its
deployment ceiling, and `--accept-ceiling` names that ceiling's id. Without the
right id, the command prints the id and the demand and writes nothing:

```text
$ marrow image --out img
cli.ceiling_unaccepted: this image's deployment ceiling id is b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691; re-run with --accept-ceiling b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691 to compose the deployment image after reviewing the demand printed below
docs.cli.shelf.greet reads or writes no durable data
docs.cli.shelf.lookup reads ^books.byIsbn and ^books.title
docs.cli.shelf.put reads ^books; writes ^books
$ marrow image --out img --accept-ceiling b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691
image <image-id>
ceiling b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691
img/program.image
```

`<image-id>` stands for the image's 64-digit hexadecimal id. The same source,
identity ledger, and toolchain yield the same image and the same ids. `image`
opens no store and writes no identity.

## marrow client typescript

`marrow client typescript` compiles and verifies the project and writes a
TypeScript client for its exports into `--out <dir>`, `client` by default.

```text
$ marrow client typescript
client/client.mts
client/marrow-supervisor.mjs
client/marrow-supervisor.d.mts
```

`client.mts` has one `async` method per export with exact types. The other two
files are the Node module that starts and supervises the runner
([TypeScript client](typescript-client.md)). Every Marrow value type has a
transfer type, so a project that verifies also generates.

## Exit codes

| Code | Meaning |
|---:|---|
| `0` | The command completed. |
| `1` | A diagnostic, fault, or operational error was reported, `doctor` found something wrong with the store, or the command has no implementation today. |
| `2` | The command line was wrong: a bare `marrow`, an unknown command or export, a bad flag or argument, or a filter that matches nothing. |

`data`, `evolve`, `serve`, `backup`, and `restore` are recognized names with
no implementation today; each reports `cli.command_unsupported` and exits `1`
([status](../status.md)).
