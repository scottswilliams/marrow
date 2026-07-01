# CLI Reference

The `marrow` binary is the single entry point for the language and its built-in
database.

```
marrow init [--client] <projectdir>
marrow check [--format text|json|jsonl] [--locked] <projectdir>
marrow doctor [--format text|json|jsonl] <projectdir>
marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance] [--approve-retire <field-path>:<count>] \
  [--backup <path> | --no-backup] [--format text|json|jsonl] <projectdir>
marrow fmt [--check | --write] <file.mw | projectdir>
marrow run [--entry <entry>] [--arg name=value]... [--maintenance] \
  [--trace] [--dry-run] [--format text|json] <projectdir>
marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>
marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
marrow serve --remote --addr <addr> [--write] \
  (--auth-token-env NAME | --auth-token-file PATH) \
  [--cursor-token-key-id <kid> (--cursor-token-key-env NAME | --cursor-token-key-file PATH)] \
  [--remote-cors-origin <origin>] <projectdir>
marrow client typescript [--cursor-token] [--out <path>] <projectdir>
marrow data <roots|stats|dump|integrity> [--backup <artifact>] [--format text|json|jsonl] <projectdir>
marrow data recover [--format text|json|jsonl] <projectdir>
marrow data get [--backup <artifact>] [--format text|json|jsonl] <projectdir> <path>
marrow backup <projectdir> <output-file>
marrow restore [--replace --count N] <projectdir> <backup-file>
marrow --version
marrow --help
```

A project directory contains a `marrow.json`; see
[project-config.md](project-config.md) for its fields. Every subcommand accepts
`--help` (or `-h`) and prints its own usage.

Common starting points:

```console
$ marrow check .
$ marrow run .
$ marrow serve .
$ marrow serve --write --cors-origin http://localhost:5173 .
$ MARROW_SURFACE_TOKEN=secret marrow serve --remote --addr 0.0.0.0:8080 --auth-token-env MARROW_SURFACE_TOKEN .
$ marrow client typescript .
$ marrow client typescript --cursor-token --out remote-client.ts .
```

Use `marrow check` before deployment or generation, `marrow run` for entry
execution, `marrow serve` to run the local project API, and
`marrow client typescript` when a browser or Node tool needs the matching
TypeScript client.

## Version

```
marrow --version
```

Print the CLI version and the storage engine profile tuple this binary writes:

```console
$ marrow --version
marrow 0.1.0 engine-profile=(key=v0, layout-epoch=0, digest=77944eb86c08b665)
```

The tuple names the key profile version, layout epoch, and engine-profile
digest. It is the same profile used by activation fencing and commit stamps.

## Exit Codes

| Code | Meaning |
|---:|---|
| `0` | The command completed successfully. |
| `1` | A recoverable failure: parse/check diagnostics, a failing test, a runtime or storage error, a project or tooling failure. |
| `2` | A command-line usage error, detected before the command body ran: an unknown subcommand or flag, a missing or duplicated argument, or an invalid flag value. |

See [error-codes.md](error-codes.md) for the dotted error codes and the
machine-readable error envelope these commands emit.

## Output Formats

Commands that report diagnostics, saved data, or test results take `--format`:

- `text` (the default) — human-readable lines. Diagnostics and findings go to
  stderr unless a command owns a report stream such as `doctor`; primary
  results go to stdout.
- `json` — one JSON object for the command's structured report.
- `jsonl` — one JSON object per line for streaming reports, ending with a
  `{"kind": "summary", …}` line where the report has many records.

Plain `run` text output is the program's own `print` stream on stdout. With
`run --format json`, stdout becomes a result envelope that carries the captured
program output separately from the rendered return value. `run --dry-run`
accepts `--format text|json` for its tooling report, written to stderr.
`run --trace` is text-only and does not accept an explicit `--format` unless it
is combined with `--dry-run`.

`marrow test --format json|jsonl` shapes the test pass/fail report on stdout.
With `--trace`, the trace is a separate text stream on stderr while the test
report stays on stdout.

Structured JSON reports that include a `project` field render the canonical
absolute project directory, equivalent to `std::fs::canonicalize(<projectdir>)`,
not the raw directory argument.

---

## `marrow init`

```
marrow init [--client] <projectdir>
```

Create a new project directory with the quickstart scaffold: `marrow.json`,
`src/<name>/books.mw`, and `tests/books_test.mw`, where `<name>` is the target
directory's final path component.

The target directory must not already exist. Its final path component must parse
as one Marrow module identifier segment, because the scaffold uses it in
`run.defaultEntry`, `module <name>::books`, and `use <name>::books`.

The generated config is explicit: `sourceRoots` is `["src"]`,
`run.defaultEntry` is `<name>::books::main`, the store is
`{"backend":"native","dataDir":".marrow/data"}`, and `tests` is `["tests"]`.
No `.gitignore` or extra project files are created.

`--client` (short `-c`) opts into a surface-bearing scaffold: the generated
source adds a minimal `surface` over the `^books` store, and the config gains a
`"client": "generated/marrow.ts"` line, so the first `marrow run` emits the
declared TypeScript client. Bare `marrow init` is unchanged — store-only, with
no `surface` and no `client`. The flag is named after the client because a
`client` path applies only to a surface-bearing project, so the two are
scaffolded together or not at all.

Exits `0` after writing the scaffold, `1` if the target name is invalid or the
target cannot be written safely, and `2` for usage errors.

```console
$ marrow init shelf
created shelf
next steps:
  cd shelf
  marrow run .    # run the project and write its store and marrow.lock

$ cd shelf
$ marrow check .
ok: . checked
```

---

## `marrow check`

```
marrow check [--format text|json|jsonl] [--locked] <projectdir>
```

Check a project directory containing `marrow.json` and report diagnostics.

