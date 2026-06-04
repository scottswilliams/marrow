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
A store persists typed tree cells over ordered bytes.
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
| Store | Persist typed tree cells and byte values. |
| Tools | Inspect source, schemas, saved trees, and errors. |

Anything that needs field names, types, indexes, history, data evolution, or
repair belongs above the store. The private engine substrate only needs ordered
tree-cell bytes.

## Source Pipeline

Marrow source follows one direct path:

1. discover project configuration and source roots;
2. parse `.mw` files as Marrow source;
3. match module declarations to source-root-relative paths;
4. resolve imports and names;
5. build resource schemas and source metadata;
6. check types, effects, durable places, and capabilities;
7. hand a checked program to runtime and tools.

The checked program contains the module graph, function signatures, schemas,
type facts, effect facts, durable-place facts, capability needs, and source
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

Public entrypoints are ordinary `pub fn` declarations. Qualified entry names
select one module exactly; bare entry names are accepted only when one public
function has that name. The CLI or host decodes boundary arguments before Marrow
code runs.

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

Identity keys are part of typed store identity. They are not stored fields.
Typed code addresses saved data through checked durable places, not raw key
tuples.

Schemas come from source. Tools may cache compiled metadata, but the cache is
not the source of truth.

## Saved Tree

Saved data is a typed tree backed by tree-cell keys and byte values:

```text
^books(id).title = "Small Gods"
```

A durable place has a store catalog ID, identity keys, member catalog IDs, and
the cell family it addresses. Source names and public path text are not physical
storage identity.

Saved resources are not opaque byte dumps. A resource is stored as typed nodes,
leaves, sequences, indexes, and metadata cells, so inspection,
traversal, backup, and repair all see the same tree shape.

Logical `.mw` places are lowered before they reach the store:

```text
^books(id).title
^books.byShelf("fiction", id)
```

Both examples are source-level renderings. The store receives catalog-backed
cell keys derived from checked facts, not source strings.

The encoding belongs to Marrow. It preserves Marrow order, records cell family
and typed key identity, and prevents collisions between data, indexes,
sequences, and metadata cells.

Ordinary `.mw` code uses managed resources. Tooling, backup, and repair use
typed tree-cell APIs and checked/catalog facts.

## Values And Order

Saved values use Marrow validation at the boundary. Primitive values have
canonical saved forms so backup, diff, traversal, equality, and restore do not
depend on the selected backend.

Strings are UTF-8. Bytes remain bytes. Booleans, numbers, dates, instants,
durations, errors, and generated identities have stable encodings.

Absence is not stored as null. An unpopulated field has no leaf cell.
Managed saved fields and keys reject `unknown`.

Tree-cell keys preserve Marrow order. That one ordering rule supports traversal,
generated indexes, backup, restore, editor live reads, and portable diffs.

Within a declared typed layer, keys order by their type.

## Managed Writes

A managed saved write is planned above the store engine:

1. resolve the checked durable place and resource schema;
2. validate keys and values;
3. read old values needed by indexes or required-field checks;
4. check unique indexes;
5. write the resource value or field;
6. maintain generated index entries;
7. commit the plan or roll it back.

Single-record writes do not require user-written transactions. If the selected
store cannot make the planned write coherent, Marrow reports a capability
error instead of partially applying it.

Use `transaction` when several saved changes form one application invariant,
such as a record plus an audit entry, several related resources, or a delete
plus cleanup work.

Whole-resource assignment replaces the managed resource tree for one identity.
Field writes change existing resources. `delete` removes a value or subtree and
maintains generated indexes. Source-level `merge` is not part of v0.1; use
explicit checked writes or a future checked transform.

Managed writes are planned before they commit, and generated index maintenance is
part of the same plan. This protects indexes, history layers, and required
fields from accidental corruption.

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

Standard modules do not bypass managed saved writes. They do not own backend
connections, parse `.mw`, inspect schemas, maintain indexes, or expose
backend-specific storage APIs.

The first release standard library stays narrow: clock, text, bytes, math,
file IO, environment, assertions, and logging.

## Backend Contract

Every store provides the same typed tree-cell operations over a private
ordered-byte engine:

- read and write exact typed cells;
- delete exact leaf, sequence, and index cells;
- scan exact index tuples with bounded pages;
- record catalog epoch, layout epoch, engine profile digest, and commit
  metadata;
- open native stores read-only without writer capability;
- report stable typed errors.

