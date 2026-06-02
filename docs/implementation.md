# Implementation And Backends

Marrow is a small language with a built-in database model. The model is typed
trees.

A tree can be local to one run, or it can be saved under a root such as
`^books`. The language does not change when storage changes.
Durable saved data stays under Marrow's language and tooling contract regardless
of which storage engine holds the bytes.

Language behavior lives in [`language/`](language/). This page describes the
runtime, tooling, and backend shape that make that language work.

## The Main Idea

```text
A resource is a typed tree.
A saved resource is a typed tree stored under ^root.
An index is a generated lookup tree.
A backend stores ordered paths and bytes.
Marrow owns the meaning of durable data.
```

Marrow owns the language. Backends own durable ordered storage.

A backend does not parse `.mw`, understand resource fields, maintain indexes,
plan data evolution, or expose backend-specific application APIs. Those jobs
belong to Marrow.

Native storage is the normal local project store. Other storage engines are
outside the default product.

## Kernel

Marrow has a small kernel:

| Part | Responsibility |
|---|---|
| Source | Parse, format, resolve, and check `.mw`. |
| Schema | Turn resources into typed tree shapes. |
| Runtime | Evaluate checked code and plan saved writes. |
| Store | Persist ordered paths and byte values. |
| Tools | Inspect source, schemas, saved trees, and errors. |

Anything that needs field names, types, indexes, history, data evolution, or
repair belongs above the store. Anything that only needs ordered paths and bytes
belongs in the store.

## Source Pipeline

Marrow source follows one direct path:

1. discover project configuration and source roots;
2. parse `.mw` files as Marrow source;
3. match module declarations to source-root-relative paths;
4. resolve imports and names;
5. build resource schemas and source metadata;
6. check types, effects, saved paths, and capabilities;
7. hand a checked program to runtime and tools.

The checked program contains the module graph, function signatures, schemas,
type facts, effect facts, logical saved paths, capability needs, and source
spans.

Marrow has one `fn` form. A function may be effectful, return a value, or
both. The runtime does not need a separate procedure construct.

The runtime does not re-parse source. The store never receives source facts.
Tools use the same checked facts for diagnostics, hover, inspect output,
generated docs, repair, and maintenance workflows.

## Project Model

A Marrow project is source plus explicit storage selection.

The shared project file is `marrow.json`. It stays small enough for the CLI,
LSP, and editor integrations to agree on.

```json
{
  "sourceRoots": ["src"],
  "run": { "defaultEntry": "shelf::sample::main" },
  "store": { "backend": "native", "dataDir": ".marrow/data" },
  "tests": ["tests/**/*.mw"]
}
```

Project configuration contains project choices:

- source roots;
- default entrypoints;
- the selected store backend;
- a native data directory;
- test file patterns.

It does not contain compiled schemas, hidden index metadata, data-evolution history,
permissions, backend application APIs, connection strings, or secrets.

Library files declare modules explicitly. A module-less `.mw` file is a script
or entrypoint, not an importable library module.

Within a source root, `shelf::books` is found at `shelf/books.mw`, and the
declaration in that file must match the path.

Public entrypoints are ordinary `pub fn` declarations selected by qualified name.
The CLI or host decodes boundary arguments before Marrow code runs.

Test files are the project's `tests` patterns. They live outside the source
roots and are scripts, so each is named from its project-relative path
(`tests/books_test.mw` → `tests::books_test`). `marrow test` runs every `pub fn`
with no parameters in a test file against a fresh in-memory store, so tests are
independent and never touch saved data. A `tests` pattern is the directory walk
of its base: `tests/**/*.mw` walks `tests/` for `.mw` files.

## Resources And Schemas

A resource declaration compiles to a schema:

- saved root, if any;
- identity keys;
- fields and child layers;
- required and sparse fields;
- indexes and unique indexes;
- keyed history layers;
- documentation comments and source spans.

The same schema checks local values and saved values. This is the main
simplification in Marrow: users learn one tree shape, then decide whether that
shape is local or saved.

Resources are schema declarations. They are not service APIs, hidden objects,
or function containers.

