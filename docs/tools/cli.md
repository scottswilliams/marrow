# CLI Reference

The installed command is `marrow`. A bare invocation and an unknown command are
usage failures (exit `2`). `marrow --help` prints the syntax implemented by the
current binary; `marrow --version` prints the package version.

The beta line's CLI is deliberately thin. `init`, `fmt`, `check`, `run`, `test`,
`import`, `client typescript`, `image`, `lsp`, `--help`, and `--version` are the
available commands; every other recognized
command name belongs to a capability being refounded and reports the typed code
`cli.command_unsupported` with exit `1`, so a script never mistakes absence for
success. [Project status](../status.md) states what returns through which
direction.

## Command index

| Command | Behavior today |
|---|---|
| `init` | Create a new project directory (this page). |
| `fmt` | Format a `.mw` file or every captured source file in a project (this page). |
| `check` | Check a project and describe each export's durable access demand (this page). |
| `run` | Compile, verify, and run an exported function, storeless or against a provisioned store (this page). |
| `test` | Discover and run `test` declarations (this page; see [tests](tests.md)). |
| `import` | Populate and provision a native store from a flat-scalar JSONL corpus (this page). |
| `client typescript` | Generate the strict TypeScript client and the pinned Node supervision module (this page; see [TypeScript client](typescript-client.md)). |
| `image` | Emit the verified program image a deployment ships, against an accepted ceiling id (this page). |
| `lsp` | Run the language server over stdio (this page; see [language server](lsp.md)). |
| `data`, `doctor`, `evolve`, `serve`, `backup`, `restore` | Recognized; report `cli.command_unsupported` until their refounding lanes land. |

## `marrow init`

```text
marrow init <projectdir>
```

Creates a new [project](projects.md): a `marrow.toml` manifest declaring the
current edition and a contained `src` tree with a headerless `src/main.mw`
script. The target directory must not already exist — `init` claims it with an
exclusive create, so two concurrent runs cannot both win, and an existing
directory reports `config.invalid`. `init` creates no store.

## `marrow fmt`

```text
marrow fmt [--check | --write] <file.mw | projectdir>
```

Formats Marrow source to canonical layout through the retained formatter. The
target is either a single `.mw` file or a [project](projects.md) directory, in
which case every captured source file is formatted through the project input.

With no flag on a single file, the formatted source is printed to stdout; on a
project directory, no flag checks without writing. `--check` leaves files
unchanged and exits nonzero when any is not canonical; `--write` replaces changed
source in place through a temporary file that preserves the original permissions.
`marrow fmt` does not read from stdin.

Source that does not parse is left untouched and reported with located
`parse.syntax` diagnostics. Formatting that would drop a retained comment is
refused with `fmt.comment_loss` rather than published lossily. A directory whose
manifest or source tree is invalid reports the matching `config.invalid` or
`project.*` code.

## `marrow check`

```text
marrow check [projectdir]
```

Captures and checks the [project](projects.md) at `projectdir` (the working
directory by default) and reports every diagnostic, each with its source file and
1-based line and column. `check` opens no store and runs no code. Diagnostics are
written to standard error; the command exits `0` when the project checks clean, `1`
when any diagnostic is reported or a fixed compiler bound is reached, and `2` on a
usage error.

