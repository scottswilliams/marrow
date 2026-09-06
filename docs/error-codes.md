# Errors

Every Marrow diagnostic carries a dotted code such as `check.type`. The code is
the stable part. The message beside it says what happened, where, and what to
try; its wording is not a machine contract.

A code's first segment names its family. `parse.*` and `check.*` are source
diagnostics, reported at a line and column. `image.*` rejects a program image
before it runs. `run.*` and `value.*` are runtime faults: a fault stops the
invocation at the source span of the operation, and a program cannot catch it.
The remaining families are operational errors from the store, the command line,
the project, and the runner.

Language-level error behavior is described in
[`language/errors-and-transactions.md`](language/errors-and-transactions.md).
Tool invocation is described in [`tools/cli.md`](tools/cli.md). This page is
generated from the code registry and lists every code the current build emits.

## Code reference

### `parse.*`

Syntax errors from the lexer and parser, reported by every command that reads
source.

| Code | Meaning |
|---|---|
| `parse.syntax` | The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the message says what was expected. |

### `fmt.*`

Refusals from `marrow fmt`.

| Code | Meaning |
|---|---|
| `fmt.comment_loss` | `marrow fmt` would drop a comment while rewriting the file, so it writes nothing. |
| `fmt.diagnostic_limit` | `marrow fmt` needs a complete parse, and the file produced more parse diagnostics than the collector keeps (4096 diagnostics or 1 MiB of text), so it writes nothing. Fix the parse errors, then format. |

### `cli.*`

Refusals raised by the `marrow` command itself.

| Code | Meaning |
|---|---|
| `cli.command_unsupported` | The command name is reserved and not implemented: `data`, `doctor`, `evolve`, `serve`, `backup`, and `restore`. `marrow --help` lists the implemented commands. |
| `cli.interface_unbuildable` | An export's signature cannot be projected onto the wire: it expands past the fixed interface budget, or it names a type the image does not declare. `marrow client typescript` and the runner refuse the whole program; the message names the export. |
| `cli.durable_unsupported` | `marrow run` resolved an export that reads or writes durable data, and no store was given. `marrow` itself opens no store; the companion runner does. Run the export against a provisioned store: `marrow run <export> --store <dir>`. A storeless export is unaffected. |
| `cli.installation_damaged` | `marrow run --store` could not use the companion runner: the release manifest beside the toolchain is missing or malformed, names another release, or the runner binary is absent or does not match its recorded identity. The store is untouched. Reinstall the toolchain. |
| `cli.ceiling_unaccepted` | `marrow image` writes an image only when `--accept-ceiling <id>` names the image's own deployment ceiling. The argument was absent or named a different id, so no image was written. The message prints the id to accept. |
| `cli.compiler_resource_limit` | Compilation crossed a fixed bound that no single construct is at fault for: an aggregate count across the whole program, or the image byte ceiling. No image is produced and the outcome carries no source location. When the image byte ceiling is crossed, checking stops at that bound, so other diagnostics the program carries are not reported until it fits. A bound one construct crosses is `check.resource_limit` at that construct. |

### `check.*`

Static errors found while checking source.

