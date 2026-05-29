# Errors

Marrow errors are part of the product surface. A good error says what
happened, where it happened, and what to try next when Marrow knows.

Language-level error behavior is described in
[`language/control-flow-and-effects.md`](language/control-flow-and-effects.md).
This page describes the CLI and tooling contract.

## CLI Exit Codes

| Code | Meaning |
|---:|---|
| `0` | Command completed successfully. |
| `1` | Recoverable parse, check, capability, runtime, storage, project, or tool failure. |
| `2` | Command-line usage failed before the command body ran. |

## Error Envelope

Machine-readable surfaces use a stable envelope:

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

- `code`: stable machine code;
- `kind`: broad category such as `parse`, `check`, `capability`, `runtime`,
  `storage`, `io`, `usage`, `protocol`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use stable lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

Marrow surfaces use dotted Marrow error codes and typed error values.

Storage errors include the failed operation, a safe path or prefix when one is
available, and the capability or limit involved. Machine-readable facts belong
in `data`; clients do not parse `message`. The store reports a `store.*` code:
`store.io`, `store.locked`, `store.format_version`, `store.corruption`,
`store.limit`, and `store.corrupt_path`. Backends enforce no key or value size
limit, so `store.limit` is produced only by archive framing, when a record's
length exceeds the archive's `u32` chunk-length field.

Managed-root protection raises `write.*` codes when code attempts maintenance
work without the maintenance capability: `write.requires_maintenance` for a
whole managed-root delete, and `write.raw_requires_maintenance` for a raw
quoted-segment write or read under a managed root. The latter is distinct from
`write.unknown_field` so a tool can tell raw syntax from a declared-field typo.

The `marrow serve` data server reports a `protocol.*` code when a request is bad:
`protocol.malformed` (not JSON, or no `op`), `protocol.unknown_op`, and
`protocol.bad_request` (malformed operation arguments — a missing or bad `path`,
an unknown path segment or key type, or invalid base64). A request that reaches
the store carries the store's own `store.*` code through unchanged.

`marrow data integrity` reports `data.*` codes (kind `tooling`) for the findings
it surfaces while verifying saved data against the project schema:
`data.decode` for a stored value that is not a canonical form of its declared
type, and `data.orphan` for saved data under an unknown root or naming a member
the schema does not declare. An undecodable stored key it meets is surfaced with
the store's own `store.corrupt_path`. A command run against a project whose
`marrow.json` is unreadable reports `io.read`; an invalid `marrow.json` reports
`config.invalid`.

## How `kind` Is Assigned

Tools derive `kind` from the first dotted segment of `code`, so the kind of a
code is stable and predictable:

| First segment | `kind` |
|---|---|
| `parse` | `parse` |
| `check`, `schema` | `check` |
| `run` | `runtime` |
| `store` | `storage` |
| `io` | `io` |
| `protocol` | `protocol` |
| everything else (`config`, `project`, `data`, `write`, `test`) | `tooling` |

A `run.capability` error is the runtime form of a missing host capability; it
carries `kind` `runtime` (the `capability` kind named in the envelope section is
a category label, not a separate code prefix).

## Code Reference

Every code below is emitted by the current build. Codes are grouped by family.
The "Surface" column says where a developer first meets the code: a single-file
`check`, a project `check`/`run`/`test`, a managed write inside a running
program, the store, the `serve` data server, or a `data`/`backup`/`restore`
maintenance command.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by `check` (single file and
project) and by any command that parses sources before running.

| Code | Meaning |
|---|---|
| `parse.syntax` | The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected. |

### `check.*` — kind `check`

Static errors found while checking a project (module resolution, types, and
control-flow rules). A bare single-file `check` reports `parse.*` only; the
name-resolution and type rules below run when a whole project is checked (by
`check <projectdir>`, `run`, or `test`).

| Code | Meaning |
|---|---|
| `check.module_path` | A library file declares a module name that does not match its path. |
| `check.duplicate_module` | Two library files declare the same module name. |
| `check.duplicate_declaration` | A name is declared or imported more than once within a single file. |
| `check.unresolved_import` | A `use` names a module that is neither a project module nor a standard-library module. |
| `check.unknown_type` | A type annotation names a type the checker does not recognize. |
| `check.return_value` | A `return` carries a value in a function with no return type, or omits one in a value-returning function. |
| `check.missing_return` | A value-returning function can reach the end of its body without returning. |
| `check.operator_type` | An operator is applied to operands whose types it does not accept. |
| `check.condition_type` | An `if`/`while` condition is not a `bool`. |
| `check.call_argument` | A call passes the wrong number of arguments, or names a parameter that does not exist. |
| `check.return_type` | A `return` value's type does not match the function's declared return type. |
| `check.assignment_type` | A value's type does not match the typed binding or assignment target it is stored into. |
| `check.untyped_value` | A value whose type cannot be resolved (`unknown`) is stored into a concrete typed place. |
| `check.unresolved_name` | A bare name used as a value resolves to no binding in scope. |
| `check.unresolved_call` | A call names a function that is neither a builtin nor a declared function. |
| `check.next_id_requires_single_int` | `nextId(^root)` names a root with no default integer allocation policy (composite identity, a non-integer key, or a keyless singleton). The static counterpart of `write.next_id_unsupported`. |
| `check.literal_range` | A numeric literal is provably outside its type's range (an integer beyond `i64`, or a decimal outside the 34-digit / 34-place envelope). The static counterpart of `run.overflow`. |
| `check.finally_control_flow` | A `finally` block lets control flow escape via `return`, `break`, or `continue`. |
| `check.loop_control_flow` | A `break`/`continue` is outside any loop, or names no enclosing loop. |
| `check.catch_type` | A `catch` annotation is not `Error`. |
| `check.invalid_assign_target` | An assignment or `merge` target is not a writable place. |
| `check.non_constant_const` | A `const` initializer is not a constant expression. |
| `check.loop_mutates_traversed_layer` | A loop over a saved layer mutates that same layer. The static counterpart of `run.traversal`. |