- It loads `marrow.json` and runs the project checker over every source root
  plus configured test files: parse, type, effect, and durable-place checks. It
  binds saved-data identity from the live store when one is present and readable,
  falling back to the committed `marrow.lock` projection otherwise; the read is
  read-only, so check never opens the store for repair, creates one, or writes
  the source tree. With `--locked`, a `marrow.lock` whose recorded source shape is
  behind the current source is a fatal `check.stale_lock` error for CI, and an
  entirely absent `marrow.lock` over a project that has durable shape to lock (a
  stamped store / accepted catalog) is a fatal `check.lock_missing` error so a
  forgotten or deleted lock cannot pass the gate; the structured envelope reports
  `status: "failed"` with the diagnostic and omits the success-only sections, so
  the envelope agrees with the exit code. By default a stale lock is a non-fatal
  advisory on stderr — the envelope stays `status: "ok"` — since a later `run` or
  `evolve apply` regenerates the lock; an absent lock on a legitimate first run,
  which has no durable shape to lock yet, raises no condition under `--locked`.
- The same gate covers the declared TypeScript client. When the project declares
  a callable surface and a `client` output path, and that file is absent or
  carries a different `marrow-client-digest` than the current generator profile
  and surface, `--locked`
  fails with `check.stale_client` for CI, and plain `check` advises on stderr and
  exits `0`. `check` never writes the client — a later `run`, `serve` startup, or
  `evolve apply` rewrites it. A project with no `client`, or no surface, raises no
  client condition.
- Passing a bare `.mw` file is a usage error. Run `marrow check` on the project
  directory that contains `marrow.json`.
- When `marrow.json` sets `run.defaultEntry`, the check verifies it names a
  runnable zero-argument entry. A missing, private, ambiguous, or parameterized
  default entry is a `check.default_entry` error rather than a run-time fault.
  `marrow doctor` inherits this check.
- On successful `json` or `jsonl` checks, the report includes
  `entry_footprints`, `surface_abi`, and `surface_routes`. These appear only on
  a successful check; a failing check omits them.
- `entry_footprints` is an array with one record per public entry, each carrying:
  - `entry`: the qualified entry name (`module::function`).
  - `write_effects_reachable`: `true` when a saved write is reachable through the
    entry's call graph.
  - `stores_read`, `stores_written`: the stores the entry reads or writes,
    identified by structural path `module::^root` (bare `^root` for an
    empty-module script).
  - `indexes_touched`: the store indexes the entry touches, identified by
    structural path `module::^root::index`.
  - `work_shape`: one of `compute_only`, `read_only`, or `writes_saved_data`.
  Store and index identities are these structural paths, not physical catalog
  ids: a path is deterministic at every check, even before a freeze assigns a
  `cat_*` id, and joins to the committed `marrow.lock` entry by its `path` field
  for callers that need the physical key.
- `surface_routes` is
  the `surface.route.v1` manifest derived from exported surface descriptors:
  JSON `POST` operation-tag paths plus render aliases and request-body kinds.
  The manifest is data; `marrow serve` is the serving profile that consumes it,
  and `marrow client typescript` renders a thin TypeScript operation-envelope
  client from it. Remote opaque cursor tokens are a serve/client profile over
  the same typed cursor DTOs, not part of the route manifest.

Exits `0` when there are no errors, `1` when there are diagnostics or
`marrow.json` cannot be read, and `2` for usage errors such as a non-directory
target.

```console
$ marrow check ./proj
ok: ./proj checked

$ marrow check --format json ./proj
{"project":"/absolute/path/to/proj","status":"failed","diagnostics":[{"code":"parse.syntax", …}]}
```

A failing check returns exit `1`:

```console
$ marrow check ./proj
./proj/src/broken.mw:1:1: error: parse.syntax: expected function parameter list
$ echo $?
1
```

---

## `marrow client typescript`

```
marrow client typescript [--cursor-token] [--out <path>] <projectdir>
```

Generate a self-contained TypeScript client for the checked application surface
operation envelope, exporting a `createClient` factory. The command runs the same
read-only project analysis used by `marrow check`, binding saved-data identity
from the committed `marrow.lock` projection; it does not open, create, repair, or
mutate the saved-data store.

This command is the explicit escape hatch. The usual lifecycle is automatic:
when `marrow.json` declares a `client` path, `marrow run`, `marrow serve`
startup, and `marrow evolve apply` rewrite that file write-if-changed on a
surface-ABI change, and `marrow check --locked` keeps it honest in CI. The
developer never has to run a separate codegen step for the declared output.

- With `--out <path>`, the rendered client is written to that file and nothing
  is echoed to stdout; the resolved path is reported on stderr. A relative path
  resolves against the current working directory, the POSIX CLI convention; an
  absolute path is honored as given. Missing parent directories are created.
- `--cursor-token` renders the remote cursor-token client profile. Page cursor
  brands are opaque strings for a remote server started with cursor-token mode,
  the header profile is `typescript.v2+surface.cursor_token.v1`, and the client
  digest is distinct from the default typed-cursor client. It does not refresh a
  declared `marrow.json` `client` path unless `--out` names a destination.
- Without `--out`, when `marrow.json` declares a `client` path, that declared
  file is refreshed write-if-changed — the same path `run`, `serve`, and
  `evolve apply` keep current — and the outcome (wrote, updated, or unchanged)
  is reported on stderr; nothing is echoed to stdout.
- Without `--out` and with no declared `client`, a successful check prints
  TypeScript to stdout and diagnostics nowhere.
- When the committed `marrow.lock` is behind the current source (the
  `check.stale_lock` condition), the generated client may not reflect the
  accepted catalog, so the command warns on stderr and names the `marrow run`
  that re-projects the lock; generation still exits `0`.
- A failed check reports the existing text diagnostics to stderr, exits `1`,
  and prints no partial client.
- Usage errors, including a missing project directory or unknown option, exit
  `2`.
- Every generated file begins with a do-not-edit header,
  `// marrow-client-profile: typescript.v2`,
  `// marrow-surface-digest: sha256:<hex>`, and
  `// marrow-client-digest: sha256:<hex>` lines. The surface digest is the
  ABI/route identity; the client digest is the deterministic freshness key the
  declared-output lifecycle and `check --locked` compare against the current
  generator profile and surface. The explicit cursor-token profile changes the
  profile line and digest without changing the surface digest.