| Code | Meaning |
|---|---|
| `check.nesting_limit` | Source nests expressions or blocks deeper than the parser limit (256). Reported at the offending span. The limit is listed under [execution limits](language/execution-limits.md). |
| `check.unsupported` | The construct is well-formed Marrow that this compiler does not implement today. Reported at the construct's span. [Status](status.md) lists what is available. |
| `check.type` | An expression or declaration is not well-typed: a return value of the wrong type, an operator applied to the wrong operand type, a name that is not in scope, or a value used where another type is required. |
| `check.name_conflict` | Two declarations share a name in one scope: two functions in one module, two declarations with one identifier, or a member, parameter, type parameter, or key column declared twice in one layer. The message names the owner and the repeated name. |
| `check.module_path` | A file's `module` header does not match the name derived from its path under `src`. `src/shelf/books.mw` declares `module shelf::books`; the message names the expected path. |
| `check.import` | A `use` import names a module the project does not contain, or two imports in one module bind the same final segment. The message names the import. |
| `check.visibility` | A call from one module names a function in another module that is not `pub`. A function without `pub` is callable only within its own module; mark it `pub` to call it from elsewhere. |
| `check.recursion` | A definition is part of a cycle: a function that calls itself directly or through other functions, a type alias that expands to itself, or a struct, resource, or enum that contains itself. Marrow admits none of these. The message names the cycle. |
| `check.requires_transaction` | A durable write, replacement, or delete runs outside a `transaction` block. A mutating export owns one block around its writes. A mutating helper is called only from inside a caller's block, and a function that calls one needs a block in turn. Reported at the write or the call; wrap it in a `transaction` block. |
| `check.transaction_owner_called` | A function calls an export that owns a `transaction` block. An owner's block begins and commits in its own frame and does not nest inside a caller's. Only a `test` body drives an owner, one transaction per call. Move the durable work into a helper without a block and call it inside the export's own block. |
| `check.transaction_empty` | A `transaction` block performs no durable operation, directly or through a call. Such a block commits nothing. Remove it, or move the durable work inside it. |
| `check.transaction_reopened` | A mutating export opens a second `transaction` block. An export owns exactly one block and commits it on every path. Combine the durable work into one block. |
| `check.transaction_uncommitted` | A path leaves a `transaction` block without committing it. The block commits at each `return` written inside it and at its closing brace. A `try` or `require` guard whose `err` exit would return from inside the block bypasses both. Spell a deliberate failure as a `return` inside the block, and place a guard that fails without committing before the block. |
| `check.durable_after_commit` | A durable read or write follows the commit of a `transaction` block on some path, directly or through a call. Move the operation inside the block, or capture the value into a local before the block closes and return the local. |
| `check.transaction_misplaced` | A `transaction` block appears in a helper that is not `pub` or in a `test` body. Only an export owns a block: a helper runs inside its caller's block, and a test drives exports or touches durable data directly. Move the block to the export that owns the durable work. |
| `check.assert_outside_test` | An `assert` statement appears outside a `test` body. Move it into a test, or use `unreachable("...")` for an invariant inside a function. |
| `check.test_driver_mix` | A `test` body both touches durable data directly and calls an export that owns a `transaction` block. A body does one or the other: it reads and writes `^` places itself, or it drives exports, where each call commits on its own. Split the two into separate tests, or reach the data through the exports. |
| `check.match_nonexhaustive` | A `match` over an enum does not cover every member. A `match` has exactly one arm per member and no wildcard arm. The message names the missing members. |
| `check.match_arm` | A `match` arm names a member the enum does not declare, repeats a member another arm covers, or binds the wrong number of payload names; or the value matched is not an enum. The message names the arm. |
| `check.instantiation_limit` | Instantiating the program's generic functions and types needs more distinct instances, or deeper type nesting, than the fixed limit. A generic function that calls itself, or a generic type that nests inside itself, over an ever-growing type reaches it. |
| `check.resource_limit` | One construct crosses a fixed bound of the program image: a declaration too wide, a stored value or member tree too deep, a key tuple or index too long, or a function body or string too large. Reported at the construct; the bounds are listed under [execution limits](language/execution-limits.md). An aggregate exhaustion with no single construct at fault is `cli.compiler_resource_limit`. |
| `check.durable_identity` | A durable declaration has no identity in `.marrow/ids`: the store root, a key component, the stored resource, one of its fields, or the application itself has no entry there, or names a retired one. The message names the kind and path. `marrow run` mints missing identities into `.marrow/ids`; commit that file. A retired path stays refused. The file is machine-written. |

### `image.*`

Program-image verification failures. An image is verified in phases before it
runs, and a malformed or altered image is rejected at the first phase that finds
a fault.

