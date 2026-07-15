# Errors

Marrow diagnostics use typed dotted codes. Human-readable messages explain
what happened, where it happened, and what to try next when Marrow knows.

Language-level error behavior is described in
[`language/errors-and-transactions.md`](language/errors-and-transactions.md).
Tool invocation is described in [`tools/cli.md`](tools/cli.md). This page is
generated from the code registry and lists every current code.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, runtime, storage, or tool failure. |
| `2` | Command-line usage failed before the command body ran. |

## Error Envelope

Machine-readable commands use this envelope where their selected format calls
for a single diagnostic object:

```json
{
  "code": "parse.syntax",
  "kind": "parse",
  "message": "expected expression",
  "help": "Add an expression after return.",
  "source_span": {
    "file": "src/app.mw",
    "line": 12,
    "column": 16
  }
}
```

The envelope is a tooling representation of an error. In `.mw` code, thrown
errors are `Error` values as described in the language reference. Tools may
add fields such as `kind` and `source_span` when reporting the error outside
the running program.

Common fields:

- `code`: typed machine code;
- `kind`: broad category such as `parse`, `check`, `runtime`, `storage`,
  `io`, `usage`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use lowercase dotted text such as `parse.syntax`. Segments
use lowercase letters, digits, and underscores.

Only the dotted `code` is machine-stable for storage errors. Details such as the
operation, path, limit name, or invalid state may appear only in the current
human-readable message; their wording is not a machine contract. The store
reports a `store.*` code:
`store.io`, `store.permission_denied`, `store.locked`, `store.format_version`,
`store.corruption`, `store.recovery_required`, `store.limit`, `store.cursor`,
`store.transaction`, and `store.read_only`.
`store.limit` reports an exhausted finite representation bound: a store framing
length/count that does not fit its `u32` field, a record/problem/index count
overflow, or exhaustion of the `u64` commit-ID sequence.

A command run against a project whose `marrow.toml` is unreadable reports
`io.read`; an invalid `marrow.toml` reports `config.invalid`, and a
contained-discovery fault reports a `project.*` code.

## How `kind` Is Assigned

Tools derive `kind` from the first dotted segment of `code`:

| First segment | `kind` |
|---|---|
| `parse` | `parse` |
| `check` | `check` |
| `image` | `artifact` |
| `run` | `runtime` |
| `value` | `runtime` |
| `store` | `storage` |
| `io` | `io` |
| everything else (`cli`, `config`, `fmt`, `project`, `wire`, `runner`) | `tooling` |

## Code Reference

The family sections below list codes emitted by the current build. Internal
codes are separate from ordinary user-facing diagnostics.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by project `check` and by any
command that parses sources before running.

| Code | Meaning |
|---|---|
| `parse.syntax` | The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected. |

### `fmt.*` — kind `tooling`

Formatter refusals.

| Code | Meaning |
|---|---|
| `fmt.comment_loss` | `marrow fmt` would drop a retained comment while rewriting the source, so the command refuses instead of publishing lossy formatted output. |

### `cli.*` — kind `tooling`

Capabilities the CLI recognizes but cannot yet serve on this beta line: a command
whose owning capability is being refounded, and a durable `marrow run` whose
execution is in the trough.