- The generated client uses the exported `surface_abi` descriptors and
  `surface.route.v1` manifest as inputs. It validates route/ABI agreement before
  rendering, stores operation tags and route paths as constants in method bodies,
  serializes `surface.operation.v1` request envelopes, rejects unsafe JavaScript
  `number` inputs for Marrow `int` leaves, and validates only the response
  envelope profile, operation tag, and result kind before returning the
  server-owned JSON result payload.
- Each generated paged read keeps its explicit page method and also gets a
  `<methodName>Pages(...)` async iterable helper. The helper takes the same exact
  index-key arguments followed by `{ limit, initialCursor? }`, advances cursors
  between requests, and yields `Page<Row, Cursor>` values rather than rows.

The generated TypeScript is convenience code, not an authority boundary. HTTP
serving and linked-Rust execution still revalidate operation tags, request-body
kinds, catalog IDs, identity brands, value shapes, and integer forms for callers
that bypass the generated client.

---

## `marrow serve`

```
marrow serve [--write] [--watch] [--cors-origin <loopback-origin>] [--addr <loopback:port>] <projectdir>
marrow serve --remote --addr <addr> [--write] \
  (--auth-token-env NAME | --auth-token-file PATH) \
  [--cursor-token-key-id <kid> (--cursor-token-key-env NAME | --cursor-token-key-file PATH)] \
  [--remote-cors-origin <origin>] <projectdir>
```

Run the HTTP serving profile for checked application surfaces. By default the
command binds loopback only, opens the project through `ProjectSurfaceReadSession`,
and serves v1 read routes, including computed reads, plus v2 ranged index page
read routes. With `--write`, it opens `ProjectSurfaceSession` and also exposes
v1 create, sparse-update, delete, and action routes. Both modes require an
already accepted native store and never create, freeze, migrate, repair, or
auto-apply saved data.

- Without `--remote`, the listener binds only loopback addresses. The default is
  `127.0.0.1:8080`; tests and tooling can pass `--addr 127.0.0.1:0` to let the
  OS choose a loopback port.
- `--cors-origin` enables browser CORS for one exact loopback origin such as
  `http://localhost:5173`, `http://127.0.0.1:5173`, or
  `http://[::1]:5173`. Non-loopback origins, URL paths, and wildcards are
  usage errors. Without this option, the server emits no CORS headers.
- `--remote` allows a non-loopback bind, but requires explicit `--addr` and
  exactly one auth source: `--auth-token-env NAME` or `--auth-token-file PATH`.
  `--watch` is refused in the remote profile. The token is one UTF-8 line after
  removing one trailing LF or CRLF; empty tokens and other leading or trailing
  whitespace are usage errors. Token files must be regular files.
- Remote requests must carry exactly one `Authorization` header whose value is
  `Bearer ` followed by the configured token, with one space and case-sensitive
  scheme. Missing, malformed, duplicate, or wrong auth returns HTTP `401` with
  code `surface.auth` before the request body is read.
- Remote serve can enable opaque page cursor tokens with
  `--cursor-token-key-id <kid>` and exactly one key source:
  `--cursor-token-key-env NAME` or `--cursor-token-key-file PATH`. Any
  cursor-token flag without `--remote` is a usage error. The key id is 1-32
  characters from `[A-Za-z0-9_-]`. The key source is one UTF-8 line after
  removing one trailing LF or CRLF, with no other leading or trailing
  whitespace, and must be unpadded base64url for exactly 32 bytes. Key values
  and issued cursor tokens are never printed.
- In cursor-token mode, page responses return `page.next` as
  `mct1.<kid>.<nonce>.<ciphertext>` instead of the typed cursor object.
  Follow-up page requests must send that string as `cursor`; `null` or an
  omitted cursor starts a page stream. Typed cursor objects are rejected as
  `surface.cursor`. Malformed, tampered, wrong-key, or authenticated-context
  mismatches are also `surface.cursor`; stale typed cursor lineage after a
  successful decrypt remains `surface.stale_cursor`.
- `--remote-cors-origin` is separate from local `--cors-origin`, requires
  `--remote`, and accepts one exact `http` or `https` origin. It rejects
  wildcards, `null`, paths, queries, and fragments. Remote preflight is
  unauthenticated only for the configured origin, rejects duplicate CORS request
  headers, and accepts requested headers exactly `Content-Type, Authorization`
  case-insensitively.
- `marrow serve` does not terminate TLS. Do not expose the remote profile over
  plain HTTP except behind a trusted TLS proxy or on a trusted private network.
- On startup the command prints
  `serve listening on http://<addr>` to stdout, then handles requests until the
  process exits.
- Each request emits one access-log line to stderr carrying the method, request
  path, HTTP status, latency in milliseconds, and the resolved operation tag
  (`op=-` when no surface route matched). The line never contains request or
  response payloads, so logs are safe to retain.
- `GET /health` reports store and catalog readiness for orchestration probes. It
  is unauthenticated and emits no CORS headers so a load balancer can poll it
  directly. It returns `200 {"status":"ready"}` while a surface session is held,
  and `503 {"status":"unavailable"}` during the brief `--watch` re-check window
  or after a re-check fails. Any method other than `GET` returns `405`.
- The active route set is derived from descriptor route manifests. Default mode
  serves `/surface/v1/read/<operation-tag>` rows, including computed reads, and
  `/surface/v2/read/<operation-tag>` ranged index page rows that require
  `surface.operation.v2` envelopes; `--write` additionally serves
  `/surface/v1/create/<operation-tag>`,
  `/surface/v1/update/<operation-tag>` and
  `/surface/v1/delete/<operation-tag>`, and
  `/surface/v1/action/<operation-tag>` rows.
- `--write` is single-owner and sequential through the native writer lock while
  the process is running. It excludes another writer and read-only inspection
  handle for the same store file.
- At startup, when `marrow.json` declares a `client` path over a surface-bearing
  project, the command rewrites that declared client write-if-changed before it
  binds the listener, so the served surface and the on-disk client agree.
- `--watch` keeps that client current while serving. Between connections the
  command polls the source-root `.mw` files for a modification-time change; on
  one it re-checks the project, rewrites the declared client write-if-changed, and
  resumes serving the fresh surface, holding the last good surface if a re-check
  transiently fails. The poll is dependency-free; `serve` adds no watcher crate.