### `schema.*` — kind `check`

Resource-schema rules. Reported during a project check alongside `check.*`.

| Code | Meaning |
|---|---|
| `schema.duplicate_member` | A resource member name collides with another member at the same level. |
| `schema.duplicate_root_owner` | Two resources claim the same saved root (a cross-resource rule the project checker reports). |
| `schema.index_in_group` | An index appears inside a group; indexes are direct members of keyed saved resources. |
| `schema.unknown_in_saved` | A managed saved field or key is typed `unknown`; saved schemas use concrete types. |
| `schema.key_member_collision` | A top-level field or layer shares a name with an identity key. |
| `schema.unknown_index_arg` | An index argument does not resolve to an identity key, a top-level field, or a field reached through unkeyed groups. |
| `schema.duplicate_stable_id` | Two resource elements declare the same stable ID. |
| `schema.unorderable_key` | A saved key has a type with no order-preserving key encoding (currently `decimal`). |
| `schema.index_missing_identity_keys` | A non-unique index does not end with all identity keys in declaration order. |
| `schema.index_requires_keyed_root` | An index is declared on a resource with no keyed saved root. |
| `schema.required_in_unkeyed_group` | A `required` field is declared inside an unkeyed group (not yet materialized by the write planner). |
| `schema.nested_index_arg` | An index argument names a field nested through an unkeyed group (not yet resolved by the write planner). |

### `run.*` — kind `runtime`

Runtime faults from the evaluator, surfaced by `run` and `test`. In `.mw` code
these are catchable `Error` values; a fault that reaches the top of the program
is reported under the code below, except `run.uncaught_error` — see "Typed
errors in running programs".

| Code | Meaning |
|---|---|
| `run.type` | A value was used where another type was required. |
| `run.unbound_name` | A name was read or assigned that is not bound in scope. |
| `run.overflow` | Integer arithmetic overflowed the 64-bit range. |
| `run.divide_by_zero` | Integer division or remainder by zero. |
| `run.no_enclosing_loop` | A `break`/`continue` reached the top of a function with no loop to target. |
| `run.unknown_function` | A call named a function the program does not declare. |
| `run.no_value` | A call to a function that returns no value was used where a value is needed. |
| `run.absent_element` | A direct read of a saved element that is absent (unpopulated). |
| `run.store` | The store reported an error (e.g. a corrupt stored path) during a read. |
| `run.unsupported` | A construct this slice of the runtime does not yet evaluate. |
| `run.capability` | A host capability a builtin needs (e.g. the clock for `std::clock::now`) was not provided to this run. |
| `run.assertion` | A `std::assert::*` assertion did not hold. `marrow test` reports these as located test failures. |
| `run.uncaught_error` | An `Error` raised by `throw` reached the top of a function with no `catch`. The original code travels in the message (e.g. `[io.read]`). |
| `run.traversal` | A write, delete, append, or merge changed the saved layer a loop was actively traversing. The dynamic counterpart of `check.loop_mutates_traversed_layer`. |
| `run.no_entry` | `marrow run` found no entry: no `--entry` was given and `marrow.json` sets no `run.defaultEntry`. |

### `write.*` — kind `tooling`

Managed-write faults raised by the write planner inside a running program. They
surface to `run`/`test` as `Error` values, so an uncaught one is reported as
`run.uncaught_error` carrying the `write.*` code in its message.

