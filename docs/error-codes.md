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
  `storage`, `io`, `usage`, or `tooling`;
- `message`: short human summary;
- `help`: optional repair guidance;
- `source_span`: optional source location;
- `data`: optional structured facts for tools.

Marrow error codes use stable lowercase dotted text such as `parse.syntax` or
`book.already_loaned`. Segments use lowercase letters, digits, and
underscores.

Marrow surfaces use dotted Marrow error codes and typed error values.

Storage errors include the failed operation and the capability or limit
involved. Machine-readable facts belong in `data`; clients do not parse
`message`. The store reports a `store.*` code:
`store.io`, `store.locked`, `store.format_version`, `store.corruption`,
`store.recovery_required`, `store.limit`, `store.cursor`, `store.transaction`,
and `store.read_only`.
Backends enforce no key or value size limit, so `store.limit` is produced only
when Marrow framing cannot encode a tree-cell metadata or value-codec length
above a `u32` field.

Managed-root protection raises `write.*` codes when code attempts maintenance
work without the maintenance capability: `write.requires_maintenance` for a
whole managed-root delete. Deleting a required field on its own raises
`write.required_field` outside maintenance.

`marrow data integrity` reports `data.*` codes (kind `tooling`) for the findings
it surfaces while verifying saved data against the project schema:
`data.decode` for a stored value that is not a canonical form of its declared
type, `data.key_type` for a stored key with a scalar type the schema does not
declare, `data.incomplete` for an existing record or keyed-layer entry missing
an accepted required field, and `data.orphan` for a stored cell under a root or
member the schema no longer declares (an undecodable cell key is reported as
`store.corruption`).
`marrow evolve` reports `evolve.*` codes when a preview witness
cannot be applied exactly. A command run against a project whose `marrow.json`
is unreadable reports `io.read`; an invalid `marrow.json` reports
`config.invalid`.

## How `kind` Is Assigned

Tools derive `kind` from the first dotted segment of `code`, so the kind of a
code is stable and predictable:

| First segment | `kind` |
|---|---|
| `parse` | `parse` |
| `check`, `schema` | `check` |
| `run`, `value` | `runtime` |
| `store` | `storage` |
| `io` | `io` |
| everything else (`config`, `project`, `catalog`, `data`, `evolve`, `write`, `test`, `restore`) | `tooling` |

A `run.capability` error is the runtime form of a missing host capability; it
carries `kind` `runtime` (the `capability` kind named in the envelope section is
a category label, not a separate code prefix).

## Code Reference

Every code below is emitted by the current build. Codes are grouped by family.
The "Surface" column says where a developer first meets the code: a single-file
`check`, a project `check`/`run`/`test`, a managed write inside a running
program, the store, or a `data` maintenance command.

### `parse.*` — kind `parse`

Syntax errors from the lexer and parser. Reported by `check` (single file and
project) and by any command that parses sources before running.

| Code | Meaning |
|---|---|
| `parse.syntax` | The source is not well-formed Marrow: a bad token, a missing piece of a declaration, or an unexpected construct. The only `parse.*` code; the `message` says what was expected. |

### `check.*` — kind `check`

Static errors found while checking source. A bare single-file `check` runs the
available checker rules for that file; project checks also run module-wide rules
over every configured source and test file.

