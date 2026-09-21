# CLI

`marrow` creates, formats, checks, runs, and tests a [project](projects.md),
audits or recovers a store bound to it, and writes the artifacts a deployment ships.

Every command that reads a project reads its [declared
dependencies](projects.md#dependencies) with it. A file one of them declares is
reported under the alias the project reaches it by rather than under a path in the
project's own tree, as in `graphtext:src/text.mw:9:5`. Identities stay relative to
their own project, so the alias is a prefix the report adds, never part of the file's
name.

```text
marrow init <projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow check [projectdir]
marrow run <export> [--stdin] [--store <dir>] [--format text | jsonl] [-- <args>...]
marrow test [--format text | jsonl] [--filter <substring>]
marrow import --store <dir> --jsonl <path> --root <name> [--keys <key,...>]
marrow doctor --store <dir> [--format text | jsonl]
marrow apply --store <dir> --old-image <old-image> --new-image <new-image> [--accept-ceiling <id>] [--format text | jsonl]
marrow recover --store <dir> [--image <image>] [--format text | jsonl]
marrow backup --store <dir> --out <backup> [--format text | jsonl]
marrow restore --from <backup> --store <dir> [--format text | jsonl]
marrow image --out <dir> --accept-ceiling <id>
marrow client typescript [--out <dir>]
marrow --version
marrow --help
marrow <command> --help
```

A flag takes its value as the next argument, as in `--store ./store`;
`--store=./store` is a usage error. `marrow --version` prints `marrow 0.1.0`;
`-V` and `version` are the same command, as `-h` and `help` are for `--help`.
Every subcommand prints its own usage on `--help` or `-h`. A flag given twice is
a usage error. A usage error names the problem and the command's help, and exits
`2`:

```text
$ marrow run --bogus
unknown option `--bogus`; run marrow run --help for usage
```

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

pub fn half(n: int): Result<int, string> {
    if n % 2 == 1 {
        return err("odd")
    }
    return ok(n / 2)
}

test "put then lookup" {
    put(1, "Small Gods", "978-0552152976")
    assert lookup("978-0552152976") ?? "" == "Small Gods"
}
```

`greet` and `half` touch no durable place. `put` and `lookup` are durable exports.

## marrow init

`marrow init <projectdir>` creates a project: a `marrow.toml` manifest and a
`src/main.mw` script holding an empty `main`.

```text
$ marrow init shelf
created shelf
next steps:
  cd shelf
  marrow check
  marrow test
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

A project target formats the project's own files. A dependency's file is reported
when it is not canonical and is never rewritten, under `--write` as under `--check`:
the project that declares a file is the project that formats and commits it.

```text
$ marrow fmt --write .
graphtext:src/text.mw: not formatted; format it in the project that declares it
```

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
demand](../language/durable-places.md#access-demand)), grouped by module:

```text
$ marrow check .
4 exports across 1 module

docs.cli.shelf: 4 exports
  lookup
    reads ^books.byIsbn, ^books.title
  put
    reads ^books
    writes ^books
  storeless: greet, half
```

Each export names every place it reads and every place it writes, in source
spelling and ordered by spelling; a `reads` or `writes` line continues on an
indented line rather than run past 96 columns. Adjacent exports of one module
that share an identical demand are listed once, as `alpha, beta (2 exports, one
shared demand)`, and the exports that touch no durable place collapse to one
`storeless:` note; a module with only such exports folds to its header line.

Every export listed is the project's own. A dependency's `pub fn` is callable from
source across the boundary but takes no export slot here, so it appears in no demand
listing and `marrow run` does not name it; a library's exports are run where the
library is.

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

Each argument's text is limited to 65,536 bytes, the language
[text bound](../language/execution-limits.md#limits). The terminal measures the
argument as written — UTF-8 bytes for a `string`, the `0x`-prefixed hexadecimal
for `bytes` — and refuses a longer one with `cli.argument_limit`, exit `1`,
before the export runs. The bound does not depend on `--store`: a storeless run
and a store-backed run admit and refuse the same arguments.

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
nonempty rendering gains one LF; an empty string or unit emits no text. A
returned `Result` is split at the top level: `ok(v)` prints `v` alone, and
`err(e)` prints `error: ` followed by `e` on standard error and exits `1`. A
`Result` nested inside another value keeps its constructor spelling, as in
`Option::some(Result::err(odd))`. A source diagnostic prints as
`file:line:column: code: message`, the same line `check` prints, on standard
output; `check` prints its diagnostics on standard error. JSONL
output is one object whose `outcome` is `value`, `diagnostic`,
`artifact_rejected`, `fault`, `incomplete`, `outcome_unknown`, or `error`; a
diagnostic or fault carries its code and span
([error codes](../error-codes.md)). A returned `err` is a `value` record in
JSONL whose `member` is `err`, and the exit status is `1`. An export whose
`transaction` block exits with `err` has committed its writes, because the block
commits on every normal exit ([transactions](../language/errors-and-transactions.md));
only a fault discards them.

```text
$ marrow run half -- 3
error: odd
$ marrow run half --format jsonl -- 3
{"data":{"enum":"Result","member":"err","payload":["odd"]},"kind":"run","outcome":"value"}
```

A returned bare string is limited to 65,536 raw UTF-8 bytes in either format.
JSON escaping can expand each byte sixfold: the complete string value record
is at most 393,259 bytes including its terminating LF. In JSONL, bare bytes
admit at most 65,536 bytes of unquoted hexadecimal text, including `0x`. JSON aggregate and
identity `data` values admit at most 65,536 encoded bytes, including punctuation
and escaping. Nested values share this limit, and rendering refuses before an
append would exceed it. A present optional at the outer JSONL `data` value uses
its inner value's policy. These limits do not bound aggregate text rendering or
total diagnostic output. A
rendering refusal reports `io.write` and exits `1`. Write or flush failure also exits `1`,
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
over argument admission and stdin consumption, so a damaged installation is
reported before an oversized argument is. Compilation and existing ledger
publication can occur before input is read; input refusal is not a guarantee
that project metadata was untouched.

The invocation result and companion cleanup are reported separately. After a
native call the CLI waits up to 10.1 seconds (a 100 ms grace period plus the 10 s
per-call deadline) for the companion to exit on its own and never terminates it,
because the store close may still be running.
Unconfirmed cleanup reports the observed PID and the retained staging path and
exits `1` without reclassifying or retrying the invocation. The PID is an
observation, not authority to signal a later process reusing that number. See
[operations](../operations/README.md#running-an-export-against-a-store).

The first storeless `marrow run` of a project with durable declarations also
writes `.marrow/ids`; commit that file. `marrow run --store` leaves it as it is.

The mint is the project's own. A durable declaration a dependency makes belongs to
that dependency's ledger, so `run` reports its `check.durable_identity` instead of
minting: run `marrow run` in the library directory and commit the `.marrow/ids` it
writes there. No command run in a consuming project writes into a dependency's tree.

## marrow test

`marrow test [--format text | jsonl] [--filter <substring>]` runs every `test`
declaration in the project and reports each outcome. [Tests](tests.md) covers
selection, the text and JSONL reports, and the four outcomes.

Write or final-flush failure exits `1`, with `io.write` on standard error when
that channel remains writable. Output may be partial; tests are not rerun.
Usage errors retain exit `2` even when their standard-error message cannot be
delivered. The same usage-error rule applies to `marrow run`.

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

The final import receipt must be written and flushed for a successful exit.
Delivery failure exits `1`, with `io.write` on standard error when writable;
it does not undo provisioning or committed batches. Failure to print the
informational fresh-store notice does not stop the import. Do not infer that
the store is unchanged from a failed command or missing receipt.

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
printing. The engine file, head, envelope and ownership marker are unchanged;
an absent marker is not created. Refusal preserves those artifacts too.
An unsupported store format is `store.format_version`, refused before engine
open ([compatibility](../compatibility.md#versioning)).

[Auditing a store](../operations/README.md#auditing-a-store) states what the
walk covers, what a finding means, and what the digest does and does not
establish. The report lists at most 256 findings, each with a stable `store.*`
code and a place ([error codes](../error-codes.md)); an index is named by its
identity from `.marrow/ids`, because the compiled program carries no index name.
`entries` counts concrete entry markers at every level, `index cells` counts
managed-index cells, and `cells` counts stored cells once each.

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

## marrow backup and restore

`marrow backup --store <dir> --out <backup>` compiles the current project and
exports its exact active store through the release-verified runner. A code-only
edit refuses with `store.image_not_active`; backup does not rebind it.
`marrow restore --from <backup> --store <dir>` uses the embedded verified image
and works outside a project. Both require an unoccupied destination and accept
`--format text | jsonl`, defaulting to text.

JSON receipts contain `kind` (`backup` or `restore`), `store`, `backup`, and
`outcome`. A `complete` outcome includes `instance`, `image` and `content_digest`;
backup additionally includes `backup_digest`, which binds the complete transfer
and is distinct from the entry-content digest. Restore's instance is fresh.

An `error` outcome includes `code`. `unpublished` names a possible retained
artifact; it does not certify completeness or exclusive custody of that path.
Backup may report `cleanup_failed: true`. Restore includes a known published
`instance` for publication/activation uncertainty. `store.restore_commit` adds
`batch_outcome` (`aborted` or `indeterminate`) for the failed construction batch;
earlier confirmed batches may remain. Text output carries the same fields, with
the primary failure explanation on stderr.

Success exits `0`; lifecycle or required-output failure exits `1`; invalid
arguments exit `2`. Input-open and image-read failures precede lifecycle effects
and report diagnostics on stderr. A failed receipt write attempts to preserve
the known lifecycle result on stderr and still exits `1`; if both streams fail,
the caller receives no receipt. Never infer absence of effects from a nonzero
exit. [Backup and restore operations](../operations/README.md#logical-backup-and-fresh-restore)
define the data, validation, synchronization and retained-stage behavior.

## marrow apply

`marrow apply` verifies explicit OLD and NEW image artifacts without capturing a
project, compiling source or minting identities. OLD must match the store's exact
active binding. The operation preserves every old durable representation and
physical address and permits new sparse scalar fields beneath existing supported
structure. Existing values remain in place; new fields start absent. Changed
keys, old field requiredness or value representation, new roots, groups, branches
or indexes, and required-field additions are refused as `store.apply_unsupported`.

The store retains its standing authority ceiling. If NEW demands additional
authority, apply proposes exactly the union of that demand and the standing
ceiling. `--accept-ceiling` must name this union, which can differ from NEW's
image ceiling. Missing required acceptance or any incorrect supplied ID returns
`store.ceiling_unaccepted` with the old and proposed IDs and named added effects.
Without an expansion, apply can activate immediately.

Apply holds one read-only store owner through exact OLD admission, logical
inspection and metadata publication. It writes no data cells and returns no
service. Apply never opens the store for writing, so the next writable attach
performs the ordinary physical check. Pending activation blocks ordinary access.
Recovery uses the explicit image matching the Head actually present; it does not
replay apply.

JSONL output has `kind: "apply"`. Success has `outcome: "applied"`, `instance`,
`old_image`, `new_image`, `old_ceiling` and `ceiling`. Failure has `code` and an
`outcome` of `refused`, `metadata_failed` or `activation_uncertain`. Ceiling
refusal adds `old_ceiling`, proposed `ceiling` and `added_effects` containing
`export`, `effect` and nullable `place`. Activation uncertainty retains `instance`.
Default text output displays these same fields.

Success exits `0`; companion or output-delivery failure exits `1`. Arguments
rejected by the CLI parser exit `2`; companion validation, including a malformed
ceiling ID, exits `1` through the CLI. A metadata failure may have changed persistent metadata.
Failed output writes or flushes attempt a diagnostic fallback with the known
result. A missing receipt or nonzero exit does not establish rollback or permit
automatic replay. Logical inspection does not verify physical checksums or
discharge an inherited physical-audit obligation.

## marrow recover

`marrow recover --store <dir> [--image <image>] [--format text | jsonl]` delegates
to the verified companion. With `--image`, it verifies the selected artifact
without project capture. Otherwise it compiles the current project without
minting identities.
The program must match the exact stored image. Recovery validates physical and
logical integrity and establishes fresh activation barriers; it runs no export
and replays no missing head update
([recovering a store](../operations/README.md#recovering-a-store)).

The default output is text. JSONL output contains one record with `kind` set to
`recovery`, the requested `store` spelling, and a `preserved` array of names moved
by this attempt. Success adds `outcome: "activated"`, `instance`, and `image`.
Failure adds `outcome: "error"` and `code`; logical-integrity findings and final
activation uncertainty also include `instance`. Preservation moves can accompany failure.

Success exits `0`; recovery or output-delivery failure exits `1`; invalid
arguments exit `2`. Compilation and installation failures are reported on stderr.
Failed delivery does not undo activation or permit reconstruction of a lost
receipt. Unlike `doctor`, recovery is not read-only.

## marrow image

`marrow image` compiles and verifies the project and writes `program.image`, the
artifact a deployment ships, into `--out <dir>`. The image's demand is its
deployment ceiling, and `--accept-ceiling` names that ceiling's id. Without the
right id, the command prints the id and the demand and writes nothing:

```text
$ marrow image --out img
cli.ceiling_unaccepted: this image's deployment ceiling id is b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691; re-run with --accept-ceiling b618d4d44afcb0eb4045c437267eba85c8b41ffd946fd1dc1b67a62ee54ba691 to compose the deployment image after reviewing the demand printed below
docs.cli.shelf.greet reads or writes no durable data
docs.cli.shelf.half reads or writes no durable data
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
| `1` | A diagnostic, fault, or operational error was reported, `run` returned a top-level `err`, or `doctor` found something wrong with the store. |
| `2` | The command line was wrong: a bare `marrow`, an unknown command or export, a bad flag or argument, or a filter that matches nothing. |
