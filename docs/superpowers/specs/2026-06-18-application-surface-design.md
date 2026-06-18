# Application Surfaces

- Date: 2026-06-18
- Status: Propose-first design. This touches language and
  application-boundary semantics, so implementation requires `docs/language/`
  and checked-facts docs to move in lockstep.
- Scope: public application API foundation for Marrow, not an HTTP server
  implementation.

## Summary

Marrow exposes application data through a source-declared `surface`: a public
application contract over a durable store. A store models durable truth. A
surface models the data shape and generated operations clients can use.

The goal is to remove CRUD boilerplate where the compiler has enough checked
facts to do it safely, without exposing raw saved paths or turning every public
API into hand-written functions. Custom behavior remains ordinary `pub fn`
Marrow code; surfaces generate the boring typed boundary around common reads and
writes.

Routes, JSON, TypeScript clients, identity codecs, paging, errors, and cache
facts are renderers over checked surface facts. They are not separate semantic
owners.

This design supersedes the earlier `serve` URI-tree proposal. Do not implement
`serve` as a separate public language surface.

## Design Principle

Marrow's advantage is that source, resource types, stores, identities, indexes,
presence rules, write plans, effects, and evolution are compiled together. The
application surface lets the compiler use that unified knowledge to generate the
boundary work:

- no hand-written DTOs;
- no hand-written validators;
- no hand-written route handlers for basic resource access;
- no ORM or query language;
- no raw saved-path API;
- no generic client-authored filters, projections, includes, or writes.

The source remains Marrow-shaped. It declares typed resources, durable stores,
indexes, surfaces, and functions. Transports adapt checked facts.

## Source Shape

Proposed surface syntax:

```mw
surface Books from ^books
    fields title, author, blurb
    collection ^books as list
    collection ^books.byAuthor as byAuthor
    create title, author, blurb
    update title, blurb
```

The first parser slice only needs the declaration and source-native collection
targets:

- `surface Name from ^store` declares one public application surface backed by
  one durable store.
- a store can have multiple surfaces, such as a public surface and an admin
  surface.
- `surface` is a module-level declaration. Its name shares the module
  declaration namespace with functions, resources, enums, stores, constants,
  imports, and other surfaces. A collision is a check error.
- a surface declaration is public by definition. There is no `pub surface` in
  this design.
- renaming the surface alias is ABI-breaking until compatibility windows are
  designed.
- every surface record includes an implicit public identity, typed as
  `Id(^store)`.
- `fields` names the non-identity store resource fields that are public on
  reads. Initial parser/checker support is top-level fields; dotted unkeyed
  fields such as `name.first as firstName` are a later projection expansion.
- `collection ^books as list` exposes the root traversal. The target is the
  backing store root, not a magic `all` sentinel.
- `collection ^books.byAuthor as byAuthor` exposes a declared store index. The
  target uses source-native path syntax, so an index named `all` is not special.
  Collection targets must be the backing store root or an index owned by the
  backing store, checked by catalog/store identity rather than spelling.
- `create` names fields accepted by the generated create operation.
- `update` names fields accepted by the generated update operation.

The contextual words inside a `surface` block are not general language keywords.
The only new top-level declaration keyword proposed here is `surface`; `from` is
contextual only in a surface header.

## Why Not Inline Actions

This design does not include `action` blocks inside a surface. Custom behavior
uses normal Marrow functions:

```mw
pub fn loanBook(id: Id(^books), borrower: string)
    if not exists(^books(id))
        throw Error(code: "book.absent", message: "Book does not exist.")

    if exists(^books(id).borrowedBy)
        throw Error(code: "book.already_loaned", message: "Already loaned.")

    ^books(id).borrowedBy = borrower
```

This preserves Marrow's one function model. It avoids method receivers, implicit
`self`, and a second execution path inside surface declarations. A later client
renderer can group `pub fn` functions near related surfaces when their checked
signatures and store footprints make that grouping unambiguous, but the function
remains an ordinary `pub fn`.

## Capability Matrix

`fields`, `create`, and `update` are allow-lists, but generated writes do not
silently create hidden public write paths:

- `fields` is the read projection. A field not named in `fields` is never
  emitted by generated read operations.
- generated `create` and `update` fields must also be present in `fields`.
- write-only generated inputs are deferred. They require explicit syntax such as
  `writeonly` or a separate input block so the public write capability is
  visible in source.
- a field absent from `create` and `update` is not writable through generated
  operations. Ordinary `pub fn` code can still implement custom write behavior.