- Served actions run with zero host capabilities. Actions that require clock,
  environment, logging, filesystem, or other host capabilities fail closed as
  `surface.action`; explicit-host action execution is a linked-Rust embedding
  API, not this HTTP profile.
- Served computed reads are public read-only functions checked to have no writes,
  transactions, host effects, throws, or unindexed collection reads. Argument
  decode failures use `surface.request`; execution or result-rendering failures
  use `surface.computed`.
- Operation requests must be HTTP/1.0 or HTTP/1.1 `POST` with
  `Content-Type: application/json`, exactly one `Content-Length`, no
  `Transfer-Encoding`, bounded headers/body, no query string, and an exact
  operation-tag path. The JSON body uses the selected route's operation envelope
  profile; `profile_version`, `operation_tag`, and request kind must match that
  route. Unknown fields in the operation envelope or surface-owned request DTOs
  are rejected as `surface.request`.
- With `--cors-origin`, matching browser preflight `OPTIONS` requests over a
  served route return `204` and `Access-Control-Allow-Origin` for that exact
  origin. Mismatched origins return `403` and no CORS allow-origin header.
- Browser clients use the generated client's existing `headers` option for
  remote auth:

  ```ts
  createClient({ baseUrl, headers: { Authorization: `Bearer ${token}` } })
  ```

- The server processes at most one request per connection, rejects trailing
  bytes already buffered after the declared body, returns `Connection: close`,
  and never reads a second request from the connection.
- Responses are JSON. Success returns the operation response envelope. Failures
  return a sanitized `{ "code": "surface.*", "message": "..." }` envelope with
  no source path, store path, or raw backend detail.

Exits `2` for usage errors such as missing remote auth, invalid cursor-token
key configuration, non-loopback local `--addr`, or invalid CORS origins, `1`
for project/session/listener failures, and otherwise runs until killed.

---

## `marrow doctor`

```
marrow doctor [--format text|json|jsonl] <projectdir>
```

Inspect a project for operator triage without repairing or writing anything.
`doctor` aggregates independent probes where possible:

- load `marrow.json`;
- validate the committed `marrow.lock` projection and report a corrupt, stale,
  or missing lock, or a lock that collides with the live store, as findings; a
  lock absent over a store carrying saved shape is `doctor.lock_missing`,
  mirroring `check.lock_missing`, while a true first run stays healthy;
- run the normal project check summary;
- open the configured native store read-only when a store file exists;
- report store lock/recovery/open failures as findings instead of stopping
  unrelated probes;
- read the store UID, commit stamp, current engine profile tuple, and activation
  fence classification;
- sample saved-data integrity with
  `DOCTOR_INTEGRITY_SAMPLE_LIMIT = 64` as one shared traversal cap.

The live store is always the authority. When the committed `marrow.lock` and the
store disagree — a stale lock, or the same epoch with a different shape — `doctor`
reports the collision and names the regenerate step; the store wins and `doctor`
repairs nothing. It never creates the native data directory, never opens a
write-capable store handle, never regenerates `marrow.lock`, and never runs the
full unbounded `marrow data integrity` scan.

Text and JSONL render one finding per line. Text output also prints a
non-finding guidance line when the integrity sample is truncated, naming the
full read-only `marrow data integrity` command to run next. JSON renders one
envelope:

```json
{
  "project": "/absolute/path/to/proj",
  "status": "findings",
  "findings": [
    {
      "code": "doctor.store_locked",
      "kind": "tooling",
      "message": "native store is locked",
      "remedy": "close the process holding the native store, then rerun the next command",
      "next_command": "marrow doctor ./proj",
      "data": {
        "underlying_code": "store.locked",
        "message": "the store file is held open by another process (a writer or a read-only inspection): /absolute/path/to/proj/.marrow/data/marrow.redb. Close the other process, then retry",
        "store": "/absolute/path/to/proj/.marrow/data/marrow.redb"
      },
      "source_span": null
    }
  ],
  "store": null,
  "fence": null,
  "integrity_sample": { "limit": 64, "items_checked": 0, "problems": 0, "truncated": false }
}
```

When the store opens, the JSON `store` object carries the stamp classification
(`stamped` or `unstamped`), store UID, commit metadata, and current engine
profile tuple. When the checked project and store are both available, `fence`
reports the activation-fence classification.

Exits `0` when no findings are reported, `1` when one or more findings are
reported, and `2` for usage errors.

---

## `marrow evolve`

```
marrow evolve preview [--from-backup <artifact>] [--scaffold] [--format text|json|jsonl] <projectdir>
marrow evolve apply [--maintenance] [--approve-retire <field-path>:<count>] \
  [--backup <path> | --no-backup] [--format text|json|jsonl] <projectdir>
```

`evolve preview` opens the configured store read-only, discharges source,
accepted catalog metadata, store snapshot, and engine metadata into an exact
witness, then reports the counts and blocking diagnostics. With
`--from-backup <artifact>`, preview validates the backup artifact, mounts it in
memory, and derives the witness from that point-in-time data instead of opening
the configured store; the mount is read-only and does not restore, activate, or
write to the store or lock. With `--scaffold`, text output is formatter-produced `.mw`
source containing one `evolve` block per repairable obligation, each naming its
target in the source form the checker resolves (`Book.pages` for a member,
`Status::archived` for an enum value). A newly-required member gets a ready-to-paste
`default` body with a type-correct constant of its leaf type; a bare same-shape
rename — a resource member or an enum member moved with a single plausible
candidate — gets the identity-preserving `rename` block, not a destructive drop;
and a populated leaf retype — which cannot be reinterpreted in place without losing
data — gets a commented migration skeleton that points at adding a member of the new
type, transforming it from the old member, then retiring the old member, never a
runnable in-place transform. An orphaned value with no single rename candidate — a
removed enum member, for instance — synthesizes no block, since its repair is
record-specific source the author must write, and the paste-the-block footer is then
omitted rather than naming a block that does not exist. It never edits project
source. JSON and JSONL keep the preview envelope and include the scaffold string.