| Code | Meaning |
|---|---|
| `write.required_absent` | A required field was absent in a whole-resource write. |
| `write.type_mismatch` | A field value's type does not match the resource schema. |
| `write.no_saved_root` | The resource has no saved root, so it cannot be written to saved data. |
| `write.identity_mismatch` | The supplied identity keys do not match the resource's saved root. |
| `write.store` | The store reported an error during a write. |
| `write.unknown_field` | A field write names a field the resource does not declare. |
| `write.unique_conflict` | A unique index already maps the supplied key(s) to a different resource. |
| `write.unknown_layer` | A keyed-layer write names a layer the resource does not declare. |
| `write.not_a_leaf_layer` | A keyed-leaf write targets a group layer. |
| `write.not_a_group_layer` | A group-entry field write targets a leaf layer. |
| `write.layer_key_arity` | A keyed-layer write supplies the wrong number of layer keys. |
| `write.id_overflow` | The integer key space is exhausted (`i64::MAX`), so no next identity or position can be allocated. |
| `write.next_id_unsupported` | `nextId` was asked for a root whose identity shape has no default integer allocation policy. The runtime backstop for `check.next_id_requires_single_int`. |
| `write.required_field` | Deleting a `required` field on its own is rejected outside maintenance. |
| `write.requires_maintenance` | A whole managed-root delete (`delete ^books`) was attempted without the maintenance capability. |
| `write.raw_requires_maintenance` | A quoted/raw segment under a managed root was used without the maintenance capability. Distinct from `write.unknown_field` so a tool can tell raw syntax from a declared-field typo. |

### `store.*` — kind `storage`

Backend faults. The in-memory store can only produce `store.corrupt_path`; a
persistent backend can also produce the I/O, locking, format, corruption, and
limit variants. A store fault met during a program read or write travels as
`run.store` or `write.store`; the `serve` server passes the `store.*` code
through unchanged.

| Code | Meaning |
|---|---|
| `store.corrupt_path` | A stored key is not a well-formed sequence of path segments. |
| `store.io` | An I/O operation on a persistent backend failed. |
| `store.locked` | The store file is already held open by another writer. |
| `store.format_version` | The store's recorded format version is not the one this build supports. |
| `store.corruption` | The persistent store is corrupt and could not be opened or read. |
| `store.limit` | An archive chunk exceeded the framing limit (a record length above the archive's `u32` chunk-length field). Backends enforce no key/value size limit, so archive framing is the sole producer. |

### `protocol.*` — kind `protocol`

Request faults from the `marrow serve` data server. A serve error reply is
`{"id": …, "error": {"code": …, "message": …}}`; it does not carry `kind` or
`source_span`. A request that reaches the store carries the store's own
`store.*` code through.

| Code | Meaning |
|---|---|
| `protocol.malformed` | A request is not a JSON object, or is missing a string `op`. |
| `protocol.unknown_op` | A request names an operation the server does not support. |
| `protocol.bad_request` | A known operation's arguments are malformed: a missing or bad `path`, an unknown path segment or key type, or invalid base64. |

### `io.*` — kind `io`

Filesystem faults. The CLI reports `io.read` when it cannot read a project file
(e.g. `marrow.json`). The `std::io` builtins raise `io.read`/`io.write` as
catchable `Error` values inside a running program.

| Code | Meaning |
|---|---|
| `io.read` | A read failed: a project source file or `marrow.json` could not be read, or `std::io::readText`/`readBytes` failed. |
| `io.write` | `std::io::writeText`/`writeBytes` failed. |

### `config.*` and `project.*` — kind `tooling`

Project-loading faults from `marrow.json` and source discovery.

| Code | Meaning |
|---|---|
| `config.invalid` | `marrow.json` is malformed JSON, has an unknown key, is missing a required field, or names an unknown backend. |
| `project.source_root` | A configured source root could not be walked (e.g. the directory does not exist). |

### `data.*` — kind `tooling`

Findings from `marrow data integrity`, which verifies saved values against the
project schema. Read-only; it never modifies the store.

| Code | Meaning |
|---|---|
| `data.decode` | A stored value is not a canonical form of its declared type. |
| `data.orphan` | Saved data lives under an unknown root or names a member the schema does not declare. |

(An undecodable stored *key* met during integrity verification is surfaced with
the store's `store.corrupt_path`, not a `data.*` code.)

### `test.*` — kind `tooling`

| Code | Meaning |
|---|---|
| `test.none` | `marrow test` found no tests; check the `tests` patterns in `marrow.json`. Exit code `1`. (Failing tests are reported per test with their own `run.assertion` or other `run.*` code, not a `test.*` code.)|

### `restore.*` — kind `tooling`

| Code | Meaning |
|---|---|
| `restore.not_empty` | `marrow restore` targets a non-empty store; normal restore writes into an empty target only. (Replace, merge, and repair restores are deferred — see [future/cli.md](future/cli.md).) Exit code `1`. |

## Typed Errors In Running Programs

In `.mw` code an error is an `Error` value with its own dotted `code`, raised by
`throw` and caught by `catch`. Builtins and managed writes raise typed errors
too: a failed `std::io::readText` raises `io.read`, a rejected write raises a
`write.*` code, and so on, all catchable in code. When such an error is *not*
caught and reaches the top of the program, `run`/`test` report it as
`run.uncaught_error` and carry the original code in the message, for example:

```
run.uncaught_error: uncaught error [io.read]: std::io::readText failed for `/no/such/file`: No such file or directory (os error 2)
```

## Deferred Surfaces

`marrow data diff`/`data load` and the non-empty `marrow restore` modes
(replace, merge, repair) are deferred — see [future/data-tools.md](future/data-tools.md)
and [future/cli.md](future/cli.md). No new code family appears for a
deferred surface until that surface ships.