| Code | Meaning |
|---|---|
| `image.envelope` | A program image failed envelope verification (phase 1): a bad magic or version, a digest that does not match the bytes, a malformed or misordered section, a length past the input, or trailing bytes. Nothing else is read. |
| `image.table` | A program image failed table verification (phase 2): the string, type, durable, constant, function, export, or span table breaks its grammar with a duplicate or unsorted entry, an out-of-range index, a bad type tag or flag, or a durable operation that does not resolve against the declared roots. |
| `image.function` | A program image failed function verification (phase 3): bytecode that does not decode to instruction boundaries, a jump that leaves the function or targets a non-boundary, an unreachable instruction, a path that falls off the end, an operand stack that disagrees at a merge or return, a local read before it is set, or a broken per-opcode rule. |
| `image.closure` | A program image failed call and effect closure (phase 4): the call graph contains a cycle, or a recorded call or effect does not close consistently across the functions. |
| `image.flow` | A program image failed transaction-flow verification (phase 5): a transaction begun outside an export, a write outside the export's one owned block, a block not opened once and closed on every path, or a read-only export that writes. These are the rules `check.transaction_*` reports at source. |
| `image.test_entry` | A program image failed test-entry verification: the test-entry table is malformed, an `assert` sits in a function that is not a test, a test entry is an export, takes parameters, returns a value, or is called by another function, or a test body both touches durable data directly and drives a transaction-owning export. |

### `run.*`

Runtime faults raised while running a verified program.

| Code | Meaning |
|---|---|
| `run.overflow` | A checked integer operation overflowed 64 bits: an add, subtract, multiply, or negate, or the `i64::MIN / -1` division and `i64::MIN % -1` remainder. |
| `run.divide_by_zero` | A division or remainder had a zero divisor. |
| `run.text_limit` | A text concatenation would exceed the 64 KiB result bound. |
| `run.unreachable` | The program reached an `unreachable("...")` statement. The text records the invariant the author believed held. |
| `run.todo` | The program reached a `todo("...")` statement. The text names the deferred work. |
| `run.assert` | A `test`'s `assert` condition was false, so the test fails. Only a test body produces this fault. |
| `run.call_depth` | The call chain grew deeper than the fixed limit (64). Recursion is refused at check time, so this guards a very deep chain of distinct calls. |
| `run.budget` | The invocation exhausted its fixed instruction budget (2^26 instructions), which is shared across the whole call tree. A loop that never terminates faults here. |
| `run.range` | A value outside a nominal type's declared interval reached a construction or arithmetic result: `Age(n)` or a `supports` operation produced an int the type's `in` range does not admit. `Age.checked(n)` tests the range without faulting. |
| `run.authority` | An export's durable demand is not covered by the store's ceiling intersected with the invocation grant, so the call is denied before the first store access. Demand never grants access; it is only checked against it. |
| `run.required_missing` | A `transaction` block reached its commit with an entry it created or staged that still has a required field unset. The block rolls back before any store write. The invocation reports `incomplete` with durable state `known_old`, at the block's span. |
| `run.unique_index` | A write would place two entries whose indexed values are equal but whose identities differ into one `unique` index. The whole transaction rolls back and the store is unchanged. |
| `run.commit` | A commit did not complete. A confirmed abort leaves durable state unchanged (`known_old`). An indeterminate result is classified after the store is reopened and audited as `known_old`, `known_new`, or `unknown`. The invocation returns no value and is never retried. Reported at the block's span. |
| `run.outcome_unknown` | A call was dispatched to the runner, but the caller could not accept one exact valid reply: a socket-read failure, a malformed frame, a mismatched turn, an unsolicited message, or a reply that did not decode. The call may have run, wholly or partly, and is never retried. Run a read-only export to observe durable state before acting again. |
| `run.corruption` | A verified program found the store or the image inconsistent and stopped: a field leaf with no entry marker, a cell that does not decode as its type, a stored schema that does not match the image, or a positional collection read past the collection's length. The compiler keeps every positional read in bounds, so the last case guards a hand-built or corrupted image. |
| `run.collection_limit` | A `List` append or `Map` insert would grow a collection past 65,536 elements or 1 MiB. |
| `run.temporal_overflow` | A temporal operation left its supported domain: `addDays` or an `instant` plus or minus a `duration` left the years 0001-9999, or a `duration` sum overflowed the signed nanosecond range. Every temporal value shares this envelope, so an out-of-range value never reaches a stored value or key. |

