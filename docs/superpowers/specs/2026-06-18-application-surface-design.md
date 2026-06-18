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

The goal is to remove CRUD boilerplate without exposing raw saved paths or
turning every public API into hand-written functions. A surface declares the
public projection and the generated operations the compiler can prove from
checked store facts. Custom behavior remains ordinary exported Marrow functions.

Routes, JSON, TypeScript clients, identity codecs, paging, errors, and cache
facts are renderers over checked `SurfaceAbi` facts. They are not separate
semantic owners.

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
indexes, surfaces, and functions. Transports adapt checked facts.

## Source Shape

First-cut syntax:

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
- every surface record includes an implicit public identity, typed as
  `Id(^store)`;
- `fields` names the non-identity store resource fields that are public on
  reads.
- `collections` names the public paged traversals. `all` means the store root.
  Any other name must be a declared store index.
- `create` names fields accepted by the generated create command.
- `update` names fields accepted by the generated update command.

The contextual words inside a `surface` block are not general language
keywords. The only new top-level declaration keyword proposed here is
`surface`.

## Why Not Inline Actions

The first cut does not include `action` blocks inside a surface. Custom behavior
uses normal Marrow functions:

```mw
export fn loanBook(id: Id(^books), borrower: string)
    if not exists(^books(id))
        throw Error(code: "book.absent", message: "Book does not exist.")

    if exists(^books(id).borrowedBy)
        throw Error(code: "book.already_loaned", message: "Already loaned.")

    ^books(id).borrowedBy = borrower
```

This preserves Marrow's one function model. It avoids method receivers, implicit
`self`, and a second execution path inside surface declarations. A later client
renderer can group exported functions near related surfaces when their checked
signatures and store footprints make that grouping unambiguous, but the function
remains an ordinary exported function.

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
- proves the record node exists for this request;
- includes the typed public identity in the result;
- reads only the declared public fields;
- applies normal required/sparse data rules;
- fails with typed errors when the identity is malformed, absent, or backed by
  invalid saved data.

### Collections

Each collection is a paged traversal.

`collections all` exposes a paged root traversal in store identity order.

`collections byAuthor` exposes a paged traversal over the declared
`index byAuthor(...)`. The checker derives the request parameters from the
index's non-identity components. The operation is rejected if the index shape
cannot produce a bounded page over store identities.

Collections never materialize an unbounded list. The ABI always includes a page
limit and an opaque cursor contract.

### Create

`create title, author, blurb` generates a create command only when the checker
can prove the store has a supported identity policy and the generated input can
produce a valid resource.

First-cut create supports single-`int` identity stores using `nextId(^store)`.
Composite identity and non-integer identity creation are handled by explicit
exported functions until a separate key-input design is accepted.

The generated create command:

- allocates the identity with `nextId(^store)`;
- builds a resource value from the supplied fields;
- requires every required field without a default to be supplied or otherwise
  proven by the surface;
- writes the new record through the checked managed-write path;
- returns the new `Id(^store)`.

Sparse create fields may be omitted. Omitted sparse fields are absent. JSON
`null` is not a deletion or absence marker in this first cut.

### Update

`update title, blurb` generates a field-wise update command. It is not a
whole-record replacement and it is not a generic raw setter.

The generated update command:

- decodes an `Id(^store)`;
- proves the record exists before writing;
- accepts a non-empty update object whose keys are a subset of the declared
  update fields;
- writes only supplied fields;
- preserves omitted fields, unkeyed groups, keyed children, and unrelated data;
- updates generated indexes through the normal managed-write path;
- rejects attempts to delete required fields or write `unknown` values.

Explicit `null` is rejected. Deletion, clearing sparse fields, whole-record
replacement, and keyed-child updates are outside the first cut.

## Surface ABI Facts

The checker produces a `SurfaceAbi` table. Renderers consume this table instead
of reinterpreting source strings.

Each surface fact records:

- stable surface identity and ABI digest;
- source alias and lifecycle state;
- backing store catalog ID;
- backing resource catalog ID;
- public identity field shape;
- public field member catalog IDs;
- sparse, required, and defaulted presence behavior;
- public collection operation IDs;
- public command operation IDs;
- result shapes and input shapes;
- effect class: query or command in the first cut;
- checked read footprints;
- checked write-plan shapes;
- generated index dependencies;
- changed root and index facts for commands;
- cursor token ABI;
- typed error envelope shape;
- protocol version, catalog epoch, source digest, and runtime generation fence
  fields for renderers that need them.

Surface and operation identities are application ABI identities. They are
distinct from store, resource, member, and index identities. Source spelling is a
public alias, not the identity authority.

The first cut rejects stale clients by ABI digest rather than preserving old
aliases. Alias deprecation, compatibility windows, and generated adapters are
deferred until a compatibility design is accepted.

