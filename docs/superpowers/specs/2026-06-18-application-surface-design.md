# Application Surfaces

- Date: 2026-06-18
- Status: Design approved for written review. Propose-first: this touches
  language and application-boundary semantics, so implementation requires
  `docs/language/` and the checked-facts docs to move in lockstep.
- Scope: public application API foundation for Marrow, not an HTTP server
  implementation.

## Summary

Marrow exposes applications through a source-declared `surface`: a public
ABI over a durable store. A store models durable truth. A surface models the
application contract clients can read and write.

The goal is to remove CRUD boilerplate where the compiler has enough checked
facts to do it safely, without exposing raw saved paths or turning every public
API into hand-written functions. A surface declares the public projection and
the generated operations the compiler can prove from checked store facts. Custom
behavior remains ordinary `pub fn` Marrow functions.

Routes, JSON, TypeScript clients, identity codecs, paging, errors, and cache
facts are renderers over checked surface facts. They are not separate semantic
owners.

## Design Principle

Marrow's advantage is that source, resource types, stores, identities, indexes,
presence rules, write plans, effects, and evolution are compiled together. The
application surface lets the compiler use that unified knowledge to
generate the boring boundary work:

- no hand-written DTOs;
- no hand-written validators;
- no hand-written route handlers for basic resource access;
- no ORM or query language;
- no raw saved-path API;
- no generic client-authored filters, projections, includes, or writes.

The source remains Marrow-shaped. It declares typed resources, durable stores,
indexes, surfaces, and functions. Transports adapt checked facts. The first
implementation slice is read-only; create and update are specified here as
follow-on slices because JSON input decoding and managed writes need their own
soundness review.

## Source Shape

Proposed surface syntax:

```mw
surface Books from ^books
    fields title, author, blurb
    collections all, byAuthor
    create title, author, blurb
    update title, blurb
```

This intentionally keeps the surface vocabulary small:

- `surface Name from ^store` declares one public application surface backed by
  one durable store.
- a store can have multiple surfaces, such as a public surface and an admin
  surface. Surface aliases are module-qualified source names, not catalog
  identities.
- every surface record includes an implicit public identity, typed as
  `Id(^store)`;
- `fields` names the non-identity store resource fields that are public on
  reads.
- `collections` names the public paged traversals. `all` means the store root.
  Any other name must be a declared store index.
- `create` names fields accepted by the generated create operation.
- `update` names fields accepted by the generated update operation.

The contextual words inside a `surface` block are not general language
keywords. The only new top-level declaration keyword proposed here is
`surface`.

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

`fields`, `create`, and `update` are independent allow-lists:

- `fields` is the read projection. A field not named in `fields` is never
  emitted by generated read operations.
- `create` is the accepted create input shape. A field can be create-only and
  omitted from `fields`, but it will not be returned by generated reads.
- `update` is the accepted field-write input shape. A field can be update-only
  and omitted from `fields`, but it will not be returned by generated reads.
- A field absent from `create` and `update` is not writable through generated
  operations. Ordinary `pub fn` code can still implement custom write behavior.

Generated operations never infer write permission from read permission, and
never infer read permission from write permission.

## Generated Operations

A surface produces checked operations. Operation names are ABI facts, not route
semantics.

### Get

Every surface with `fields` has a generated identity read:

```text
get(id: Id(^books)): Books.Record
```

The generated operation:

- decodes the identity through the same `Id(^store)` boundary used by Marrow;
- proves the record node exists for this request. Ordinary absence is a
  catchable surface absence and can render as HTTP 404 in an HTTP profile;
- verifies required-field completeness for the backing resource, including
  required fields inside unkeyed groups, before rendering the public projection;
- includes the typed public identity in the result;
- reads only the declared public fields;
- applies normal required/sparse data rules.

A malformed identity is a request-shape error. Invalid attached data is
different: if backing-resource verification finds missing required data or a
stored leaf needed by the surface cannot decode as the checked type, the
operation raises a fatal data-attachment fault. That fault is not catchable
Marrow control flow and is not mapped to a 4xx client error. A surface host
isolates the fault to the current operation, returns a sanitized service fault
at the transport boundary, keeps serving other requests, and relies on
`marrow data integrity` or repair workflows to identify and fix the corrupt
record. A collection page that reaches a corrupt record fails the page; it must
not skip the record silently.

### Collections

Each collection is a paged traversal.

`collections all` exposes a paged root traversal in store identity order.