### `value.*`

Faults raised while encoding a value for a durable write.

| Code | Meaning |
|---|---|
| `value.range` | A durable value cannot be represented by the store codec: at a durable write, a composite field's individually bounded scalar leaves exceed the dynamic 1 MiB aggregate encoded-value limit. Encoding completes before any store write, so the rejected write has no store effect. The same code closes codec range arms, such as a date outside 0001-9999, that checked source cannot produce. |

### `store.*`

Faults from a store. The message names the store path or operation; only the
code is stable.

| Code | Meaning |
|---|---|
| `store.io` | An I/O operation on a store failed. |
| `store.permission_denied` | The process lacks read/write access to the store directory or file. The message names the store path; grant access to that directory, then retry. |
| `store.locked` | The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry. |
| `store.format_version` | The store records a format version this build does not support. |
| `store.corruption` | The store file or one of its cells is corrupt and could not be opened or decoded, including a truncated or torn store body. |
| `store.recovery_required` | The store was left unclean by an interrupted shutdown, and a read-only open cannot repair it. A writing open recovers it; recovery replays no Marrow code and retries no invocation. If recovery cannot produce an openable store, the open reports `store.corruption`. |
| `store.limit` | A fixed bound of the store's representation is exhausted: a framing length or count that does not fit its field, an entry, problem, or index count that overflowed, or an exhausted commit-witness generation. |
| `store.cursor` | A traversal cursor does not belong to the traversal being resumed. |
| `store.transaction` | A transaction or snapshot operation was requested in an invalid store state. |
| `store.read_only` | A write was requested through a read-only store handle. |
| `store.contract_changed` | The program image changes the durable contract or the exported interface versus the store's active binding, so it is not a code-only update. The store is intact and the prior program remains usable. Accepting a changed contract is future work; today a new store is provisioned from the new program. [Changing the program](operations/README.md#changing-the-program) describes the outcomes. |
| `store.demand_exceeds_ceiling` | The program image's durable demand exceeds the ceiling the store was provisioned under. The message names, for each place beyond the ceiling, the export, the effect (read, write, presence, delete, or iterate), and the place. No store call is made and the store is intact. Expand the store's accepted ceiling to cover the named demand before running the new program. |
| `store.image_not_active` | The program is a code-only edit of the store's active program, and the requested operation does not rebind. `marrow import` populates a store only under its active program; run `marrow run --store` with the current program first, which rebinds the store to the new code, then retry. The store is intact. |

### `io.*`

Operational I/O faults from the command line and the runner.

| Code | Meaning |
|---|---|
| `io.read` | An operational read failed, such as reading a project source file, `marrow.toml`, a runner launch artifact, or a runner protocol frame. |
| `io.thread` | The CLI could not spawn the worker thread it uses for parsing, checking, and running. |
| `io.write` | An operational write failed, such as creating an initialized project file, publishing a generated client or identity artifact, writing command output, or writing a runner protocol frame. |

### `config.*`

Configuration faults, including an invalid project manifest.

| Code | Meaning |
|---|---|
| `config.invalid` | The project manifest `marrow.toml` is malformed TOML, declares an unknown key, or declares no supported `edition`; a command argument is not valid UTF-8; or `marrow init` targets a directory that already exists. A malformed manifest reports its line and column. |

### `project.*`

Faults from discovering a project's sources under `src` and reading its
identity ledger `.marrow/ids`.