Generated operations never infer write permission from read permission, and
never infer read permission from write permission.

## Generated Operations

A surface produces checked operation descriptors. Operation names are ABI facts,
not route semantics.

### Get

Every surface with `fields` has a generated identity read. In the notation
below, `Books.record` names the `SurfaceAbi` projection descriptor for the
surface record; it is not a `.mw` type alias.

```text
get(id: Id(^books)): Books.record
```

The generated operation:

- decodes the identity through the read request-parameter codec and the same
  `Id(^store)` guard used by Marrow;
- proves the record node exists for this request. Ordinary absence is a
  catchable surface absence and can render as HTTP 404 in an HTTP profile;
- materializes the full non-keyed backing resource body before projection:
  top-level fields and unkeyed groups, including non-public required fields;
- includes the typed public identity in the result;
- emits only the declared public projection;
- applies normal required/sparse data rules while materializing.

The read footprint and the output projection are different contracts. The read
footprint is full non-keyed resource materialization so missing non-public
required data cannot be served as a valid public record. The projection is the
public shape emitted to the client. This costs one full non-keyed record
materialization per `get`; keyed child layers remain outside `get`. The
materialization has an operation decoded-byte budget. A valid record whose
non-keyed body exceeds that budget fails with `surface.limit`, not
`surface.request` or `surface.invalid_data`.

A malformed identity is a request-shape error. Invalid attached data is
different: if materialization finds missing required data or a stored leaf in
the materialized non-keyed backing resource cannot decode as the checked type,
the operation raises a fatal data-attachment fault. That fault is not catchable
Marrow control flow and is not mapped to a 4xx client error. A surface host
isolates the fault to the current operation, returns a sanitized service fault
at the transport boundary, keeps serving other requests, and relies on
`marrow data integrity` or repair workflows to identify and fix the corrupt
record.

The implementation of `get` must start from a shared typed materialization API,
not prose matching over runtime errors. That API returns variants such as
`AbsentRecord`, `MissingRequired`, `DecodeInvalid`, `MaterializationLimit`, and
`StoreFault`; runtime, tooling, and surface renderers then map those variants to
their own envelopes.

### Collections

Collection support is not part of the first runtime operation lane.

When implemented, a collection is a paged traversal over either the root
identity order or a declared index target:

```mw
collection ^books as list
collection ^books.byAuthor as byAuthor
```

Each collection operation returns a canonical page envelope:

```text
Page<Books.record> = { items: sequence[Books.record], nextCursor: maybe-present Cursor }
```

Generated clients expose the shape as:

```text
books.list({ limit, cursor }): Page<Book>
books.byAuthor(authorId, { limit, cursor }): Page<Book>
```

`collection ^books.byAuthor as byAuthor` exposes a paged traversal over a
declared non-unique index such as `index byAuthor(author, id)`. The initial
collection lane supports only one non-identity index component followed by the
complete store identity suffix. The client supplies that component as an exact
parameter, and paging advances over the identity suffix in index order. The
operation is rejected if the index is unique, omits the complete identity
suffix, has more than one non-identity component, walks a keyed child layer, or
would need an unbounded scan. The target must be the surface backing store or an
index owned by the surface backing store; a foreign store root or foreign-store
index is a check error even when its key payload has the same scalar shape.

The general rule for later multi-component index collections is exact prefix,
then at most one trailing ordered range component, then the complete identity
suffix. A surface must not expose a collection that leaves an intermediate
non-identity component to enumerate.

Collections never materialize an unbounded list. `limit` is normalized to a
bounded protocol value. The initial protocol maximum is 128 rows per page and 8
MiB decoded non-keyed record data per page. Invalid request limits fail with
`surface.request`; a well-formed request whose next materialized row would
exceed the decoded-byte budget returns the rows that already fit, if at least
one row fits, and a cursor after the last returned row. If the first candidate
record exceeds the decoded-byte budget, the page fails with `surface.limit` and
returns no advancing cursor. The effective row and decoded-byte budgets are part
of the operation equality tag.

Each returned collection row performs the same full non-keyed backing-resource
materialization as `get`, then emits the public projection. Each candidate
identity must prove the backing record node exists. A dangling index entry,
malformed identity suffix, missing record node, decode failure, or required-data
fault fails the whole page and returns no advancing cursor boundary. The rest of
that collection remains unavailable through this surface until operator repair
fixes the corrupt record. This is the accepted consistency-over-availability
tradeoff.