A persistent local backend also owns its data directory, format checks, and
local locking. Those details stay below the contract.

The private engine substrate passes conformance tests for exact reads and
writes, prefix deletes, bounded scans, cursor-resumed scans, transactions,
rollback, nested savepoints, and errors.

## Native Store

Native storage is the normal local project store. It is opened by the CLI,
runtime, or optional `marrow serve`.

The native store stays small:

- one normal writable owner per data directory;
- ordered byte keys for private tree-cell storage;
- persistent commits for completed writes;
- bounded scans and typed limit errors;
- a recorded format version.

A native data directory holds a single `marrow.redb` file in the local engine
format, not a typed export artifact. There is no separate lock file: the engine
takes an advisory lock on `marrow.redb` itself, so a second writer for the same
directory surfaces as a `store.locked` error rather than leaving a sidecar behind.

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

Portability is typed tree-cell data plus the source and catalog facts needed to
interpret it. Backup and restore must account for catalog IDs, typed values,
generated index trees, sequence state, and engine-profile metadata.
Derived structures are verified or rebuilt from primary resources before a
restore publishes data.

Raw engine byte streams are not the production portability contract.

Import modes are deferred — see [future/cli.md](future/cli.md).

Backend-native files can support fast local snapshots, but they are not typed
export artifacts.

## Data Evolution And Maintenance

Schemas evolve through source changes and explicit data-evolution work. For
v0.1, rename tooling uses explicit source paths and maintenance code. Durable
rename identity belongs to future catalog work.

Marrow does not guess data movement. If a change moves data, populates a new
required field, rebuilds an index, or changes identity, that work is explicit,
inspectable, and recoverable.

Data-evolution work is source-native preview/apply over checked catalog and
store facts. Tool/admin maintenance runs may grant repair code the capabilities
ordinary source syntax cannot request. There is no separate migration DSL and no
hidden history ledger in the database kernel.

Maintenance mode is selected by tools, not ordinary application code. A
maintenance run names the roots it may change. It can rebuild indexes, delete
whole roots, or repair data that fails schema validation through explicit
managed writes.

## Tools And Server

Tools inspect the same source, schema, and saved tree model that programs use.
There is no private admin database.

Inspection uses typed resources and checked/catalog facts. `marrow data`
provides read-only `roots`, `stats`, `dump`, `integrity`, and `get` commands
over the typed tree-cell store. It does not expose backend traversal, physical
keys, or archive replay as production APIs. `diff`, `load`, and typed
backup/restore are separate tooling contracts (see
[future/data-tools.md](future/data-tools.md)).

`marrow backup` and `marrow restore` are typed backup/restore. A backup is a
manifest plus the canonical tree-cell data stream, not a raw engine-byte copy; the
manifest binds the data to the source digest, accepted catalog epoch, engine
profile, and value-codec version it was written under. The generated indexes are
derived, so the stream omits them and restore rebuilds them from the data. Restore
validates that binding and the data against the schema, then replays into an empty
store in one transaction. Backups are deterministic and portable across conforming
backends at the same layout and codec, but byte identity requires matching accepted
catalog facts, engine profile, value codec, and stored data.

`marrow lsp` is the editor language server: JSON-RPC over stdio with
`Content-Length` framing. It tracks open documents with full text sync and
publishes diagnostics; today those are parse diagnostics, with hover and
project-level (checked-fact) diagnostics to follow. It is distinct from
`marrow serve` below — a different protocol for a different purpose.

`marrow serve` is optional. Normal commands may open a project store directly.
The server is useful when several local tools need one long-lived owner for a
persistent backend, live reads, or local-session inspection.

The server protocol is newline-delimited JSON over a loopback TCP connection
(`127.0.0.1`); the bound address is printed on startup. It is a small,
read-only debug/admin inspection surface. Path-addressed operations validate
against checked saved facts before reading. The operations are:

- list stored roots with `debug_data_roots`;
- list child keys with `debug_data_children`;
- read an exact typed data query with `debug_data_get`;
- walk a bounded typed data subtree with `debug_data_walk`.

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

Storage errors name the failed operation and the capability or limit involved.
Backend-specific messages remain plain operator text, not the machine contract.

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
program facts in runtime and tools, typed tree-cell operations below.

## Non-Goals

Marrow does not grow a second storage query language, hidden object store, ORM
layer, SQL-style migration subsystem, implicit async syntax, required background
service, web framework, remote database product, built-in users-and-roles
system, or backend-specific application APIs.