`evolve apply` recomputes that preview witness over the live project and store,
requires an exact match, checks the activation window, and commits the data work
plus metadata stamp in one transaction. Like `run`, it records a project's
baseline saved-data identity first when the project has none yet, then applies the
evolution. The advanced accepted shape commits in that same store transaction as
the data work and the slim commit stamp, so the accepted shape never advances
without the data behind it; the live store is the sole write-time authority. After
that commit, the CLI regenerates `marrow.lock` as a one-way projection of the
committed store snapshot, so the committed lock tracks the store and can never
override it. Any Retire-bearing apply also requires either `--backup <path>` or
`--no-backup`: a backup is written through the typed atomic backup path and
validated before apply mutates the store, while `--no-backup` records the explicit
opt-out in the rendered receipt. Evolve refuses backup paths that resolve to
managed project artifacts or subtrees: `marrow.json`, `marrow.lock`, source roots,
test paths, and the native data directory/store file. The command output still
renders receipt counts for defaults, transforms, retires, rebuilt indexes, and
recovery-point choice, but those counts are not persisted in commit metadata.
Destructive retire also needs `--maintenance` and an approval whose catalog ID
and populated count match the preview.

---

## `marrow fmt`

```
marrow fmt [--check | --write] <file.mw | projectdir>
```

Format Marrow source. `marrow fmt` does not read from stdin.

- A single `.mw` file with no flag prints the formatted source to stdout.
- `--check` reports each file that is not already formatted and exits `1` if any
  differ; it writes nothing.
- `--write` rewrites changed files in place. Each changed file is written to an
  adjacent temporary file and replaces the original only after the new content is
  written successfully; a parse or write failure leaves the original file intact.

All three modes agree on losslessness. A comment the formatter cannot re-emit —
one stranded on a continuation line inside an open delimiter — is refused
(`fmt.comment_loss`, exit `1`) in every mode, including the default stdout mode,
which prints nothing rather than emit comment-stripped source. `marrow fmt
file > file` therefore never silently discards content.

Blank lines that group statements or members are kept: between two items in a
body, a single blank line is preserved where the source held one or more, two or
more consecutive blank lines collapse to one, and a leading or trailing blank
inside a body is dropped. A comment that follows a blank line stays its own line
attached to the item below it. Blank lines are layout only; they do not affect a
declaration's durable shape identity.
- A project directory formats every `.mw` file under its source roots, and
  requires `--check` or `--write`. Printing many files to stdout is meaningless,
  so a bare `marrow fmt <dir>` is a usage error (exit `2`).

Source that does not parse is reported and left untouched (exit `1`).

```console
$ marrow fmt src/shelf.mw          # print formatted source
module shelf
…

$ marrow fmt --check ./proj        # exit 1 if anything is unformatted
$ marrow fmt --write ./proj        # rewrite in place
```

Exit codes: `0` formatted/already-formatted; `1` a `--check` file differs, a
format would discard retained comments, or a file failed to parse or write; `2` a
directory with no `--check`/`--write`, an unknown flag, or a `-` stdin argument.

---

## `marrow run`

```
marrow run [--entry <entry>] [--arg name=value]... [--maintenance] \
  [--trace] [--dry-run] [--format text|json] <projectdir>
```

Check a project, then run an entry function over the store its `marrow.json`
selects (see [project-config.md](project-config.md)). A project must check
cleanly before it runs. Omitted `store` and the explicit memory backend admit
only a program with no durable declarations; a program that declares a durable
surface (a `resource`, a saved `store`, or an `enum`) needs a configured
`native` store and otherwise fails the pre-run check with
`check.durable_store_required`.

A clean run records the project's baseline saved-data identity if it has none yet.
The first run of a project with a durable surface freezes its identity into the
store transactionally as it commits. When the local store is empty and a committed
`marrow.lock` exists, the run seeds the store from the lock instead of minting
fresh identity, so a fresh checkout adopts the committed identity exactly; a
corrupt lock refuses the run (`catalog.lock_corrupt`) rather than minting around
it, and fresh identity is minted only when no lock exists. The live store is the
sole write-time authority: after a baseline or evolution commits, the CLI
regenerates `marrow.lock` as a one-way projection of the committed store snapshot.
A project already past its baseline proposes no change and the store is left
untouched; `marrow.lock` can never override or repair a valid live store.
There is no separate acceptance step. See [data-evolution.md](data-evolution.md).

Opening a native store is fenced against its catalog activation stamp. A store
that holds saved records but no activation stamp is refused
(`run.store_unstamped`); run `marrow evolve preview` to inspect the required
work and `marrow evolve apply` to stamp it first. When the source's shape
drifted from the stamped schema, a change that mutates no stored records (such
as adding a sparse field) is auto-applied through the production apply path and
the run proceeds against the advanced catalog; a change that would backfill,
transform, or destructively drop populated data is refused with
`run.schema_drift`, naming the `marrow evolve apply` step that discharges it.

The entry is `--entry` if given, otherwise the project's `run.defaultEntry`.
Qualified entries (`module::function`) resolve exactly. A bare entry name is
accepted only when it names one public function in the checked program; ambiguous
bare names fail with `run.ambiguous_function`. If neither entry source is
present, `run` fails with `run.no_entry` (exit `1`).

`--arg name=value` supplies one entry parameter value. Repeat `--arg` in argv
order. The CLI parser only splits at the first `=`; signature-driven decoding
belongs to the checked entry call. Scalar and enum arguments use the same
textual spellings accepted by runtime literals and checked enum facts. `string`
values are the raw text after the first `=`. Sequence parameters whose element
type is scalar or enum collect repeated values in argv order; `--arg name=[]`
spells an empty sequence. Single-component `Id(^store)` parameters decode
through the same identity-key guards used by saved data. Composite identity
keys, resource-shaped parameters, group entries, local trees, and other
unsupported entry surfaces fail with `run.entry_argument` (exit `1`). There is
no `--args-json`; it is an unknown option and exits `2`.

Output written with `print` goes to stdout. `std::log` output goes to
stderr. The run reads the real system clock, environment, and filesystem.