`collections byAuthor` exposes a paged traversal over a declared non-unique
index such as `index byAuthor(author, id)`. The first read slice supports only
one non-identity index component followed by the complete store identity suffix.
The client supplies that component as an exact parameter, and paging advances
over the identity suffix in index order. The operation is rejected if the index
is unique, omits the complete identity suffix, has more than one non-identity
component, walks a keyed child layer, or would need an unbounded scan.

The general rule for later multi-component index collections is exact prefix,
then at most one trailing ordered range component, then the complete identity
suffix. A surface must not expose a collection that leaves an intermediate
non-identity component to enumerate.

Collections never materialize an unbounded list. The ABI always includes a page
limit and an opaque cursor contract.

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
  needed, because the first surface syntax names only top-level fields;
- writes the new record through the checked managed-write path;
- returns the new `Id(^store)`.

Sparse create fields can be omitted. Omitted sparse fields are absent. JSON
`null` is not a deletion or absence marker for generated create.

### Update

`update title, blurb` generates a field-wise update operation. It is not a
whole-record replacement and it is not a generic raw setter.

The generated update operation:

- decodes an `Id(^store)`;
- proves the record exists before writing;
- accepts a non-empty update object whose keys are a subset of the declared
  update fields;
- decodes each supplied JSON value through the surface input codec for that
  field's checked type;
- rejects unknown object keys and values that do not decode to the checked field
  type;
- writes only supplied fields;
- preserves omitted fields, unkeyed groups, keyed children, and unrelated data;
- updates generated indexes through the normal managed-write path.

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
- checked read-effect summaries by reference;
- checked managed-write summaries by reference once write slices are
  implemented;
- the surface JSON input and output codec descriptors;
- the cursor token descriptor;
- the public error envelope descriptor.

The ABI digest is a computed renderer equality tag. It is derived from the
surface declaration, the surface protocol version, and the referenced source and
catalog facts that already have single owners. It is not stored in
`marrow.catalog.json`, does not allocate a durable identity, and does not bypass
the existing activation, catalog, source-digest, runtime-generation, or commit
fences. A renderer can use the digest to reject a stale generated client, but
that rejection is a presentation-layer guard over existing semantic facts.

Alias deprecation, compatibility windows, and generated adapters are deferred
until a compatibility design is accepted.

## Surface JSON Codecs

Surface operations use a dedicated surface JSON codec. They do not route through
the `marrow run --format json` entry boundary, and they do not use `std::json`.
`std::json` remains a scalar JSON helper inside Marrow programs; it does not map
JSON objects to resources.

The output codec is the single owner for surface-record projection to JSON:

- public required fields render as values;
- sparse fields render as omitted when absent;
- JSON `null` is not emitted for Marrow absence;
- missing required saved data is a fatal data-attachment fault, not `null`;
- `Id(^store)` values render as branded store-aware identity values, never raw
  scalars without provenance;
- enum values render by accepted member identity and current public spelling;
- decimals, dates, instants, durations, bytes, and error codes use one
  normative surface JSON form;
- identity-typed fields render the identity value without dereferencing. A
  renderer profile that attempts to expose a dereferenced link must fail missing
  referents as a public integrity fault; it must not silently create a trusted
  link.

The renderer can additionally produce links for HTTP or hypermedia profiles, but
the ABI value is the typed identity. Links are presentation, not semantic
identity.

The input codec is the single owner for JSON-to-Marrow decoding of generated
create and update inputs:

- object inputs reject unknown keys;
- required create keys must be present according to the checked create contract;
- omitted sparse create keys become absent;
- omitted update keys are unchanged;
- explicit JSON `null` is rejected in the initial generated-write slice;
- scalar, enum, bytes, decimal, temporal, error-code, and `Id(^store)` values
  decode through one normative surface form;
- enum text resolves to an accepted enum-member identity;
- identity text resolves through the store-aware identity guard;
- malformed, out-of-range, wrong-type, or wrong-store values are request-shape
  errors before Marrow code or managed writes run.

## Cursor Boundary

Surface cursors are typed keyset tokens. They are not engine cursors and they do
not expose raw physical keys.

A cursor token is bound to:

- operation equality tag;
- normalized operation parameters;
- traversal order and projection;
- page size;
- last returned typed key boundary;
- cursor codec version.