The collection lane depends on a separate store/runtime keyset traversal API.
The current opaque engine cursor is not the public surface cursor contract.

### Create

`create title, author, blurb` generates a create operation only when the checker
can prove the store has a supported identity policy and the generated input can
produce a valid resource.

Generated create supports single-`int` identity stores using `nextId(^store)`.
Composite identity and non-integer identity creation are handled by explicit
`pub fn` functions until a separate key-input design is accepted.

The generated create operation:

- allocates the identity with `nextId(^store)`;
- builds a resource value from the supplied fields;
- is rejected at check time unless every required non-identity field in the
  resource appears in the `create` list;
- is rejected at check time when required fields inside unkeyed groups would be
  needed, because the initial generated-write syntax names only top-level
  fields;
- writes the new record through the checked managed-write path inside one
  operation transaction;
- materializes and validates the post-create non-keyed backing resource state;
- commits only after the post-create state can produce the surface record
  projection, then returns that projection.

Sparse create fields can be omitted. Omitted sparse fields are absent. JSON
`null` is not a deletion or absence marker for generated create.

### Update

`update title, blurb` generates a field-wise update operation. It is not a
whole-record replacement and it is not a generic raw setter.

The generated update operation:

- decodes an `Id(^store)`;
- proves the record exists before writing;
- materializes the full non-keyed backing resource body inside the operation
  transaction before writing, so corrupt private required data aborts before any
  durable mutation;
- accepts a non-empty update object whose keys are a subset of the declared
  update fields;
- decodes each supplied JSON value through the generated-write object-body codec
  for that field's checked type;
- rejects unknown object keys and values that do not decode to the checked field
  type;
- writes only supplied fields;
- preserves omitted fields, unkeyed groups, keyed children, and unrelated data;
- updates generated indexes through the normal managed-write path;
- materializes and validates the post-update non-keyed backing resource state;
- commits only after the post-update state can produce the surface record
  projection, then returns that projection.

Explicit `null` is rejected. Deletion, clearing sparse fields, whole-record
replacement, and keyed-child updates are outside generated update.

## Surface ABI Facts

The checker produces derived `SurfaceAbi` facts. Renderers consume these facts
instead of reinterpreting source strings.

`SurfaceAbi` is not a new catalog namespace and it is not persisted metadata.
There are no stable surface or operation catalog IDs. A surface fact references
the existing owners:

- the module-qualified surface alias from source;
- the backing store and resource catalog IDs;
- the public member catalog IDs for `fields`, `create`, and `update`;
- the declared index catalog IDs used by collections;
- generated read footprints and cost shapes computed by construction, reusing
  existing footprint/cost-shape machinery where useful;
- materialization policy and decoded-byte budget;
- checked managed-write summaries by reference once write slices are
  implemented;
- the read request-parameter codec descriptor;
- the generated-write object-body codec descriptor once write slices are
  implemented;
- the surface JSON output codec descriptor;
- the cursor token descriptor once collections are implemented;
- the public error envelope descriptor.

`SurfaceAbi` is available only when all referenced durable facts have accepted
catalog IDs. A project with pending catalog identity can still parse and check
surface syntax, but it cannot export a stable `SurfaceAbi`, run generated
surface operations, or produce generated clients. It reports a typed checker or
tooling diagnostic such as `check.surface_catalog_pending`. Source spelling or
proposal-only IDs must not be used as fallback ABI identity.

The ABI digest is a computed renderer equality tag for the whole surface. It is
derived from the surface declaration, the surface protocol version, every
operation equality tag, and the referenced accepted source and catalog facts
that already have single owners. It is not stored in `marrow.catalog.json`, does
not allocate a durable identity, and does not bypass the existing activation,
catalog, source-digest, runtime-generation, or commit fences. A renderer can use
the digest to reject a stale generated client, but that rejection is a
presentation-layer guard over existing semantic facts.

Each generated operation also has an operation equality tag. It is a
deterministic hash of the surface protocol version plus that operation's ABI
slice: operation kind, module-qualified surface alias, backing store/resource
catalog IDs, projected member IDs, collection index ID when present, request
parameter codec descriptor, output projection descriptor, materialization
policy, read footprint, traversal order, page-size bounds, decoded-byte budget,
cursor codec version when present, and public error envelope descriptor. For
generated create/update operations, the tag also includes input member catalog
IDs, generated-write object-body codec descriptor, identity allocation or
existence policy, managed-write effect summary, and generated-index effect
summary.