| Code | Meaning |
|---|---|
| `project.source_path` | A source file path is not a valid module identity: it is absolute, escapes `src` with `..`, is not a canonical forward-slash path, contains a NUL or control character, lives outside `src`, is not a `.mw` file with a non-empty name, or exceeds 4096 bytes. A project whose `src` is a symlink reports this before discovery. |
| `project.module_collision` | Two source files collide on module identity: they derive the same module name, or their paths differ only in case and would name the same file on a case-insensitive filesystem. The message names both files. |
| `project.capture_limit` | A project capture exceeded a fixed bound: too many source files, one source file too large, or the source files together too large. |
| `project.ids_corrupt` | `.marrow/ids` is corrupt and is rejected whole: unresolved Git conflict markers, a malformed or duplicate line, two lines claiming one `(kind, path)` or one id (a double mint on parallel branches), a retired id reissued, an inconsistent retirement high-water, a truncated file missing its end marker, or a size past the fixed bound. Restore the file from version control. |
| `project.ids_mint` | `marrow run` could not mint missing identities: an anchor was invalid, duplicated, live, or retired; the ledger would exceed its fixed size; the entropy source failed; or a candidate id collided. `.marrow/ids` is unchanged. Fix the source or the ledger state, then run again; an entropy failure or a collision may pass on another attempt. |
| `project.ids_location` | The identity ledger was found at the retired path `marrow.ids`. Its home is `.marrow/ids`: move it with `git mv marrow.ids .marrow/ids` and commit the move. When both exist, keep `.marrow/ids` and delete the root file; a project has exactly one ledger. |
| `project.ids_publication_pending` | A `.marrow/ids` publication marker is live, so no command reads the ledger. `.marrow/ids.pending` means a publication was interrupted; `marrow run` settles it before it reads the project. A stray `.marrow/ids.pending.create` is not settled automatically: delete it and `.marrow/ids.publish.stage`, then run again. |

### `wire.*`

Rejections of a message between the generated client and the runner. A frame is
rejected at the first bound or grammar rule it breaks, before its content is
acted on.

| Code | Meaning |
|---|---|
| `wire.frame_too_large` | A frame declared a payload longer than the maximum frame size. It is rejected before its body is read. |
| `wire.depth_limit` | A message's JSON nests deeper than the maximum depth. Decoding stops before the structure is built. |
| `wire.string_limit` | A message's JSON contains a string longer than the maximum string size. |
| `wire.unsupported_version` | A frame carried a protocol version this build does not speak. The runner and the generated client are a matched release pair. |
| `wire.malformed` | A frame body is not a well-formed message: not valid JSON, a fractional or exponent number, an unknown message kind, a missing or mistyped field, or trailing bytes. |
| `wire.noncanonical` | A frame body is valid JSON but not canonical: insignificant whitespace, unsorted or duplicate keys, a non-minimal number, or a non-canonical escape. A message has exactly one byte spelling. |

### `runner.*`

Rejections from the runner that serves a launched program.

| Code | Meaning |
|---|---|
| `runner.handshake` | A connection failed the handshake: the peer did not present the launch nonce, spoke an unsupported version, or sent a malformed hello. No session is established. |
| `runner.unknown_export` | A request named an export the served image does not carry. Nothing runs. |
| `runner.arg_mismatch` | A request's arguments do not match the export's signature: the count differs, or a value does not decode as the parameter type. Nothing runs. |
| `runner.durable_unsupported` | A request named a durable export the runner cannot serve: the storeless serve mode has no store, or the program's durable shape is one the runner does not execute today. A storeless export, and a durable export over a provisioned store, are unaffected. |
| `runner.spawn` | The `marrow` process could not start the companion runner for a persistent run. The store is untouched. |

### Internal codes

These codes guard invariants the surrounding layers already close. An ordinary
program does not reach them.

| Code | Meaning |
|---|---|
| `cli.compiler_invariant` | The compiler detected an internal state inconsistency and failed closed without producing a program image or source diagnostic. |
| `run.enum_variant` | A bytecode enum-payload read named a member the value did not select. The compiler dispatches on the tag before reading a payload, so a compiled program does not reach this; it guards a hand-built or corrupted image. |
| `runner.reply_encode` | A served export's return value failed to encode for the wire. Interface build excludes an export whose return shape is not transferable, so a served program does not reach this; the request fails closed. |