`--maintenance` grants the run the maintenance capability for data evolution and
repair tooling. It permits whole managed-root deletes and required-field deletes
that the default run rejects. An operator must type it; the default run and
`run.defaultEntry` can never inject it. Use it deliberately.

`--trace` reports each statement as it runs — file, line, call depth, and the
visible locals — and each managed write or delete, in execution order. The trace
is a text-only tooling stream on stderr, leaving the program's stdout for its
own `print` output. Combining `--trace` with any explicit `--format` is a usage
error unless `--dry-run` is also present.

In the human-readable text of a `--trace` or `--dry-run` write, the leaf value is
rendered as its declared typed scalar, not as raw codec bytes: a `bool` reads
`true`/`false`, an int/date/duration/instant reads its canonical typed text. The
machine-readable `value_b64` field in dry-run JSON output stays the raw stored
bytes.

`--format json` on a non-dry run moves the program's `print` stream into the
`output` field of a stdout envelope. The envelope carries `result` as either
`{"kind":"none"}` or `{"kind":"value","value":...}` when the entry's return
value has a JSON surface, an empty `diagnostics` array, and `store_stamp` with
`store_uid`, `catalog_epoch`, and `commit_id` for durable-store runs. A sibling
`committed: true` appears only when this invocation committed a write; read-only
runs omit `committed`. Identity returns carry the store root and saved-key type
tags; string and bytes keys are bounded in the run envelope with `truncated` and
`originalBytes`. An enum return renders as
`{"kind":"enum","member":"Enum::member"}`, the stable, reorder-invariant member
spelling `print`/`string`/interpolation produce, not a positional index.
Resource-shaped returns are outside the run surface and fail with
`run.entry_surface` (exit `1`). If return rendering or a later runtime fault
fails after a durable write has committed, stderr carries a run error envelope
with `diagnostics`, `output`, `store_stamp`, and `committed: true`; stdout does
not carry a successful result envelope. If an uncaught `Error` reaches the top
of a JSON run, stderr includes the original error code as
`diagnostics[0].data.code`.

`--dry-run` classifies the run through the checked project and store fences
without freezing first-run durable identity into the native store and without
auto-applying zero-mutation schema drift. If a real run would freeze the
baseline, apply schema drift, or fence, the dry-run report contains
tooling content for that action and exits `0`; JSON reports spell these booleans
as `would_freeze`, `would_apply`, and `would_fence`. When a fence would not
pass, the entry is not executed. Otherwise the entry runs against an isolated
store, so user `transaction` blocks cannot consume the dry-run boundary. Only
saved data is isolated; host side effects such as `std::io` writes or
`std::log` lines are not.

`--dry-run` takes `--format text|json`. The report is tooling output on stderr
under every format, off the program's stdout stream. Under text, planned writes
are `would write <path>` / `would delete <path>` lines, followed by per-target
create/write/delete counts and a `dry run: N write(s), M delete(s) (not
committed)` summary. A whole-record assignment (`^books(id) = book`) clears the
record slot before writing it, so it renders as a `would delete <record>`
immediately followed by `would write <record>` for the same id. When the id is
brand new this removes no existing data; the leading delete is the slot clear,
not a deletion of prior cells. Under `json`, the report object contains
`committed`, `writes`, `deletes`, `messages`, `would_freeze`, `would_apply`, `would_fence`,
`planned`, and `write_counts`. Planned entries carry the op, human path, base64
value bytes, and a structured `target`. Target identities, index keys, and keyed
data path segments use the same typed saved-key JSON objects as `marrow data`.
`write_counts.roots` and `write_counts.indexes` are objects keyed by root or
index name; each leaf is `{ "creates": N, "writes": N, "deletes": N }`. `creates`
counts records the run would newly create: a record establishes one create
regardless of how many field assignments touch it, and a write to a record that
already exists is a write, not a create. The `writes`/`deletes` summary equals
the sum of the per-target counts.

`--trace` composes with `--dry-run`: the run is traced while its saved writes are
isolated from the configured store. This composition is text-only: trace events
and the dry-run report both go to stderr, and the program's own stdout output
stays uninterrupted. For source-native data evolution use `marrow evolve
preview`; `run --maintenance --dry-run` is for
explicit repair/admin code.

Exits `0` on success, `1` if the project does not check, the store cannot be
opened, there is no entry, or the run raises an error. An uncaught runtime fault
is reported on stderr located at the source it was raised in,
`file:line:col: code: message`, the same form `check` and `test` use.

```console
$ marrow run ./proj
added 1: Small Gods

$ marrow run --entry shelf::main ./proj
added 2: Small Gods

$ marrow run --maintenance --entry shelf::repair ./proj
```

---

## `marrow test`

```
marrow test [--trace] [--format text|json|jsonl] [--filter <substring>] <projectdir>
```

Check a project, then run its tests: every `pub fn` with no parameters in a test
file selected by the `tests` paths in `marrow.json`. Each test runs against a
fresh in-memory store. A test's `std::log` output is discarded so it stays out
of the report.

In text format, each result is printed as `ok`, `FAIL` (a `std::assert::*`
failure, code `run.assertion`), or `ERROR` (any other runtime error), located at
the test's source position, followed by a summary line.
`--filter <substring>` runs only tests whose qualified name contains the
substring; a filter that selects nothing fails with `test.none`.

Under `--format json`, stdout is one test report envelope:

```json
{"project":"/absolute/path/to/proj","tests":[{"kind":"test","name":"tests::smoke_test::add_runs","outcome":"passed","file":"tests/smoke_test.mw","span":{"line":1,"column":1}}],"summary":{"total":1,"selected":1,"passed":1,"failed":0,"errored":0}}
```

Under `--format jsonl`, stdout is one test-result record per line followed by a
summary record:

```jsonl
{"kind":"test","name":"tests::smoke_test::add_runs","outcome":"passed","file":"tests/smoke_test.mw","span":{"line":1,"column":1}}
{"kind":"summary","total":1,"selected":1,"passed":1,"failed":0,"errored":0}
```

Failed and errored JSON records also carry the runtime fault `code` and an
`output` field. `output` is the test's bounded pre-fault `print` output as a
string, or `null` when the test produced no output. Passing records omit
`output`. Passing result spans point at the test function declaration; failed
and errored result spans point at the runtime fault.