Generated clients and versioned transport profiles must send the expected
surface ABI digest or operation equality tag with every request. A mismatch
fails with `surface.abi_mismatch`. Bare route-only requests, when an HTTP
renderer later permits them, are current-contract requests; they do not receive
the stale-client guarantee.

Alias deprecation, compatibility windows, and generated adapters are deferred
until a compatibility design is accepted.

## Surface Boundary Codecs

Surface operations use a dedicated surface boundary codec. They do not route
through the `marrow run --format json` entry boundary, and they do not use
`std::json`. `std::json` remains a scalar JSON helper inside Marrow programs; it
does not map JSON objects to resources.

### Scalar Forms

Surface scalar JSON reuses the existing Marrow run/data JSON scalar forms where
those forms exist. The surface codec descriptor names the exact scalar profile,
so renderers do not invent independent encodings. If a checked scalar type has
no accepted scalar profile, the surface checker rejects operations that expose
that type until the profile is designed. Identity values are the only
specialized production form in this design.

### Identity Form

In production surface JSON, an `Id(^store)` appears in a typed position whose
expected store is known from the operation descriptor or generated client type.
Store provenance lives in `SurfaceAbi`; it is not repeated in every record.

Production identity JSON is surface-local:

- a single-key identity renders as the key payload, e.g. `"id": 1`;
- a composite identity renders as an object keyed by identity-key names, e.g.
  `"id": { "student": "student-1", "course": "course-9" }`;
- generated clients brand these values by surface/store type, such as `BookId`.

The generic debug form, when a renderer needs a self-describing identity value,
is:

```json
{"$id":{"root":"^books","keys":[1]}}
```

The `root` field uses the saved-root spelling `^books`; it does not invent a
module-qualified root syntax. Decoders reject missing fields, extra fields,
non-array `keys`, wrong key arity, wrong key type, and a root that does not
match the expected `Id(^store)` type.

HTTP path or query renderers can choose a compact route spelling, but that
spelling is presentation. It must round-trip through the typed identity guard.

### Read Request Parameters

The point-`get` lane includes request-parameter decoding only for `Id(^store)`
arguments. Collection parameter and cursor decoding belong to the collection
lane. The request-parameter codec rejects malformed JSON, wrong-store
identities, wrong key arity, and wrong scalar key types.

### Output Codec

The output codec is the single owner for surface-record projection to JSON:

- public required fields render as values;
- sparse fields render as omitted when absent;
- JSON `null` is not emitted for Marrow absence;
- missing required saved data is a fatal data-attachment fault, not `null`;
- `Id(^store)` values render with the production identity form above;
- enum values render by accepted member identity and current public spelling;
- scalar values use the named surface scalar profile;
- identity-typed fields render the identity value without dereferencing. A
  renderer profile that attempts to expose a dereferenced link must fail missing
  referents as a public integrity fault; it must not silently create a trusted
  link.

The renderer can additionally produce links for HTTP or hypermedia profiles, but
the ABI value is the typed identity. Links are presentation, not semantic
identity.

### Generated-Write Object Body Codec

This follow-on codec is the single owner for JSON-to-Marrow decoding of
generated create and update object bodies:

- object inputs reject unknown keys;
- required create keys must be present according to the checked create contract;
- omitted sparse create keys become absent;
- omitted update keys are unchanged;
- explicit JSON `null` is rejected in the initial generated-write slice;
- scalar, enum, bytes, decimal, temporal, error-code, and `Id(^store)` values
  decode through the named surface scalar and identity profiles;
- enum text resolves to an accepted enum-member identity;
- identity input resolves through the store-aware identity guard;
- malformed, out-of-range, wrong-type, or wrong-store values are request-shape
  errors before Marrow code or managed writes run.

## Cursor Boundary

Surface cursors are typed keyset tokens. They are not engine cursors and they do
not expose raw physical keys.

A cursor token is bound to:

- operation equality tag;
- store UID;
- activation lineage: accepted catalog digest and source digest, plus engine
  profile digest;
- normalized operation parameters;
- traversal order and projection;
- page size;
- decoded-byte budget;
- last returned typed key boundary;
- cursor codec version.

The cursor is intentionally not bound to the ordinary commit ID, so normal
writes do not stale every page. A restore, store swap, catalog/source/profile
change, cursor codec change, operation ABI change, or store UID change fails
with `surface.stale_cursor`.