Continuation uses a strict greater-than seek from the last returned key boundary
under the same operation parameters, ordering, projection, and page size.
Surface cursors do not promise stable multi-page snapshots across writes. A
create whose key sorts before the cursor boundary can be missed by later pages,
and a delete of a previously returned record does not prevent the next strict
seek from advancing. Store commit changes do not by themselves stale a cursor.

If the operation equality tag no longer matches the active renderer facts, the
next page fails with a typed stale-cursor error. Catalog/source/runtime fences
remain owned by the existing activation path.

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

Those route spellings are aliases over derived operation facts. Reusing a route
for a different operation, index shape, parameter type, or result projection
changes the ABI digest and stale clients fail rather than silently observing a
different contract.

The canonical identity segment format is a renderer decision, but it must be
decoded through the typed identity boundary and must avoid collision with
structural route names.

## Public Error Envelope

Surface renderers expose a public error envelope with:

- `code`: stable dotted public error code;
- `message`: public diagnostic text suitable for the client boundary;
- `data`: optional structured public payload.

The public envelope does not include source spans, file paths, line numbers,
trace output, internal physical keys, raw catalog rows, or repair details.
Operator logs and tooling diagnostics can retain richer internal facts, but
surface clients branch only on public `code` and `data`.

## Operations Contract

The first implementation target is local and single-writer.

Safe initial serving rules:

- serving is owned by one local host process;
- non-loopback/public serving is outside the initial serving target;
- first-run catalog baseline and pending catalog proposals must be resolved
  before serving production traffic;
- write slices serialize generated operations through one writer queue;
- every generated create/update operation runs as one short operation transaction
  once those slices are implemented;
- reads are bounded pages and release snapshots before returning;
- no maintenance, backup, restore, evolve, repair, or raw data tooling is
  exposed through surfaces;
- backup, restore, evolution, and repair remain operator workflows outside the
  surface runtime;
- stale cursor/client conditions are typed errors, not fallback behavior.

## Diagnostics

Surface diagnostics must be checker-native and specific:

- exposed field does not exist;
- exposed field is not supported by the initial surface JSON shape;
- collection names no store root or declared index;
- collection would require an unsupported or hidden traversal;
- collection index has unsupported component or range shape;
- create omits a required field;
- create targets required fields inside an unkeyed group;
- create targets an unsupported identity policy;
- update names an identity key, generated index, keyed child layer, or
  unsupported field shape;
- input codec cannot decode a JSON value to the checked field type;
- operation would require maintenance capability;
- surface alias collides with another public surface alias;
- derived surface ABI changed in a way that requires client regeneration.

Diagnostics must point to the surface declaration and, where useful, the store,
field, or index declaration it references. Clients and tools branch on stable
codes and structured payloads, never prose.

## Implementation Slices

Implement the read slice first:

- `surface Name from ^store`;
- `fields` over top-level scalar, enum, and `Id(^store)` fields;
- generated `get`;
- paged `collections all`;
- paged `collections <non_unique_index>` where the index has one non-identity
  component followed by the complete store identity suffix;
- derived `SurfaceAbi` facts;
- surface JSON output codec;
- public error envelope;
- cursor token codec.

Then implement separately reviewed follow-on slices:

- generated TypeScript client rendering over `SurfaceAbi`;
- optional HTTP renderer over `SurfaceAbi`;
- surface JSON input codec;
- generated `create` for single-`int` `nextId(^store)` identity stores;
- generated `update` for top-level scalar, enum, and identity fields.

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
- generic field setters outside declared `update`;
- delete, clear, merge, patch, PUT, PATCH, and DELETE semantics;
- keyed-child CRUD;
- nested unkeyed-group projection;
- unique-index lookup operations;
- composite and non-integer identity creation;
- route-level schema/model endpoint;
- live streams, jobs, sync, and background work;
- backup-backed serving;
- field-level cache precision beyond changed root/index facts.

## Alternatives Reviewed

### Raw Store Serving

`serve store ^books` gives the best demo and the worst foundation. It exposes
storage shape as public ABI, makes field additions public by default, and ties
route spelling to source spelling rather than public ABI facts.

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
- `docs/language/modules-functions.md`: define the relationship between
  surfaces and `pub fn` functions.
- `docs/tooling-surfaces.md`: add `SurfaceAbi` as a typed protocol fact surface.
- `docs/error-codes.md`: add parse/check/surface/runtime codes for surface
  validation and stale clients.
- `docs/operations.md`: define initial local serving constraints.
- implementation docs for checker facts, JSON renderer, TypeScript renderer, and
  optional HTTP renderer.