Exits `0` only when every test passes. It exits `1` if any test fails or errors,
if the project does not check, or if no test is found (`test.none`).

With `--trace`, every test runs under an execution trace attributed to that test
by name. The trace is tooling output on stderr; the test report stays on stdout,
so the two streams never interleave. Trace events are text-only and stream as
they run; combining `--trace` with `--format json|jsonl` is a usage error.

```console
$ marrow test ./proj
ok    tests::smoke_test::add_runs
FAIL  tests::shelf_test::title_is_set
      tests/shelf_test.mw:7:5: run.assertion: assertion failed: isTrue(false)

2 tests: 1 passed, 1 failed, 0 errored
$ echo $?
1
```

The implemented assertions are `std::assert::isTrue`, `isFalse`, `equal`,
`isAbsent`, and `fail`.

## `marrow data`

`marrow data` is the typed inspection and repair-tooling boundary. It must read
through checked source, accepted catalog metadata, and typed tree-cell store
APIs. It does not expose raw backend keys, raw saved-path encoders, or archive
streams as production CLI behavior.

There is no `marrow explain` command in v0.1. Checked access, path, and name
facts are internal compiler/tooling facts surfaced through diagnostics,
`marrow data integrity`, dry-run reports, editor features, or future
accepted tooling surfaces. They are not exposed as optimizer or standalone
explanation output.

Diagnostic/admin/operator access to a project's saved data. The v0.1 decision is
to keep `get` and `dump` as `marrow data` subcommands, not production app APIs.
The inspection subcommands never create or modify the store; a project with no
saved data on disk reports as empty. `recover` is the only write-capable `data`
subcommand: it opens an existing native store so the backend can replay an
interrupted commit. `get` is exact-path and point-bounded. `dump` is
snapshot-bound and must stream or page rather than materializing unbounded data.
`roots`, `stats`, `dump`, `integrity`, and `get` also accept
`--backup <artifact>` to inspect a validated backup through an ephemeral
in-memory mount instead of opening the configured store; `recover` does not
accept that flag.
See [data-tools.md](data-tools.md) for full output shapes and the path syntax.
These commands are not production app APIs and not a production backup/restore
format.

`data diff` and `data load` are deferred — see
[future/data-tools.md](future/data-tools.md).

All `data` commands exit `2` on a usage error (missing directory, bad flag, an
unparseable `<path>` for `get`), and `1` on a config or store error. `roots`,
`stats`, `dump`, `recover`, and `get` exit `0` otherwise; `integrity` exits `1`
when it finds a problem.

### `data roots`

List the project's saved roots, one `^root` per line (or `(no saved data)`).

```console
$ marrow data roots ./proj
^books

$ marrow data roots --format json ./proj
{"project":"/absolute/path/to/proj","roots":["books"],"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
```

`store_snapshot` is `null` when no store-backed read occurs. Inside a present
snapshot, store metadata fields such as `store_uid`, `catalog_digest`, and
`commit` may be `null` when the store has not recorded that metadata.

### `data stats`

Count the saved roots, records, and cells. One record is one saved entity, an
identity tuple such as `^books(1)`; one cell is one stored `(path, value)` pair.
The record count is the same number `marrow backup` reports, `restore --replace
--count N` confirms, and evolution counts; the cell count matches the `data dump`
line count.

```console
$ marrow data stats ./proj
roots: 1
records: 1
cells: 2

$ marrow data stats --format json ./proj
{"project":"/absolute/path/to/proj","records":1,"cells":2,"roots":1,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
```

### `data dump`

Print every stored `(path, value)` for inspection: records in identity-key
order, each record's fields in declaration order. Text renders values through
their checked leaf type: strings are quoted and escaped, bytes are `0x<hex>`,
`Id(^store)` references are saved paths, and enum values are module-qualified
member identities. A `string` leaf whose stored bytes are not valid UTF-8 is
corruption, not bytes, and renders as `<undecodable string: 0x<hex>>` so it is
never mistaken for a `0x<hex>` bytes value; `data integrity` is the authority
that reports it. JSON/JSONL carry the checked path plus base64 of the value
bytes. This is not a production backup format.

```console
$ marrow data dump ./proj
^books(1).title	"Small Gods"
^books(1).author	"Terry Pratchett"
^books(1).loanedTo	^authors(1)
^books(1).state	app::Status::archived

$ marrow data dump --format jsonl ./proj
{"path":"^books(1).title","value_b64":"…"}
{"path":"^books(1).author","value_b64":"…"}
{"path":"^books(1).loanedTo","value_b64":"…"}
{"path":"^books(1).state","value_b64":"…"}
{"kind":"summary","cells":4,"store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}
```

### `data integrity`

Verify each checked, reachable stored value decodes against its declared schema
type, verify required-field completeness for existing records and keyed-layer
entries, verify that canonical identity leaves point to existing saved record
nodes, and verify that no actual stored data cell is left under a root or member
the schema no longer declares. It needs the checked project, so it loads and
checks the source first. It reports decode mismatches (`data.decode`), key type
mismatches (`data.key_type`), dangling identity leaves (`data.dangling_ref`),
missing required fields (`data.incomplete`), orphaned managed cells
(`data.orphan`), and corrupt typed tree-cell keys (`store.corruption`). Exits
`0` on a clean store, `1` when any problem is found.
Pending or defaulted members without an accepted catalog id create no stored-data
completeness obligation.

```console
$ marrow data integrity ./proj
ok: ./proj integrity verified (2 cells)
```

### `data recover`

Open the configured native store write-capably so the backend can replay an
interrupted commit after a read-only command reported `store.recovery_required`.
It reads only `marrow.json` to find the store path; it does not check source
files first. A missing native store is treated as nothing to recover and is not
created. An existing file that is not a Marrow store, including an empty file,
is `store.corruption`. If replay/open finds damage beyond recovery, the command
reports the store error such as `store.corruption`.

```console
$ marrow data recover ./proj
store open/repair completed: ./proj/.marrow/data/marrow.redb
```

### `data get`