Continuation uses a strict greater-than seek from the last returned key boundary
under the same operation parameters, ordering, projection, page size, decoded
budget, store lineage, and operation tag. Surface cursors do not promise stable
multi-page snapshots across writes. A create whose key sorts before the cursor
boundary can be missed by later pages, and a delete of a previously returned
record does not prevent the next strict seek from advancing. Store commit
changes do not by themselves stale a cursor.

The collection lane must first add or reuse a typed keyset traversal API that
can resume root and exact-index scans from a full typed identity suffix. It must
not expose or serialize the store engine's opaque cursor.

## Transport Rendering

HTTP routes are optional renderer output. They do not define the surface
contract.

A renderer might produce routes like:

```text
GET  /books
GET  /books/{bookId}
GET  /books/by-author?author={authorId}
POST /books
POST /books/{bookId}
```

Those route spellings are aliases over derived operation facts. Versioned
generated clients and transport profiles must send the expected surface ABI
digest or operation equality tag; otherwise they are current-contract route
requests with no stale-client guarantee.

The canonical identity segment format is a renderer decision, but it must be
decoded through the typed identity boundary and must avoid collision with
structural route names.

## Public Error Envelope

Surface renderers expose a public error envelope with:

- `code`: stable dotted public error code;
- `message`: public diagnostic text suitable for the client boundary;
- `data`: optional structured public payload.

Initial public codes:

- `surface.request`: malformed request parameter, identity, index argument, or
  generated-write body;
- `surface.absent`: requested record identity is well-formed but no record node
  exists;
- `surface.cursor`: malformed cursor token;
- `surface.stale_cursor`: cursor token is well-formed but its operation equality
  tag no longer matches the active operation facts or store lineage;
- `surface.abi_mismatch`: generated client or transport request targets an ABI
  slice that is no longer active;
- `surface.invalid_data`: backing saved data cannot be materialized under the
  checked resource shape;
- `surface.limit`: well-formed operation exceeds its materialization, row, or
  decoded-byte budget;
- `surface.integrity`: a renderer profile that dereferences identity links finds
  a missing referent;
- `surface.store`: the store reports a fault while serving a surface operation.

The public envelope does not include source spans, file paths, line numbers,
trace output, internal physical keys, raw catalog rows, or repair details.
Operator logs and tooling diagnostics can retain richer internal facts, but
surface clients branch only on public `code` and `data`. Public surface
envelopes can include `kind: "surface"` when they are rendered through a generic
tooling envelope.

Checker diagnostics need stable codes before implementation. Initial reserved
checker codes are:

- `check.surface_decl`;
- `check.surface_collision`;
- `check.surface_target`;
- `check.surface_field`;
- `check.surface_catalog_pending`;
- `check.surface_operation`.

## Operations Contract

The first runtime implementation target is local and single-writer.

Safe initial serving rules:

- serving is owned by one local host process;
- non-loopback/public serving is outside the initial serving target;
- first-run catalog baseline and pending catalog proposals must be resolved
  before runtime surface operations or generated clients are available;
- write slices serialize generated operations through one writer queue;
- every generated create/update operation runs as one short operation
  transaction once those slices are implemented;
- reads are bounded by operation cost shape, decoded-byte budgets, and page
  limits;
- no maintenance, backup, restore, evolve, repair, or raw data tooling is
  exposed through surfaces;
- backup, restore, evolution, and repair remain operator workflows outside the
  surface runtime;
- stale cursor/client conditions are typed errors, not fallback behavior.

## Diagnostics

Surface diagnostics must be checker-native and specific:

- surface declaration collides with another module-level declaration;
- surface target is not a store;
- exposed field does not exist;
- exposed field is not supported by the initial surface JSON shape;
- exposed field or input field has no accepted surface scalar profile;
- collection target names no store root or declared index;
- collection target names a foreign store root or an index not owned by the
  surface backing store;
- collection would require an unsupported or hidden traversal;
- collection index has unsupported component or range shape;
- create omits a required field;
- create targets required fields inside an unkeyed group;
- create targets an unsupported identity policy;
- update names an identity key, generated index, keyed child layer, write-only
  field, or unsupported field shape;
- generated-write codec cannot decode a JSON value to the checked field type;
- operation would require maintenance capability;
- accepted catalog IDs are not available for every referenced durable fact;
- derived surface ABI changed in a way that requires client regeneration.

Diagnostics must point to the surface declaration and, where useful, the store,
field, or index declaration it references. Clients and tools branch on stable
codes and structured payloads, never prose.

## Implementation Sequence

Implement as small independently reviewed lanes:

1. Language docs, parser, and formatter: `surface` declaration, contextual
   surface items, namespace rules, no runtime behavior.
2. Checker facts: resolve surfaces to accepted stores and supported fields,
   reject unsupported source shapes, and report pending catalog availability.
3. Boundary codec facts: scalar profile reference, production identity form,
   generic debug identity form, public error envelope descriptors.
4. Typed materialization API: shared `AbsentRecord`, `MissingRequired`,
   `DecodeInvalid`, `MaterializationLimit`, and `StoreFault` variants over
   non-keyed resource materialization.
5. Point `get` executor: identity decode, materialization, projection output,
   no collections or cursors.
6. Typed keyset traversal API: root and exact-index scans resume from a full
   typed identity suffix, without serializing engine cursors.
7. Collection executor and cursor token codec over the typed traversal API.
8. Generated TypeScript client rendering over `SurfaceAbi`.
9. Optional HTTP renderer over `SurfaceAbi`.
10. Generated-write object-body codec.
11. Generated `create` for single-`int` `nextId(^store)` identity stores.
12. Generated `update` for top-level scalar, enum, and identity fields.

The first production test boundary for generated runtime operations is a
Marrow-owned library API in `marrow-run` or a new focused crate boundary, not
HTTP and not a test-only entrypoint. CLI/HTTP/TypeScript renderers layer on the
same facts later.

## Deferred

Deferred deliberately:

- inline surface `action` blocks;
- public route declarations in `.mw`;
- auth/principal context;
- non-loopback serving;
- old-client compatibility windows;
- public alias deprecation lifecycle;
- generated adapters across schema evolution;
- client-authored filters, orderings, projections, includes, and expansions;
- multi-component index collection parameters and ranges;
- write-only generated inputs;
- generic field setters outside declared `update`;
- delete, clear, merge, patch, PUT, PATCH, and DELETE semantics;
- keyed-child CRUD;
- full nested unkeyed-group projection in the first point-`get` lane;
- unique-index lookup operations;
- composite and non-integer identity creation;
- route-level schema/model endpoint;
- live streams, jobs, sync, and background work;
- backup-backed serving;
- field-level cache precision beyond changed root/index facts.

## Alternatives Reviewed

### Retired URI-Tree Serving

The earlier URI-tree serving proposal gives the best demo and the worst
foundation. It exposes storage shape as public ABI, makes field additions public
by default, and ties route spelling to source spelling rather than public ABI
facts. It is superseded by `surface`.

### Exported Functions Only

`pub fn` is the most idiomatic execution model. It keeps behavior in ordinary
Marrow and lets generators render typed clients. It is too verbose for common
CRUD, because every simple read, create, and update becomes a hand-written
function and view shape.

### Surface ABI

The selected design keeps stores private by default, avoids CRUD boilerplate for
the operations the checker can prove, and routes all behavior through checked
facts. Custom invariants and workflows remain ordinary `pub fn` functions.

## Adversarial Guardrails

- A surface is not a query language.
- A surface is not a route language.
- A surface is not a catalog namespace.
- A surface is not the `marrow run --format json` boundary.
- A surface does not expose raw saved paths.
- A surface does not make all store fields public.
- A surface does not generate arbitrary setters.
- A surface does not materialize unbounded collections.
- A surface does not hide scans behind filters.
- A surface does not reinterpret invalid saved data as JSON null.
- A surface does not bypass `Id(^store)`, requiredness, sparse-field, index, or
  managed-write rules.
- A surface renderer can be replaced without changing Marrow semantics.

## Required Documentation Updates Before Implementation

- `docs/language/grammar.md`: add `surface_decl` and contextual surface items.
- `docs/language/syntax.md`: reserve or define `surface` spelling and describe
  contextual surface item names.
- `docs/language/resources-and-storage.md`: define how surfaces reference
  stores, fields, and indexes without changing durable storage semantics.
- `docs/language/modules-functions.md`: define the relationship between surfaces
  and `pub fn` functions.
- `docs/tooling-surfaces.md`: add `SurfaceAbi` as a typed protocol fact surface.
- `docs/error-codes.md`: add parse/check/surface/runtime codes for surface
  validation and stale clients.
- `docs/operations.md`: define initial local serving constraints.
- `docs/stability.md`: state how application surfaces supersede the older
  linked-Rust/TypeScript-export application-boundary sketch.
- implementation docs for checker facts, materialization, codecs, TypeScript
  renderer, and optional HTTP renderer.