A project that checks clean is compiled and independently verified, and `check`
prints one line per exported (`pub fn`) function to standard output, in
`module.item` order, describing that export's durable
[access demand](../language/durable-places.md#access-demand) in source spelling:

```text
bookstore.lookup reads ^books and ^books.byIsbn
bookstore.put reads ^books; writes ^books
```

The demand is the set of durable places the export's whole call graph touches,
named by their durable paths (`^root`, `^root.field`, `^root.index`) and grouped by
whether the export reads or writes each. A presence probe, a field or entry read,
and an ordered index or family traversal are reads; a write and an erase are writes;
a place a read-modify-write export both reads and writes is named in both clauses.
The demand *describes* the access a program requires and never grants it. An export
that touches no durable data is reported as reading or writing no durable data.

## `marrow run`

```text
marrow run <export> [--store <dir>] [--format text | jsonl] [-- <args>...]
```

Runs one exported (`pub fn`) function of the [project](projects.md) at the
working directory. The project is captured, compiled to a reproducible program
image, and independently verified into a sealed image before the VM runs the
export; the compiler opens no store and cannot mint a verified image. Arguments
after `--` are decoded positionally against the export's scalar parameter types
(`int`, `bool`, `string`).

A storeless export runs directly. A durable export — one whose verified demand
reads or writes durable data — runs against a provisioned store named with
`--store <dir>`: the terminal never opens the store itself but runs the verified
image in a release-verified companion runner attached to the store, submits the
call, and renders the result. The companion runner and its release manifest must be
installed beside `marrow` (the stock install layout); a missing or altered
companion is reported as `cli.installation_damaged`. A durable export run **without**
`--store` has no store to act on and reports the typed `cli.durable_unsupported`
outcome (exit `1`). Durable execution also runs under source tests against a fresh
ephemeral attachment, needing no store or companion (see [`marrow test`](#marrow-test)).

When a fresh durable declaration has no identity in the project's
[identity ledger](projects.md#the-identity-ledger), `run` still mints one from OS
entropy and publishes the updated `marrow.ids` atomically before compiling
again — commit that file — and then parks the durable export. Because an identity
is durable once minted, `run` mints and persists it even when the program still
has unrelated errors: the mint is not gated on an otherwise-clean compile, and
the recompile then reports whatever genuinely remains. This convenience is
`run`-only (every other command fails precisely with `check.durable_identity`)
and is superseded by the accepted change-review `apply` action when that lane
lands; a retired identity is never minted over.

Output is text by default — the returned value, or `absent` for a vacant
optional. `--format jsonl` prints one canonical JSON object: an outcome of
`value`, `diagnostic`, `artifact_rejected`, `fault`, or `error`, keeping the
four failure families distinct. A source diagnostic (`check.*`, `parse.*`), an
image rejection (`image.*`), a source-mapped runtime fault (`run.*`), and an
operational error (`store.*`, `io.*`) never collapse into one another.

Exit `0` carries the value; exit `1` is any failure family (including a durable
export parked in the trough); exit `2` is a usage error (an unknown export or a
bad argument).

## `marrow test`

```text
marrow test [--format text | jsonl] [--filter <substring>]
```

Discovers every `test "name"` declaration in the [project](projects.md) at the
working directory, compiles them into a separately verified image carrying a
closed test-entry table, and runs each one through the VM. A test
whose every `assert` holds passes; a false `assert` (`run.assert`) fails it; any
other runtime fault errors it. A test that touches no durable data runs storeless;
a test that reads or writes durable data runs against its own fresh ephemeral
attachment, so no test opens a persistent store or observes another test's writes.

`--filter` selects tests whose name contains the given substring and fails when
none match. Output is human text by default — one line per test and a summary —
or, with `--format jsonl`, one `kind: "test"` object per test and a final
`kind: "summary"` object. The command exits `0` when every selected test passes,
`1` when any fails or errors, and `2` on a usage error. See
[Tests](tests.md) for the report grammar and the `test`/`assert` language.

## `marrow import`

```text
marrow import --store <dir> --jsonl <path> --root <name> [--keys <col,...>]
```

Populates a native store from a flat-scalar JSONL corpus through the trusted
importer, provisioning the store on first use. The project at the working directory
is compiled and independently verified first; import is not a mint path, so a
missing durable identity is reported and the developer runs `marrow check` before
importing. Like `marrow run --store`, the terminal never opens the store: it hands
the verified image and the corpus to the release-verified companion runner, which
is the sole opener of the store, so the stock install layout is required.

The corpus is one JSON object per line, each member a scalar (string, integer, or
boolean) named exactly as a key column (`--keys`) or a field of the target
`--root`; a member may be `null` or absent for a sparse field. Every row is created
through the path kernel — no command receives a raw storage key, engine handle, or
transaction object — and a store that denies writes refuses the import. The command
reports the store provisioning (on first use) and a final `rows_imported` count;
input is read and committed in bounded batches rather than materialized whole.

## `marrow client`

```text
marrow client typescript [--out <dir>]
```

Compiles and verifies the [project](projects.md) at the working directory,
reconstructs its wire interface from the verified image, and writes three files
into the output directory (default `client`): the generated `client.mts` — one
named `async` method per exported function with exact transfer types and
runtime validation — and the pinned Node supervision module
(`marrow-supervisor.mjs` plus its type declarations). Stable inputs yield
byte-identical output. The wire transfer graph is closed over every value type
(the seven scalars, records, sums, `List`, `Map`, and entry identities), so a
verified program's interface always projects; a signature too complex for the
fixed interface budget or naming an unknown type row refuses the whole generation
with `cli.interface_unbuildable`. Unlike `run`, the generator never mints durable
identities. See
[TypeScript client](typescript-client.md) for the generated API, the
supervision law, and the loss classification.

## `marrow image`

```text
marrow image --out <dir> --accept-ceiling <id>
```

Compiles and independently verifies the [project](projects.md) at the working
directory and writes the verified `program.image` into the output directory — the
durable artifact a packaged application's deployment pins beside its
release-verified runner. Unlike `run`, `image` never mints durable identities and
opens no store.

An image's exports have a durable **demand**, and the store an application
provisions under the image records the union of that demand as the maximum authority
it will ever admit — its deployment ceiling (the `marrow check` section above
describes the demand it prints, and the ceiling identity). So the
command renders each export's demand and requires the owner to name the accepted
ceiling id: `--accept-ceiling` must equal the image's own demand-union ceiling id
before any image is written. When the argument is absent or names a different id, no
image is written and the command reports `cli.ceiling_unaccepted` with the actual
ceiling id to accept, so a deployment's durable authority is named deliberately and
never widened or narrowed by accident. Stable inputs yield a byte-identical image.

## `marrow lsp`

```text
marrow lsp
```

Runs the [language server](lsp.md) over standard input and output, speaking
JSON-RPC 2.0 with Language Server Protocol framing. The server captures and
analyzes the project at the client-selected workspace root and serves
diagnostics, whole-document formatting, hover, and go-to-definition from the
compiler's published analysis facts. It takes no arguments and is normally
launched by an editor, not run by hand; it reads and writes the protocol stream,
never ordinary terminal text. The command does not open a store. See
[language server](lsp.md) for the served capabilities and the protocol contract.

## Usage and exit codes

Flags that take values use separate arguments, such as `--check`; the CLI does
not accept `--flag=value` forms. Dotted diagnostic codes are defined in the
[Error Code Reference](../error-codes.md).

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Typed failure: a diagnostic was reported, or the command is not yet available on this line. |
| `2` | Command-line usage failed before the command body ran. |