Read one path's value for inspection. The value renders as checked text, like
`dump`: strings are quoted and escaped, bytes are `0x<hex>`, references are
saved paths, and enum values are member identities.
Absence is a valid result (exit `0`): a path with no value but children prints
`(no value; has children)`, a truly absent path prints `(absent)`. An
unparseable path is a usage error (exit `2`).

```console
$ marrow data get ./proj '^books(1).title'
"Small Gods"

$ marrow data get ./proj '^books(1).loanedTo'
^authors(1)

$ marrow data get --format json ./proj '^books(1).title'
{"path":"^books(1).title","presence":"value_only","value_b64":"U21hbGwgR29kcw==","store_snapshot":{"profile_version":"data.generation.v1","store_uid":"store_00000000000000000000000000000001","catalog_digest":"sha256:...","commit":{"commit_id":1,"catalog_epoch":1,"source_digest":"sha256:...","layout_epoch":0,"engine_profile_digest":"77944eb86c08b665"},"open_transaction":null,"checked_source_digest":"sha256:..."}}

$ marrow data get ./proj '^books(99).title'
(absent)
```

`store_snapshot` is `null` when the read has no store-backed version. Inside a
present snapshot, store metadata fields such as `store_uid`, `catalog_digest`,
and `commit` may be `null` when the store has not recorded that metadata.

---

## `marrow backup`

```
marrow backup <projectdir> <output-file>
```

Write a typed portable backup of a project's saved data. The backup is a Marrow
artifact, not a raw engine-file copy: a small header, a typed manifest, the
accepted-catalog section, and the project's canonical ordered data-cell stream.
The catalog section carries the accepted catalog rows, so a restored store is
self-contained and can regenerate the committed `marrow.lock` projection. The
manifest binds the data to the program that wrote it — its source digest,
accepted catalog epoch and digest, engine profile, value-codec version,
data-stream digest, store UID, and one integrity checksum over the manifest,
catalog section, and data cells — so a later restore
can refuse data it cannot faithfully reproduce. The manifest fields are
`source_digest`, `catalog_epoch`, `catalog_digest`, `state_digest`, `store_uid`,
reserved-empty `parent_snapshot_digest`, `engine`, `commit`, `record_count`, and
`archive_checksum`; this shape is backup `format_version` 6. The data stream
carries the store's data cells only; the generated indexes are derived, so a
restore rebuilds them rather than replaying them. Commit descriptors carry only
the slim commit stamp, not activation receipt counts or effect digests.

Backup cell targets derive from catalog stable IDs, so backups are
byte-identical only when the accepted catalog facts, engine profile, value
codec, and stored data match. Stable IDs are random opaque values that freeze
when accepted, so divergent catalog histories may still freeze distinct
accepted IDs for source that looks equivalent.

The store is read through one stable snapshot for the backup traversal. Backup
opens the store read-only and never modifies it; a project with no saved data
yet writes a valid empty backup.

The output archive is written to an adjacent temporary file and then renamed over
`<output-file>` only after the complete backup has been written successfully. A
failed backup preserves any prior archive at that path and removes its temporary
file. No overwrite flag is exposed: the path is replaced on success and preserved
on failure.

The reported count is the saved records (entities), the same number `data stats
records:` reports and `restore --replace --count N` confirms.

```console
$ marrow backup ./proj ./proj-backup.mwbackup
ok: backed up 12 record(s) to ./proj-backup.mwbackup
```

Exits `0` on success, `1` if the project does not check, the store cannot be
read, or the output file cannot be written, and `2` on a command-line usage
error.

## `marrow restore`

```
marrow restore [--replace --count N] <projectdir> <backup-file>
```

Replay a backup into the project's native store. Restore checks the project
against the accepted catalog the backup carries, validates the backup against
it (`restore.source_mismatch`, `restore.catalog_mismatch`,
`restore.engine_recompile_required`). By default it refuses a target that
already holds saved data, generated indexes, or an accepted catalog
(`restore.not_empty`), so a normal restore writes into an empty store only.
`--replace --count N` is the explicit destructive mode: restore first verifies
the live target through the same structural-digest integrity witness `data
integrity` runs, refusing a corrupt target as `store.corruption` before counting
or overwriting, then counts the live target's saved records (entities, the same
count `data stats records:` reports) and proceeds only when that count equals
`N`. A mismatch reports `restore.not_empty` with the expected and found record
counts and leaves the target data and catalog unchanged. `--replace`
without `--count`, `--count` without `--replace`, negative or non-integer counts,
and duplicate restore flags are usage errors.

Source mismatch reports print both the backup and project source digests. Catalog
mismatch reports print the backup catalog epoch/digest and the project catalog
epoch/digest. The replay writes the backup's catalog rows alongside its data
cells and mints a fresh store UID, so the restored store carries its accepted
identity and runs immediately. A non-empty `parent_snapshot_digest` is rejected;
v0.1 accepts only the empty reserved sentinel.
The whole replay runs in one transaction: a checksum mismatch or trailing bytes
(`restore.corrupt_chunk`), restored data that does not decode against the schema,
or an orphaned managed cell in the restored stream (`restore.data_invalid`) rolls
the target back to its prior state, so it either gains the whole backup or is
left unchanged. Because the replay is a single transaction, its memory use is
proportional to the backup size — a known v0.1 bound. Restore rebuilds the
generated indexes from the restored data inside the same transaction. In replace
mode the transaction first clears data, generated indexes, accepted catalog rows,
and restore-owned metadata, so a backup without a catalog cannot leave stale
catalog rows behind. A different engine, layout, or codec reports
`restore.engine_recompile_required`; applying that recompile is future work.

```console
$ marrow restore ./proj ./proj-backup.mwbackup
ok: restored 12 record(s) from ./proj-backup.mwbackup
```

```console
$ marrow restore --replace --count 12 ./proj ./proj-backup.mwbackup
ok: restored 12 record(s) from ./proj-backup.mwbackup; receipt: mode=replace expected_live_records=12 replaced_live_records=12
```

Exits `0` on success, `1` on any validation, checksum, store, or i/o failure, and
`2` on a command-line usage error. See [error-codes.md](error-codes.md) for the
`restore.*` family.