One saved root has one managed owner. If a store owns `^books`, another store
cannot claim `^books` with a different shape.

Identity keys live in the saved path. They are not stored fields. Typed code
addresses saved data through the store identity type, not raw key tuples.

Schemas come from source. Tools may cache compiled metadata, but the cache is
not the source of truth.

## Saved Tree

Saved data is an ordered tree of paths and byte values:

```text
^books(id).title = "Small Gods"
```

A saved path has a root, keyed layers, fields, and index branches. A path may
have a value, children, both, or neither.

Saved resources are not hidden blobs. A resource is stored as declared fields
and layers below its identity path, so inspection, traversal, backup, and
repair all see the same tree shape.

Logical `.mw` paths are lowered before they reach a backend:

```text
^books(id).title
^books.byShelf("fiction", id)
```

Both paths start at the saved root `books`. Resource fields, keyed layers, and
index names become encoded path segments below that root. The byte layout is
not part of `.mw`.

The encoding belongs to Marrow. It preserves Marrow order, records segment
kinds, and prevents collisions between record keys, fields, layers, and index
names.

Raw saved-tree access exists for import, export, data evolution, repair, and tools.
Ordinary `.mw` code uses managed resources.

## Values And Order

Saved values use Marrow validation at the boundary. Primitive values have
canonical saved forms so backup, diff, traversal, equality, and restore do not
depend on the selected backend.

Strings are UTF-8. Bytes remain bytes. Booleans, numbers, dates, instants,
durations, errors, and generated identities have stable encodings.

Absence is not stored as null. An unpopulated field has no value at that path.
Managed saved fields and keys reject `unknown`.

The backend returns child keys in Marrow order. That one ordering rule supports
traversal, generated indexes, backup, restore, editor live reads, and portable
diffs.

Within a declared typed layer, keys order by their type. Raw inspection uses
the stable encoded segment order.

## Managed Writes

A managed saved write is planned above the backend:

1. resolve the resource schema for the saved path;
2. validate keys and values;
3. read old values needed by indexes or required-field checks;
4. check unique indexes;
5. write the resource value or field;
6. update generated index entries;
7. commit the plan or roll it back.

Single-record writes do not require user-written transactions. If the selected
store cannot make the planned write coherent, Marrow reports a capability
error instead of partially applying it.

Use `transaction` when several saved changes form one application invariant,
such as a record plus an audit entry, several related resources, or a delete
plus cleanup work.

Whole-resource assignment replaces the managed resource tree for one identity.
Field writes update existing resources. `delete` removes a value or subtree and
updates generated indexes. Source-level `merge` is not part of v0.1; use
explicit checked writes or a future checked transform.

Managed roots reject raw writes unless a tool enters explicit maintenance
mode. This protects indexes, history layers, and required fields from
accidental corruption.

## Runtime Consistency

Marrow makes these guarantees to ordinary code:

- one managed write is internally coherent;
- a transaction groups saved writes and generated index writes;
- reads inside a transaction see earlier saved writes from that transaction;
- normal reads do not expose half-applied managed transactions;
- `return`, `break`, and `continue` leave a transaction by committing it;
- an escaping error leaves a transaction by rolling it back;
- caught errors are ordinary control flow;
- local variables, output, and host effects are not rolled back by a
  saved-data transaction.

Nested transactions are savepoints. An inner rollback can be caught by outer
code, and the outer transaction can continue.

Source-level `lock` is not part of v0.1. Backend writer coordination is an
implementation concern, not an accepted source construct.

`nextId(...)` is runtime policy over a keyed store root. The default policy
covers a store with one `int` identity key. `append(path, value)` allocates
the next positive integer child below that path and does not fill holes.

## Standard Library Boundary

The standard library is ordinary Marrow modules plus a small host capability
table.

Pure modules are deterministic helpers over Marrow values. Host modules such
as clock, file IO, environment, and logging declare the capabilities they
need. A command or embedding either provides those capabilities or reports a
typed capability error.

Standard modules do not bypass managed saved paths. They do not own backend
connections, parse `.mw`, inspect schemas, maintain indexes, or expose
backend-specific storage APIs.