## JSON Boundary

The surface renderer owns a single resource JSON codec for surface values. It is
not `std::json` and it is not a second storage model.

First-cut rules:

- required fields render as values;
- sparse fields render as omitted when absent;
- JSON `null` is not emitted for Marrow absence;
- missing required saved data is a typed invalid-data fault, not `null`;
- `Id(^store)` values render as branded store-aware identity values, never raw
  scalars without provenance;
- enum values render by accepted member identity and current public spelling;
- decimals, dates, instants, durations, bytes, and error codes use one
  normative surface JSON form;
- dangling identity references either fail typed or render an explicit integrity
  state chosen by the surface profile; they never silently become trusted links.

The renderer can additionally produce links for HTTP or hypermedia profiles, but
the ABI value is the typed identity. Links are presentation, not semantic
identity.

## Cursor Boundary

Surface cursors are typed keyset tokens. They are not engine cursors and they do
not expose raw physical keys.

A cursor token is bound to:

- surface operation ID;
- ABI digest;
- catalog epoch and source digest;
- runtime generation when present;
- store UID;
- commit ID observed by the page;
- normalized operation parameters;
- traversal order and projection;
- page size;
- last returned typed identity or key boundary;
- cursor codec version.

If a cursor is stale because the store commit, catalog, source digest, store UID,
or ABI digest changed, the next page fails with a typed stale-cursor error. The
first cut does not promise stable multi-page snapshots across writes.

## Transport Rendering

HTTP routes are optional renderer output. They do not define the surface
contract.

A renderer might produce routes like:

```text
GET  /books
GET  /books/{bookId}
GET  /books/by-author/{authorId}
POST /books
POST /books/{bookId}
```

Those route spellings are aliases over operation IDs. Reusing a route for a
different operation, index shape, parameter type, or result projection changes
the ABI digest and stale clients fail rather than silently observing a different
contract.

The canonical identity segment format is a renderer decision, but it must be
decoded through the typed identity boundary and must avoid collision with
structural route names.

## Operations Contract

The first implementation target is local and single-writer.

Safe first-cut serving rules:

- serving is owned by one local host process;
- non-loopback/public serving is outside the first cut;
- first-run catalog baseline and pending catalog proposals must be resolved
  before serving production traffic;
- writes are serialized through one writer queue;
- every generated create/update command runs as one short operation transaction;
- reads are bounded pages and release snapshots before returning;
- no maintenance, backup, restore, evolve, repair, or raw data tooling is
  exposed through surfaces;
- backup, restore, evolution, and repair remain operator workflows outside the
  surface runtime;
- stale cursor/client conditions are typed errors, not fallback behavior.

## Diagnostics

Surface diagnostics must be checker-native and specific:

- exposed field does not exist;
- exposed field is not supported by the first-cut surface JSON shape;
- collection names no store root or declared index;
- collection would require an unsupported or hidden traversal;
- create omits a required field;
- create targets an unsupported identity policy;
- update names an identity key, generated index, keyed child layer, or
  unsupported field shape;
- operation would require maintenance capability;
- surface alias collides with another public surface alias;
- generated ABI changed in a way that requires client regeneration.

Diagnostics must point to the surface declaration and, where useful, the store,
field, or index declaration it references. Clients and tools branch on stable
codes and structured payloads, never prose.

## First Cut

Implement only:

- `surface Name from ^store`;
- `fields` over top-level scalar, enum, and `Id(^store)` fields;
- generated `get`;
- paged `collections all`;
- paged `collections <non_unique_index>`;
- `create` for single-`int` `nextId(^store)` identity stores;
- `update` for top-level scalar, enum, and identity fields;
- `SurfaceAbi` facts;
- TypeScript and JSON renderers over `SurfaceAbi`;
- optional HTTP renderer over `SurfaceAbi`.

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

`export fn` is the most idiomatic execution model. It keeps behavior in ordinary
Marrow and lets generators render typed clients. It is too verbose for common
CRUD, because every simple read, create, and update becomes a hand-written
function and view shape.

### Surface ABI

The selected design keeps stores private by default, avoids CRUD boilerplate for
the operations the checker can prove, and routes all behavior through checked
facts. Custom invariants and workflows remain ordinary exported functions.

## Adversarial Guardrails

- A surface is not a query language.
- A surface is not a route language.
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
  surfaces and exported functions.
- `docs/tooling-surfaces.md`: add `SurfaceAbi` as a typed protocol fact surface.
- `docs/error-codes.md`: add parse/check/surface/runtime codes for surface
  validation and stale clients.
- `docs/operations.md`: define first-cut local serving constraints.
- implementation docs for checker facts, JSON renderer, TypeScript renderer, and
  optional HTTP renderer.