| Code | Meaning |
|---|---|
| `check.module_path` | A library file declares a module name that does not match its path. |
| `check.duplicate_module` | Two library files declare the same module name. |
| `check.multiple_scripts` | A project holds more than one file without a `module` declaration. A project may have at most one single-file script (its entrypoint); every other file must declare a `module`. |
| `check.duplicate_declaration` | A name is declared or imported more than once within a single file. |
| `check.unresolved_import` | A `use` names a module that is neither a project module nor a standard-library module. |
| `check.unknown_type` | A type annotation names a type the checker does not recognize. |
| `check.return_value` | A `return` carries a value in a function with no return type, or omits one in a value-returning function. |
| `check.missing_return` | A value-returning function can reach the end of its body without returning. |
| `check.operator_type` | An operator is applied to operands whose types it does not accept. |
| `check.condition_type` | An `if`/`while` condition is not a `bool`, or an `if const` guard is not a saved value read that can be presence-bound. |
| `check.call_argument` | A call or constructor passes the wrong number of arguments, names a parameter or key that does not exist, omits a required key, or supplies one more than once. |
| `check.return_type` | A `return` value's type does not match the function's declared return type. |
| `check.assignment_type` | A value's type does not match the typed binding or assignment target it is stored into. |
| `check.untyped_value` | A value whose type cannot be resolved (`unknown`) is stored into a concrete typed place. |
| `check.key_type` | A saved key or identity argument's type does not match the key it addresses: a scalar of the wrong type in a keyed lookup, or an identity of a foreign store root spliced into a keyspace. |
| `check.unresolved_name` | A bare name used as a value resolves to no binding in scope. |
| `check.unresolved_call` | A call names a function that is neither a builtin nor a declared function. |
| `check.private_function` | A qualified call (`module::fn`) names a function that exists but is not `pub`, so it is not callable from another module. The name resolves; the visibility does not. |
| `check.ambiguous_call` | A bare call names a `pub` function reachable in two or more modules, so the bare name cannot pick one — it must be qualified (`module::fn`). |
| `check.next_id_requires_single_int` | `nextId(^root)` names a root with no default integer allocation policy (composite identity, a non-integer key, or a keyless singleton). The static counterpart of `write.next_id_unsupported`. |
| `check.rejected_surface` | Source uses a parsed construct outside the accepted v0.1 surface, such as saved-path `inout` call arguments or old saved traversal method shapers such as `.take(...)`, `.window(...)`, and `.resume(...)`. Reserved syntax forms such as `merge`, `lock`, `out`, and `~` are parser diagnostics instead. |
| `check.catalog_intent` | Binding source against the accepted catalog cannot resolve durable identity soundly: a proposed catalog whose identities collide, a reserved spelling reused without an `evolve` intent, or an `evolve` intent that cannot carry identity forward — a rename without an accepted entry holding the new canonical path and old alias. A source declaration the accepted catalog does not yet record is informational, not an error: it reports that durable identity is not yet frozen, and running the program or applying an evolution records it. |
| `check.bare_maybe_present_read` | A maybe-present saved read appears in value position without a read-site resolution form such as `??`, `exists(...)`, `if const name = place`, optional chaining, or an attached-data traversal. A `required` declaration is a validity rule for populated records; it is not a proof that arbitrary saved data is present at this read site. |
| `check.literal_range` | A numeric literal is provably outside its type's range (an integer beyond `i64`, or a decimal outside the 34-digit / 34-place envelope). The static counterpart of the runtime numeric range faults. |
| `check.string_escape` | A string literal or interpolation text segment carries a backslash escape outside the recognized set (`\\`, `\"`, `\n`, `\r`, `\t`), or a trailing lone backslash. |
| `check.finally_control_flow` | A `finally` block lets control flow escape via `return`, `break`, or `continue`. |
| `check.loop_control_flow` | A `break`/`continue` is outside any loop, or names no enclosing loop. |
| `check.catch_type` | A `catch` annotation is not `Error`. |
| `check.throw_type` | A `throw` operand is known not to be an `Error` value. |
| `check.try_handler` | A `try` block has neither a `catch` nor a `finally` clause. |
| `check.match_requires_enum` | A `match` scrutinee is not an enum value, or names an enum the project does not declare. |
| `check.unknown_enum_member` | A `match` arm path, or an `Enum::member` reference, walks to no member the enum declares. |
| `check.duplicate_match_arm` | Two `match` arms cover the same member — a repeated arm, or a leaf already covered by an enclosing category arm. |
| `check.nonexhaustive_match` | A `match` over an enum does not cover every selectable leaf; the message names each uncovered leaf by its full path. |
| `check.ambiguous_match_arm` | A `match` arm is a bare member name that appears under more than one parent of the enum tree; the message names the qualifying paths to disambiguate. |
| `check.ambiguous_member` | A bare `Enum::member` literal (in value or `is` position) names a member that appears under more than one parent; the full path (`Enum::parent::member`) disambiguates. |
| `check.category_not_selectable` | A category enum member is named in value position; only a concrete member under it is selectable. |
| `check.is_requires_enum` | The left operand of `is` is not an enum value. |
| `check.is_type` | The right operand of `is` is not a member of the left operand's enum. |
| `check.invalid_assign_target` | An assignment target is not a writable place. |
| `check.non_constant_const` | A `const` initializer is not a constant expression. |
| `check.loop_mutates_traversed_layer` | A loop over a saved layer mutates that same layer. The static counterpart of `run.traversal`. |
| `check.neighbor_unsupported` | `next`/`prev` targets a shape with no single key level to seek: a composite-identity record or an index branch. |
| `check.range` | A range-for header is ill-formed: the endpoints are not the same steppable type, or the `by` step does not match them (a number for `int`/`decimal`, a positive duration for `date`/`instant`). `decimal` and `instant` require an explicit step; a zero step, a literal step pointing away from literal endpoints (a dead loop), a negated duration on a temporal range, or a `by` on a non-range iterable is rejected. |
| `check.range_value` | A range expression appears outside a `for` iterable. Ranges are loop shapes, not values. |
| `check.collection_unsupported` | `values` or `entries` is applied to an address-only collection, such as a non-unique index branch, that has no materialized values distinct from its keys. |
| `check.private_enum` | A cross-module enum reference names an enum that exists but is not `pub`; the enum resolves, the visibility does not. |
| `check.nesting_limit` | Source nests expressions or statement blocks deeper than the fixed parser limit (256). Raised by the parser at the offending span so pathologically nested source fails closed rather than overflowing the stack; see the [cost model](language/cost-model.md). |
| `check.evolve_target` | An `evolve` intent names an entity — a resource member, saved root, store index, enum, or enum member — that the current source does not declare (or, for a rename's source side, that the accepted catalog does not record). |
| `check.evolve_type` | An `evolve default` value does not match its target member's type, or an `evolve transform` body does not type-check. |
| `check.evolve_transform` | An `evolve transform` body is ill-formed: it is impure, reads its own target or a member another `default`/`transform` rewrites in the same block, or does not compute a top-level member as a pure function of `old`'s other decodable members. |

### `schema.*` — kind `check`

Resource-schema rules. Reported during a project check alongside `check.*`.

| Code | Meaning |
|---|---|
| `schema.duplicate_member` | A resource or enum member name collides with another member at the same level. |
| `schema.category_leaf` | A `category` enum member has no nested members, so it can never be selected or matched. |
| `schema.parent_not_category` | An enum member has nested members but is not a `category`; a grouping node must be marked `category`, since a value selects a concrete member under it. |
| `schema.duplicate_root_owner` | Two stores declare the same saved root (a cross-declaration rule the project checker reports). |
| `schema.unknown_in_saved` | A managed saved field or key is typed `unknown`; saved schemas use concrete types. |
| `schema.unsupported_type` | A parsed type spelling is only supported in a narrower declaration context, such as `map[K, V]` saved keyed-leaf member sugar. |
| `schema.key_member_collision` | A top-level field or layer shares a name with an identity key. |
| `schema.unknown_index_arg` | An index argument does not resolve to an identity key or a top-level field. |
| `schema.unorderable_key` | A saved key has a type with no order-preserving key encoding (currently `decimal`). |
| `schema.nonscalar_key` | A saved key (an identity key, a keyed-layer key parameter, or an index argument) is typed as an identity, a name, or a sequence; a key must be an orderable scalar. |
| `schema.non_enum_named_field` | A saved field has a named type that is not a declared enum; saved fields store scalars or declared enum values. |
| `schema.index_missing_identity_keys` | A non-unique index does not end with all identity keys in declaration order. |
| `schema.index_requires_keyed_root` | An index is declared on a store with no keyed root. |
| `schema.nested_index_arg` | An index argument names a field nested through an unkeyed group (not yet resolved by the write planner). |

### `catalog.*` — kind `tooling`

| Code | Meaning |
|---|---|
| `catalog.merge_conflict` | `marrow.catalog.json` or another accepted-catalog metadata section contains Git conflict marker lines. Resolve the conflict and rerun the command. |

### `run.*` — kind `runtime`

Runtime faults from the evaluator, surfaced by `run` and `test`. Deterministic
faults that the evaluator can recover from are raised as catchable `Error`
values: arithmetic faults, decimal envelope failures, recoverable type and
parse/range failures from builtins, and assertions keep their specific `run.*`
code. Runtime backstops for
unchecked or internal states, control-flow invariants, missing host
capabilities, unsupported constructs, storage failures, and traversal
conflicts are fatal runtime errors rather than catchable `Error` values. A
catchable fault that reaches the top of the program is reported under its own
code, except `run.uncaught_error` — see "Typed Errors In Running Programs".

| Code | Meaning |
|---|---|
| `run.type` | A value was used where another type was required. Recoverable builtin/evaluator type faults are catchable; unchecked internal type backstops can be fatal. |
| `run.unbound_name` | A name was read or assigned that is not bound in scope. Fatal runtime backstop for unchecked programs. |
| `run.overflow` | Integer arithmetic overflowed the 64-bit range. |
| `run.decimal_overflow` | Decimal arithmetic exceeded the 34-digit / 34-place envelope. |
| `run.divide_by_zero` | Integer division or remainder by zero. |
| `run.no_enclosing_loop` | A `break`/`continue` reached the top of a function with no loop to target. Fatal runtime control-flow backstop. |
| `run.unknown_function` | A call named a function the program does not declare. Fatal runtime backstop for unchecked programs. |
| `run.ambiguous_function` | A bare run entry name matched more than one public function. Qualify the entry as `module::function`. |
| `run.private_function` | A qualified call or run entry reached a function that exists but is not `pub` to the caller. The runtime backstop for `check.private_function`. |
| `run.no_value` | A call to a function that returns no value was used where a value is needed. Fatal runtime backstop for unchecked programs. |
| `run.absent_element` | A value-position absence in a read-site resolution form is catchable as an `Error` when the form is defined to catch it. Ordinary maybe-present reads must be resolved at the read site (`??` / `if exists` / `if const` / `?.`) or are compile errors. A missing required field discovered while materializing an already fixed saved address is fatal invalid attached data and is not hidden by `??` or `catch`. |
| `run.store` | The store reported an error (e.g. corrupt tree-cell payload) during a read. Fatal storage/backend failure while evaluating a read. |
| `run.unsupported` | A construct the runtime does not evaluate. Fatal runtime backstop. |
| `run.capability` | A host capability a builtin needs (e.g. the clock for `std::clock::now`) was not provided to this run. Fatal host/tooling failure. |
| `run.assertion` | A `std::assert::*` assertion did not hold. `marrow test` reports these as located test failures. |
| `run.uncaught_error` | An `Error` raised by `throw` reached the top of a function with no `catch`. The original code travels in the message (e.g. `[io.read]`). |
| `run.traversal` | A write, delete, or append changed the saved layer a loop was actively traversing. Fatal dynamic counterpart of `check.loop_mutates_traversed_layer`. |
| `run.recursion_limit` | Function-call nesting exceeded the fixed call-depth budget (256). Located at the offending call site and reports the budget plus observed attempted depth, so runaway or unbounded recursion fails closed rather than overflowing the stack; see the [cost model](language/cost-model.md). |
| `run.no_entry` | `marrow run` found no entry: no `--entry` was given and `marrow.json` sets no `run.defaultEntry`. |
| `run.store_evolved` | The store was stamped at a catalog epoch newer than this program accepted, so a newer binary evolved it. Recompile or upgrade against the current accepted catalog. Fenced before any execution; the store is unchanged. |
| `run.store_behind` | The store was stamped at a catalog epoch older than this program accepted, so its data predates the catalog. Run `marrow evolve apply` to activate the store first. Fenced before any execution; the store is unchanged. |
| `run.schema_drift` | The store was stamped under a different schema at the same catalog epoch: its recorded source digest does not match the durable shape this binary expects. Run `marrow evolve preview` to inspect the required repair or `marrow evolve apply` to activate it. Fenced before any execution; the store is unchanged. |
| `run.engine_profile` | The store's engine profile does not match this binary's storage layout. Fenced before any execution; the store is unchanged. |
| `run.store_unstamped` | The store holds saved records but carries no catalog activation stamp. Run `marrow check --data` and `marrow evolve apply` to activate the accepted catalog before running. Fenced before any execution; the store is unchanged. |

### `value.*` — kind `runtime`

Value codec range faults raised while formatting or writing runtime values.
These are catchable `Error` values inside a running program.

| Code | Meaning |
|---|---|
| `value.range` | A date or instant lies outside Marrow's supported calendar range, years 0001-9999. |

### `write.*` — kind `tooling`

Managed-write faults raised by the write planner inside a running program. They
surface to `run`/`test` as `Error` values, so code can catch them; an uncaught
one is reported under its own `write.*` code.

| Code | Meaning |
|---|---|
| `write.required_absent` | A required field was absent in a whole-resource write. |
| `write.type_mismatch` | A field value's type does not match the resource schema. |
| `write.identity_mismatch` | The supplied identity keys do not match the store root's identity shape. |
| `write.store` | The store reported an error during a write. |
| `write.unknown_field` | A field write names a field the resource does not declare. |
| `write.unique_conflict` | A unique index already maps the supplied key(s) to a different identity. |
| `write.unknown_layer` | A keyed-layer write names a layer the resource does not declare. |
| `write.not_a_leaf_layer` | A keyed-leaf write targets a group layer. |
| `write.not_a_group_layer` | A group-entry field write targets a leaf layer. |
| `write.layer_key_arity` | A keyed-layer write supplies the wrong number of layer keys. |
| `write.id_overflow` | The integer key space is exhausted (`i64::MAX`), so no next identity or position can be allocated. |
| `write.next_id_unsupported` | `nextId` was asked for a root whose identity shape has no default integer allocation policy. The runtime backstop for `check.next_id_requires_single_int`. |
| `write.required_field` | Deleting a `required` field on its own is rejected outside maintenance. |
| `write.requires_maintenance` | A whole managed-root delete (`delete ^books`) was attempted without the maintenance capability. |

### `store.*` — kind `storage`

Store faults. The tree-cell facade produces `store.corruption` for malformed
tree-cell metadata, value codecs, index cells, or accepted catalog rows. A
persistent backend can also produce the I/O, locking, format, corruption,
recovery, limit, and read-only variants. Opening a damaged native store fails
closed with a typed code — never a process crash: a truncated or torn body is
`store.corruption`, and a store left needing repair by an unclean shutdown is
`store.recovery_required`. A store fault met during a program read or write
travels as `run.store` or `write.store`; data tooling reports the `store.*` code
directly.

| Code | Meaning |
|---|---|
| `store.io` | An I/O operation on a persistent backend failed. |
| `store.locked` | The store file is held open by another process (a writer or a read-only inspection). Close the other process, then retry. |
| `store.format_version` | The store's recorded format version is not the one this build supports. |
| `store.corruption` | The store file, tree-cell metadata, tree-cell index cell, or accepted catalog table is corrupt and could not be opened or decoded — including a truncated or torn store body and a catalog snapshot whose recomputed digest does not match its stored header. |
| `store.recovery_required` | The store was not shut down cleanly, so a read-only open is refused until a write-capable open replays the interrupted commit. Run `marrow data recover` to attempt that open. The recovery is attempted, not guaranteed: the command reports whether the store opened, and a store damaged beyond replay surfaces `store.corruption`. |
| `store.limit` | A Marrow framing layer could not encode a tree-cell metadata or value-codec length above a `u32` field. Backends enforce no key/value size limit. |
| `store.cursor` | A bounded scan cursor does not belong to the scan being resumed. |
| `store.transaction` | A transaction or snapshot operation was requested in an invalid store state. |
| `store.read_only` | A write-capability operation was requested through a read-only store handle. |

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
| `data.key_type` | A stored record key, keyed-layer key, or identity payload key has a scalar type the schema does not declare for that key position (e.g. a string key under an `int` identity). |
| `data.incomplete` | An existing record or keyed-layer entry is missing an accepted required field. JSON and JSONL include `store_catalog_id`, `record_identity`, `parent_path`, and `missing_member_catalog_id`; `source_span.path` is display-only. |
| `data.orphan` | A stored data cell is under a saved root or member the schema no longer declares; integrity reports repair guidance for source-native evolution or maintenance repair. Derived index cells are never flagged. An actual stored cell whose key does not decode under the tree-cell key grammar is reported as `store.corruption`. |

### `evolve.*` — kind `tooling`

Source-native data-evolution preview/apply faults.

| Code | Meaning |
|---|---|
| `evolve.no_accepted_catalog` | Apply was run on a project that declares no saved data, so there is no baseline catalog epoch to advance from. |
| `evolve.repair_required` | The attached data snapshot cannot discharge a required obligation. Repair the data through explicit maintenance/admin code, then run `marrow evolve preview` again. |
| `evolve.drift` | The live source, catalog, store snapshot, engine metadata, affected IDs, or counts no longer match the preview witness. Rerun `marrow evolve preview`, then rerun `marrow evolve apply`. |
| `evolve.store_commit_drift` | The store commit changed after preview. Rerun `marrow evolve preview`, then rerun `marrow evolve apply`. |
| `evolve.catalog_drift` | The store's accepted catalog snapshot changed after preview, so the witness was discharged against a catalog the store no longer holds. Apply refuses before writing; rerun `marrow evolve preview`, then rerun `marrow evolve apply`. |
| `evolve.maintenance_required` | A destructive retire was reached without the maintenance gate. |
| `evolve.approval_required` | A destructive retire needs an approval naming the catalog ID and populated count from preview. |
| `evolve.approval_mismatch` | The supplied destructive approval did not match the exact preview witness. |
| `evolve.plan_mismatch` | Apply could not write the exact number of effects the witness counted. |
| `evolve.transform_faulted` | A checked transform body faulted while running against real data, so apply rolled back. |

### `test.*` — kind `tooling`

| Code | Meaning |
|---|---|
| `test.none` | `marrow test` found no tests; check the `tests` patterns in `marrow.json`. Exit code `1`. (Failing tests are reported per test with their own `run.assertion` or other `run.*` code, not a `test.*` code.)|

### `backup.*` — kind `tooling`

| Code | Meaning |
| --- | --- |
| `backup.store_uid_missing` | The existing store predates the physical store UID stamp. Run or evolve apply with this build to stamp the store before backup. |

### `restore.*` — kind `tooling`

Faults from `marrow restore` when a backup cannot be replayed into the project's
store. `marrow backup` reports `io.write` for a file it cannot write, a
`store.*` code for a read fault, or `backup.store_uid_missing` when an existing
store predates the required physical store UID stamp.

| Code | Meaning |
|---|---|
| `restore.format_version` | The file is not a Marrow backup, or its format version is not the one this build restores. |
| `restore.corrupt_chunk` | The backup's cell stream is truncated or its data checksum does not match the manifest. |
| `restore.not_empty` | The target store already holds saved data, generated indexes, or an accepted catalog. v0.1 restore writes into an empty store only. |
| `restore.engine_recompile_required` | The backup was written under a different engine, layout, or value codec. A cross-engine restore is a future engine recompile. |
| `restore.source_mismatch` | The backup was written from a program whose schema does not match this project. The message prints backup source digest and project source digest. |
| `restore.catalog_mismatch` | The backup's catalog does not match this project's accepted catalog. The message prints backup catalog epoch/digest and project catalog epoch/digest. |
| `restore.data_invalid` | The replayed data does not validate against the project schema, including orphaned managed cells; the restore was rolled back. |

## Typed Errors In Running Programs

In `.mw` code an error is an `Error` value with its own dotted `code`, raised by
`throw` and caught by `catch`. Builtins, managed writes, and deterministic
runtime faults raise typed errors too when the fault is recoverable: a failed
`std::io::readText` raises `io.read`, a rejected write raises a `write.*` code,
arithmetic raises specific numeric `run.*` codes, and value range failures raise
`value.*` codes. These typed raises are catchable in code. Fatal runtime
backstops for unchecked/internal states and host/tooling failures are not
`Error` values and can surface at the top level under their own `run.*` code.
When a language `throw` or `std::io` error is *not* caught and reaches the top of
the program, `run`/`test` report it as `run.uncaught_error` and carry the
original code in the message, for example:

```
run.uncaught_error: uncaught error [io.read]: std::io::readText failed for `/no/such/file`: No such file or directory (os error 2)
```

## Deferred Surfaces

`marrow data diff`/`data load`, non-empty restore modes, and cross-engine restore
are deferred — see [future/data-tools.md](future/data-tools.md) and
[future/cli.md](future/cli.md). No active command-output code family appears for
a deferred surface until that surface ships.

The `decode.*` family is reserved for future checked decode and repair reports.
These codes do not appear in v0.1 command output.

| Code | Reserved meaning |
|---|---|
| `decode.shape` | A stored tree shape does not match the checked resource shape. |
| `decode.unknown_member` | Stored data names a member the checked catalog cannot resolve. |
| `decode.required_absent` | A required saved member is absent from stored data. |
| `decode.value` | Stored bytes do not decode as the checked leaf type. |