The first release standard library stays narrow: clock, text, bytes, math,
file IO, environment, assertions, and logging.

## Backend Contract

Every backend provides the same ordered-tree operations over encoded saved
paths:

- read the exact value at a path;
- write the exact value at a path;
- delete a value or bounded subtree;
- report whether a path has a value, children, both, or neither;
- return child keys in Marrow order;
- scan a bounded subtree in Marrow order;
- list saved roots;
- report stable typed errors.

A persistent local backend also owns its data directory, format checks, and
local locking. Those details stay below the contract.

Backends pass conformance tests for values, presence, ordering, scans,
deletes, roots, replay, limits, and errors.

## Native Store

Native storage is the normal local project store. It is opened by the CLI,
runtime, or optional `marrow serve`.

The native store stays small:

- one normal writable owner per data directory;
- ordered byte keys for roots and encoded path segments;
- persistent commits for completed writes;
- bounded scans and typed limit errors;
- a recorded format version.

A native data directory holds a single `marrow.redb` file in the local engine
format, not the portable archive format. There is no separate lock file: the
engine takes an advisory lock on `marrow.redb` itself, so a second writer for the
same directory surfaces as a `store.locked` error rather than leaving a sidecar
behind.

## Capability Profiles

A capability profile is the set of storage promises Marrow can rely on while
checking or running code.

| Profile | Use |
|---|---|
| Minimal | Static portability checks without opening a store. |
| Memory | REPLs, tests, and short runs. |
| Native | Normal local project data. |

Code checks capabilities, not backend names. Backend names are configuration
and operator vocabulary.

Some capabilities come from a backend. Others are provided by the Marrow
runtime over a simpler backend. Backend atomic transactions are useful, but
the language-level transaction contract belongs above the backend.

Optional capabilities include snapshot reads, durable sync, advisory locks,
safe ID allocation, streaming scans, reverse scans, safe reservation for
unique index entries, and documented key/value limits.

## Adapter Boundary

An adapter for another storage engine must implement the same backend contract
as native storage. It may translate Marrow's ordered tree into tables,
documents, keys, or engine records internally.

Adapters do not add language features. They do not expose engine queries,
engine collation, engine schema catalogs, triggers, stored procedures, or
backend-specific application APIs as Marrow syntax or builtins.

Specific database adapters are not part of the default release. They belong in
separate packages when they are useful enough to maintain.

## Portability

The portable form of saved data is an ordered path/value stream plus a small
manifest. The stream uses Marrow canonical paths, not backend-native files,
table rows, or engine keys.

With source present, tools can render paths as typed resources. Without
source, tools can still restore or inspect the raw tree.

Normal backups include generated index trees. Typed restore can verify them
against primary resources or rebuild them when source is available.

Normal restore writes into an empty target. Non-empty restore modes are deferred
— see [future/cli.md](future/cli.md).

Backend-native files can support fast local snapshots, but they are not the
portable archive format.

## Data Evolution And Maintenance

Schemas evolve through source changes and explicit data-evolution work. For
v0.1, rename tooling uses explicit source paths and maintenance code. Durable
rename identity belongs to future catalog work.

Marrow does not guess data movement. If a change moves data, populates a new
required field, rebuilds an index, or changes identity, that work is explicit,
inspectable, and recoverable.

Data-evolution work is ordinary Marrow code. A tool runs a named `fn` in
explicit maintenance mode when it needs maintenance capabilities. There is no
separate migration DSL and no hidden migration ledger in the database kernel.

Maintenance mode is selected by tools, not ordinary application code. A
maintenance run names the roots it may change. It can opt into raw writes
under managed roots, rebuild indexes, delete whole roots, or repair bytes that
fail schema validation.

## Tools And Server

Tools inspect the same source, schema, and saved tree model that programs use.
There is no private admin database.

Inspection has two modes:

- typed inspection when project source is available;
- raw tree inspection when only saved data is available.