| Code | Meaning |
|---|---|
| `cli.command_unsupported` | A command name is recognized but not yet available on this beta line: its owning capability is being refounded and returns through a later lane. `marrow fmt`, `marrow --version`, and `marrow --help` are the currently available commands. |
| `cli.durable_unsupported` | `marrow run` resolved a durable export — one whose verified demand reads or writes durable data — that the beta line cannot yet execute. The export compiled, independently verified, and completed its durable identity, but the CLI no longer opens a store in process (T01's in-process open ended at D00, where the durable-run trough begins). Durable execution returns as the ephemeral-memory preview and later the persistent companion path. A storeless export is unaffected. |

### `check.*` — kind `check`

Static errors found while checking source.

| Code | Meaning |
|---|---|
| `check.nesting_limit` | Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see [execution limits](language/execution-limits.md). |
| `check.unsupported` | A parsed construct is well-formed Marrow but outside the subset the beta line currently compiles. Its owning language capability is being refounded lane by lane and returns through a later one; until then the construct is absent by the capability trough, and the checker reports this at its span. |
| `check.type` | An expression or declaration is not well-typed in the compiled subset: a return value whose type does not match the declared return type, an operator applied to the wrong operand type, a use of a name that is not in scope, or a value used where a different type is required. |
| `check.name_conflict` | Two declarations collide on a name the compiler must resolve uniquely: two functions in one module share a name, or two declarations share an identifier in the same scope. The message names the colliding declarations. |
| `check.module_path` | A file's `module` header does not match the module name derived from its source-root-relative path. The path is the authority for module identity, so `src/shelf/books.mw` must declare `module shelf::books`; the message names the expected path. |
| `check.import` | A `use` import cannot be resolved: it names a module the project does not contain, or two imports in one module bind the same final segment and are ambiguous. The message names the offending import. |
| `check.visibility` | A call from one module names a function in another module that is not `pub`. A function without `pub` is callable only within its own module; mark it `pub` to expose it across the module boundary. |
| `check.recursion` | A definition is part of a cycle the language requires to be acyclic: a function on a direct or mutual recursion cycle (the compiled subset does not admit recursion), a type alias whose expansion reaches itself, or a value type (struct, record, or enum) that contains itself directly or transitively (an infinite value; recursive nominal values are deferred). The message names the cycle. This is reported at check time so the source, not the image, carries the diagnostic. |
| `check.assert_outside_test` | An `assert` statement appears outside a `test` declaration. `assert` is the test-owned assertion: it is legal only inside a `test "name"` body, never in an ordinary function. Move the assertion into a test, or use `unreachable("...")` for an in-program invariant fault. |
| `check.match_nonexhaustive` | A `match` over an enum does not cover every selectable member of that enum. A flat enum's `match` must have exactly one arm per member and no wildcard arm; the message names the missing members. Add an arm for each uncovered member. |
| `check.match_arm` | A `match` arm is not well-formed against its scrutinee enum: it names a member the enum does not declare, repeats a member another arm already covers, binds a number of payload names that does not match the member's payload, or the scrutinee is not an enum value. The message names the offending arm. |
| `check.instantiation_limit` | Monomorphizing a program requires more distinct generic instantiations, or deeper generic type nesting, than the fixed limit. A well-typed program with acyclic call and value-containment graphs mints finitely many instances; this bound (campaign law 9) fails a divergent monomorphization — a generic function that calls itself, or a generic type that nests inside itself, over an ever-growing type — with a typed error before the instantiation worklist or the minting recursion grows unboundedly. |
| `check.durable_identity` | A durable declaration lacks its complete ledger identity: the store root, key column, stored resource, one of its fields, or the application itself has no matching entry in the machine-written `marrow.ids` identity artifact — or its `(kind, path)` names a retired identity that can never be reused. The message names the identity kind and path. `marrow run` mints missing identities into `marrow.ids` (commit that file); a retired path stays refused. `marrow.ids` is machine-written only and is never edited by hand. |

### `image.*` — kind `artifact`

Program-image decode and verification rejections, one per verifier phase. A
compiled image travels `bytes → verify → sealed image`; a hostile or malformed
image is rejected at the earliest phase whose invariant it violates, before the
VM can run it.

| Code | Meaning |
|---|---|
| `image.envelope` | A program image failed envelope verification (phase 1): a bad magic or version, a digest that does not match the image bytes, a malformed or misordered section frame, a declared length past the input, or trailing bytes. The image is rejected before any table is read. |
| `image.table` | A program image failed table verification (phase 2): a string, type, durable, constant, function, export, or span table violates its grammar — a duplicate or unsorted entry, an out-of-range index, a bad type tag or flag, or an operation site that does not resolve against the declared roots and records. |
| `image.function` | A program image failed per-function verification (phase 3): the bytecode does not decode to instruction boundaries, a jump leaves the function or lands off a boundary, an instruction is unreachable or a path falls off the end without returning, the typed operand stack does not agree at a merge or a return, a local is read before it is initialized, or a per-opcode rule is violated. |
| `image.closure` | A program image failed call/effect-closure verification (phase 4): the call graph contains a cycle (recursion is not admitted), or a recorded call or effect does not close consistently across the function set. |
| `image.flow` | A program image failed transaction-flow verification (phase 5): a transaction is begun outside an export entry, a mutation or mutating call sits outside the single owned transaction region, the region is not opened exactly once and closed on every path, or a read-only export contains a mutation. |
| `image.test_entry` | A program image failed test-entry verification: the closed non-wire TEST-ENTRY table is malformed (an out-of-range or duplicate/unsorted name or function index), an `assert` instruction sits in a function that is not a test entry, or a test entry is also an export, takes parameters, does not return unit, reads or writes durable data, or is called by another function. A test entry is a storeless zero-argument entry point, never an export or durable identity. |

### `run.*` — kind `runtime`

Source-mapped runtime faults raised by the VM and the path kernel while running a
verified program: checked-arithmetic overflow, a zero division or remainder
divisor, a text bound, a reached `unreachable` invariant, call depth, an
execution budget, a nominal-interval violation, a temporal-domain overflow, an
authority denial, a required field left unset at commit, an unconfirmed commit,
and durable corruption. These are not catchable inside the program.

| Code | Meaning |
|---|---|
| `run.overflow` | A checked integer operation overflowed the 64-bit range at runtime: an add, subtract, multiply, negate, or the `i64::MIN / -1` division and `i64::MIN % -1` remainder cases whose result is unrepresentable. The fault is mapped to the source span of the operation and is not catchable inside the program. |
| `run.divide_by_zero` | A division or remainder operation had a zero divisor at runtime. The fault is mapped to the source span of the operation and is not catchable inside the program. |
| `run.text_limit` | A text concatenation would exceed the fixed 64 KiB result bound, so the operation faults rather than allocating unboundedly. Mapped to the source span of the concatenation and not catchable inside the program. |
| `run.unreachable` | A program reached an `unreachable("...")` statement, the sole application-declared invariant fault. The static text records the invariant the author believed held; reaching the statement means it did not. The fault is mapped to the statement's source span and is not catchable inside the program. |
| `run.assert` | A `test`'s `assert` condition was false at runtime, so the test fails. `marrow test` reports the test as failed and maps the fault to the assertion's source span. Only a `test` body can produce this fault; it is not catchable inside the program. |
| `run.call_depth` | Runtime call depth exceeded the fixed limit (64). Static recursion is already rejected at verification, so this guards a pathologically deep non-recursive call chain; mapped to the call site and not catchable inside the program. |
| `run.budget` | A running program exhausted the fixed per-invocation instruction budget, shared across the whole call tree so total work stays bounded regardless of loop or call structure. A non-terminating loop faults here rather than running forever. The fault stops execution and is not catchable inside the program. |
| `run.range` | A value outside a nominal type's declared interval reached a construction or arithmetic result at runtime: `Age(n)` or a `supports`-unlocked operation produced an int the type's `in` range does not admit. The fault is mapped to the source span of the operation and is not catchable inside the program; use `Type.checked(n)` for a fault-free range test. |
| `run.authority` | An export's verified durable demand is not covered by the deployment ceiling intersected with the invocation grant, so the call is denied before the first engine access. The demand never grants access; it is only checked against it. Not catchable inside the program. |
| `run.required_missing` | A durable transaction reached its commit with an entry it created or staged that still leaves a required field unset. The transaction rolls back rather than committing a partial entry, and the fault is mapped to the transaction's source span. Not catchable inside the program. |
| `run.commit` | A durable transaction commit did not confirm. The store handle is poisoned and every later operation fails; the process must exit and reopen, where the recorded witness classifies whether the commit completed. The fault is mapped to the transaction's source span and is not catchable inside the program. |
| `run.corruption` | The path kernel found the durable store internally inconsistent while running a verified program: a field leaf with no entry marker (an orphan leaf), a cell it could not decode as its typed value, or a stored schema descriptor that does not match the program image. The fault is mapped to the operation's source span and is not catchable inside the program. |
| `run.collection_limit` | A `List` append or `Map` insert would grow a collection past a fixed representational bound: more than 65536 elements, or an aggregate value size over 1 MiB. The operation faults rather than allocating unboundedly, mapped to its source span, and is not catchable inside the program. |
| `run.temporal_overflow` | A temporal operation produced a result outside its supported domain at runtime: `date_add_days` or `instant +/- duration` left the supported calendar range (years 0001-9999), or `duration +/- duration` overflowed the signed-nanosecond `i128` range. The fault is mapped to the source span of the operation and is not catchable inside the program. Every `.mw` temporal path shares this 0001-9999 / `i128` envelope, so an out-of-range value never escapes into a stored value or key. |

### `value.*` — kind `runtime`

Value codec range faults raised at the store write/read boundary while encoding
a runtime value to its canonical saved bytes or projecting it to an
order-preserving key. These are catchable `Error` values inside a running
program.

| Code | Meaning |
|---|---|
| `value.range` | A `date` or `instant` reaching the store codec lies outside Marrow's supported calendar range, years 0001-9999. This is a store-boundary integrity guard, not a source-arithmetic fault: every `.mw` temporal path (the compile-time-validated `date`/`instant` literal constructors, `date_add_days`, and `instant +/- duration` arithmetic) shares the same 0001-9999 envelope and rejects at check time or raises `run.temporal_overflow` before an out-of-range value can be produced, so no ordinary checked program reaches this code. It fires only if a value that bypasses those bounds reaches the canonical encoder or key projection. |

### `store.*` — kind `storage`

Store faults. The tree-cell facade produces `store.corruption` for malformed
tree-cell metadata, value codecs, index cells, or accepted catalog rows. A
persistent backend can also produce the I/O, locking, format, corruption,
recovery, limit, and read-only variants. Opening a damaged native store fails
closed with a typed code — never a process crash: a truncated or torn body is
`store.corruption`, and a store left needing repair by an unclean shutdown is
`store.recovery_required`.

| Code | Meaning |
|---|---|
| `store.io` | An I/O operation on a persistent backend failed. |
| `store.permission_denied` | The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry. |
| `store.locked` | The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry. |
| `store.format_version` | The store's recorded format version is not the one this build supports. |
| `store.corruption` | The store file or a tree-cell record is corrupt and could not be opened or decoded, including a truncated or torn store body. |
| `store.recovery_required` | The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. The recovery command returns with the refounded durable lifecycle; recovery is attempted, not guaranteed, and a store damaged beyond replay surfaces `store.corruption`. |
| `store.limit` | Marrow exhausted a fixed representational bound: a store framing length/count did not fit its `u32` field, a record/problem/index count overflowed, or the `u64` commit-ID sequence was exhausted. |
| `store.cursor` | A bounded scan cursor does not belong to the scan being resumed. |
| `store.transaction` | A transaction or snapshot operation was requested in an invalid store state. |
| `store.read_only` | A write-capability operation was requested through a read-only store handle. |

### `io.*` — kind `io`

I/O faults spanning the CLI, the durable store, and the `std::io` builtins. The
CLI reports `io.read` when it cannot read a project file (e.g. `marrow.toml`)
and `io.thread` when it cannot start its worker thread. The `std::io` builtins
raise `io.read`/`io.write` as catchable `Error` values inside a running program.

| Code | Meaning |
|---|---|
| `io.read` | A read failed: a project source file or `marrow.toml` could not be read, or `std::io::readText`/`readBytes` failed. |
| `io.thread` | The CLI could not spawn the worker thread it uses for parsing, checking, and running. |
| `io.write` | `std::io::writeText`/`writeBytes` failed. |

### `config.*` — kind `tooling`

Configuration faults, including an invalid project manifest (`marrow.toml`) and
a non-UTF-8 command argument.

| Code | Meaning |
|---|---|
| `config.invalid` | A configuration input or project-setup precondition is invalid: the project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; a command argument is not valid UTF-8; or `marrow init` targets a directory that already exists. A malformed-manifest fault carries its `marrow.toml` line and column in `source_span`; a validation fault with no single source point carries none. |

### `project.*` — kind `tooling`

Project-capture faults raised while discovering a project's source under `src`
and reading its committed `marrow.ids` identity artifact: an invalid contained
path, a module-identity collision, an exceeded capture bound, a corrupt
identity artifact, or a failed identity mint.

| Code | Meaning |
|---|---|
| `project.source_path` | A captured source file path is not a valid contained module identity: it is absolute, escapes the source root with `..`, is not a canonical forward-slash path, contains a NUL or ASCII control character, lives outside the fixed `src` source root, or is not a `.mw` file with a non-empty name. A project whose `src` root is itself a symlink is refused with this code before discovery. |
| `project.module_collision` | Two captured source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files. |
| `project.capture_limit` | A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large. The bound guards the compiler against an unbounded project tree. |
| `project.ids_corrupt` | The committed `marrow.ids` identity artifact is corrupt and is rejected whole, never half-read: unresolved Git conflict markers, a malformed or duplicate row, two rows claiming one `(kind, path)` anchor or one id (the signature of a conflicting double-mint on parallel branches), a retired id reissued by a live row, an inconsistent retirement high-water, a truncated (torn) file missing its end marker, or a size past the fixed artifact bound. `marrow.ids` is machine-written only: restore it from version control rather than editing it. |
| `project.ids_mint` | `marrow run` could not mint a missing durable identity: the OS entropy source was unavailable, or a freshly drawn id collided with an existing or retired one (minting never retries a draw). The `marrow.ids` artifact is left byte-for-byte unchanged; rerun to draw fresh entropy. |

### `wire.*` — kind `tooling`

Local-wire protocol rejections raised by the single wire owner while framing or
decoding a message between the generated client and the runner. A frame is
rejected at the earliest bound or grammar rule it violates — an oversized frame,
a too-deep or too-long value, an unrecognized protocol version, a malformed
body, or a non-canonical encoding — before its content is acted on.

| Code | Meaning |
|---|---|
| `wire.frame_too_large` | A local-wire frame declared a payload longer than the fixed maximum frame size, so the framed message is rejected before its body is read or allocated (campaign law 9). The single wire owner rejects an oversized frame rather than buffering unbounded bytes off the socket. |
| `wire.depth_limit` | A local-wire message's canonical JSON nests arrays or objects deeper than the fixed maximum depth, so decoding is refused before the structure is fully materialized (campaign law 9). The bound fails a pathologically nested payload closed rather than recursing unboundedly. |
| `wire.string_limit` | A local-wire message's canonical JSON contains a string longer than the fixed maximum string size (campaign law 9). The bound fails an oversized string closed rather than allocating it. |
| `wire.unsupported_version` | A local-wire frame carried a protocol version byte this build does not speak. The runner and the generated client are a matched release pair; a version this build does not recognize is rejected at the frame boundary before the body is interpreted. |
| `wire.malformed` | A local-wire frame body is not a well-formed protocol message: its bytes are not valid JSON, carry a fractional or exponent number Marrow has no value for, name an unknown message kind, omit a required field, use a field of the wrong JSON type, or leave trailing bytes after the value. The single wire owner rejects it rather than acting on a partially understood message. |
| `wire.noncanonical` | A local-wire frame body is valid JSON but not in canonical form: it carries insignificant whitespace, object keys that are unsorted or duplicated, a non-minimal number spelling, or a non-canonical string escape. The single wire owner accepts only the one canonical encoding so a message has exactly one byte spelling. |

### `runner.*` — kind `tooling`

Runner request rejections raised while admitting a local-wire connection and
serving a request against the launched program image: a failed handshake, a
request naming an unknown export, arguments that do not match the export
signature, or a durable export the stock runner cannot yet execute.

| Code | Meaning |
|---|---|
| `runner.handshake` | A local-wire connection failed the runner handshake and was closed fail-closed: the connecting peer did not present the expected launch nonce, spoke an unsupported protocol version, or sent a malformed hello. No session is established and no request is served over the connection. |
| `runner.unknown_export` | A local-wire request named an export identity the served program image does not carry. The runner dispatches only on a verified export id present in the image it was launched with; an unknown id is rejected without running anything. |
| `runner.arg_mismatch` | A local-wire request's arguments do not match the target export's verified signature: the argument count differs, or an argument value does not decode into the declared parameter type. The runner rejects the request before running rather than coercing a mismatched value. |
| `runner.durable_unsupported` | A local-wire request named an export whose verified demand reads or writes durable data. The stock runner executes only storeless exports on this beta line; durable execution returns with the ephemeral-memory attachment and later the persistent companion path. A storeless export is unaffected. |

### Internal Codes

These codes are emitted only by implementation-maintainer surfaces or as
defense-in-depth fail-closed guards over invariants the surrounding layers
already close. They are not ordinary user-facing diagnostics.

| Code | Meaning |
|---|---|
| `run.enum_variant` | A defense-in-depth guard: a bytecode enum-payload read named a variant the running enum value did not select. The compiler dispatches on the enum tag before extracting a variant's payload, so ordinary compiled programs never reach this; it fails an image closed rather than reading a differently-typed payload leaf when a hand-built or corrupted image extracts the wrong variant. Mapped to the operation's source span and not catchable inside the program. |
| `run.collection_range` | A defense-in-depth guard: a bytecode positional collection read (a list element or a map key/value at an index) addressed a position past the collection's length. The compiler's `for` lowering keeps every positional read in bounds, so ordinary compiled programs never reach this; it fails an image closed rather than reading out of bounds when a hand-built or corrupted image supplies an out-of-range index. Mapped to the operation's source span and not catchable inside the program. |