`marrow data` is the raw saved-tree command group for inspection, dump, diff,
load, integrity checks, and stats. Today it provides `marrow data roots` (list
the saved roots), `marrow data stats` (count saved roots and records), `marrow
data dump` (print every stored path/value in encoded order — the same canonical
stream backup writes), `marrow data integrity` (verify every stored value
decodes as a canonical form of its declared schema type, reporting decode
mismatches as `data.decode`, stale or foreign data as `data.orphan`, and a
corrupt key as `store.corrupt_path`; it exits `1` when it finds a problem), and
`marrow data get <path>` (read one path's value). Inspection is read-only and
never creates the store; `dump`/`get` need only `marrow.json`, while
`integrity` typechecks against the project's checked schema. `diff` and `load`
are deferred (see [future/data-tools.md](future/data-tools.md)).

`marrow backup <projectdir> <archive>` writes the store's whole saved tree to a
portable archive — the canonical ordered (path, value) stream behind a small
manifest (format magic, version, and record count), not an engine file.
`marrow restore <projectdir> <archive>`
replays one into an empty store in a single transaction; a non-empty target fails
with `restore.not_empty`, since restoring over existing data is an explicit
maintenance action. Empty-target restore is the only mode implemented today;
non-empty restore modes are deferred (see [future/cli.md](future/cli.md)).

`marrow lsp` is the editor language server: JSON-RPC over stdio with
`Content-Length` framing. It tracks open documents with full text sync and
publishes diagnostics; today those are parse diagnostics, with hover and
project-level (checked-fact) diagnostics to follow. It is distinct from
`marrow serve` below — a different protocol for a different purpose.

`marrow serve` is optional. Normal commands may open a project store directly.
The server is useful when several local tools need one long-lived owner for a
persistent backend, live reads, or local-session inspection.

The server protocol is newline-delimited JSON over a loopback TCP connection
(`127.0.0.1`); the bound address is printed on startup. It is a small, read-only
inspection surface. The operations are the saved-tree reads:

- list saved roots with `saved_roots`;
- list child keys with `saved_children`;
- read an exact saved path with `saved_get`;
- walk a bounded saved subtree with `saved_walk`.

Two read-only extensions are planned for later, not in the first release:
evaluating one checked, non-mutating query in a session and returning its typed
result, and registering a session so a client can publish its own in-memory
trees for read-only inspection. Neither would mutate saved data.

The protocol never writes managed roots: it is a read-only inspection surface.
Managed data changes come only from checked Marrow execution — `marrow run` or an
embedded runtime — and from explicit repair, restore, data-evolution, and store
maintenance workflows, never from the serve protocol. That read-only guarantee is
what lets serve be a long-lived shared owner of the store.

Loopback TCP is available for clients that cannot use local IPC. Binding TCP
beyond loopback is an explicit operator choice and requires authentication and
transport security.

The protocol is a tooling surface, not Marrow's application API. Application
APIs are written in Marrow code.

## Errors And Limits

Marrow reports parser, checker, usage, runtime, storage, and protocol failures
as typed errors. CLI and server output preserve stable codes and structured
data.

Storage errors name the failed operation, the safe saved path or prefix when
one can be shown, and the capability or limit involved. Backend-specific
messages remain plain operator text, not the machine contract.

Bounds are part of the design. Tool reads are bounded or paged. Backends may
advertise key and value limits. A small bounded implementation is better than
an unbounded one that is hard to reason about.

## Security Model

Marrow does not include a database users-and-roles system in ordinary `.mw`.
The normal security boundary is the host process, filesystem or backend
credentials, and the selected transport.

Local CLI commands use the current user's access to project source and data.
Remote server transport requires explicit authentication and transport
security before it leaves loopback.

Application authorization belongs in application data and application code. It
is not a hidden backend permission layer and it is not stored in `marrow.json`.

## Extensions

Extensions may import data, export data, bridge host systems, or implement the
backend contract in a separate package. They do not define the Marrow language
or the default storage model.

The core project keeps extension boundaries small: typed source in, checked
program facts in runtime and tools, ordered path/value operations below.

## Non-Goals

Marrow does not grow a second storage query language, hidden object store, ORM
layer, SQL-style migration subsystem, implicit async syntax, required background
service, web framework, remote database product, built-in users-and-roles
system, or backend-specific application APIs.
