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

Each surface also has one surface-local ABI namespace:

- `id` is the implicit public identity field name in the first design. A stored
  field that would project as `id` is rejected until field aliasing is designed.
- `get`, `create`, and `update` are reserved generated operation names.
- collection aliases share this namespace with generated operation names and
  future projection aliases. They must not collide with each other, `id`, or a
  reserved generated operation name.
- field projection names are the source member names until aliasing is designed.
  They must not collide with `id`, each other, or a collection alias.

Surface-local names are ABI facts. They are not route paths, and renderers do
not get a separate namespace to reinterpret them.

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

In this initial design, generated write capability requires explicit read
exposure because write-only fields are deferred. Generated operations still do
not infer any undeclared read or write capability beyond the declared
allow-lists.

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

- receives a typed identity decoded by the active boundary profile or host
  embedding, then applies the same `Id(^store)` guard used by Marrow;
- proves the record node exists for this request. Ordinary absence is a
  catchable surface absence and can render as HTTP 404 in an HTTP profile;
- materializes the full non-keyed backing resource body before projection:
  top-level fields and unkeyed groups, including non-public required fields;
- includes the typed public identity in the result;
- emits only the declared public projection;
- applies normal required/sparse data rules while materializing.

A `get` materializes under one consistent read snapshot. It does not mix fields
from different store snapshots within a single response.

The read footprint and the output projection are different contracts. The read
footprint is full non-keyed resource materialization so missing non-public
required data cannot be served as a valid public record. The projection is the
public shape emitted to the client. This costs one full non-keyed record
materialization per `get`; keyed child layers remain outside `get`.

Materialization has a per-record decoded-byte maximum. The materializer streams
the non-keyed body in accepted resource/member order. Before fully decoding a
variable-sized stored leaf, the materializer must prove an encoded or checked
upper bound for that leaf that fits in the remaining per-record budget. Fixed
size leaves use their static checked bound. If no such bound is available, the
leaf fails with `MaterializationLimit` before allocating the decoded value. A
decoded leaf is then charged to the accumulated decoded size; an implementation
bug that lets the decoded value exceed the proved bound is a materializer bug,
not a public contract. A `surface.limit` result does not certify that the unread
remainder of the record is valid. It means the surface operation stopped at the
budget boundary before it could finish materializing the record.

The decoded-byte maximum is caller-scoped. Surface callers pass the bounded
surface budget. Existing `run`, saved-iterator, data-tooling, and integrity
callers pass an unbounded budget unless their own docs are changed in the same
lane.

A malformed identity is a request-shape error. Invalid attached data is
different: if materialization finds missing required data or a stored leaf in
the materialized non-keyed backing resource cannot decode as the checked type,
the operation raises a fatal data-attachment fault. That fault is not catchable
Marrow control flow and is not mapped to a 4xx client error. A surface host
isolates the fault to the current operation, returns a sanitized service fault
with public code `surface.invalid_data`, keeps serving other requests, and
relies on `marrow data integrity` or repair workflows to identify and fix the
corrupt record. Transport profiles may render that code as an HTTP 5xx service
fault, but clients never receive source paths, physical keys, catalog rows, or
repair details.

The implementation of `get` must start from a shared typed materialization API,
not prose matching over runtime errors. The surface materialization outcome set
is closed over `AbsentRecord`, `MissingRequired`, `DecodeInvalid`,
`MaterializationLimit`, and `StoreFault`; runtime, tooling, and surface
renderers then map those variants to their own envelopes. Any new outcome
variant must update this mapping table, the `surface.*` and `decode.*` error
code docs, and the verification obligations in the same lane. `MissingRequired`
maps internally to the reserved
`decode.required_absent` taxonomy. `DecodeInvalid` subsumes `decode.value`,
`decode.shape`, and `decode.unknown_member`, carrying the precise internal code
for operator logs. Surface public envelopes use `surface.invalid_data` while
operator logs retain the precise decode code.

Initial surface mappings:

| Materialization outcome | `get` | collection row | create post-write | update pre-write | update post-write |
| --- | --- | --- | --- | --- | --- |
| `AbsentRecord` | `surface.absent` | `surface.invalid_data` | `surface.write` | `surface.absent` | `surface.write` |
| `MissingRequired` / `DecodeInvalid` (`decode.value`, `decode.shape`, `decode.unknown_member`) | `surface.invalid_data` | `surface.invalid_data` | `surface.write` | `surface.invalid_data` | `surface.write` |
| `MaterializationLimit` | `surface.limit` | `surface.limit` | `surface.limit` | `surface.limit` | `surface.limit` |
| `StoreFault` | `surface.store` | `surface.store` | `surface.store` | `surface.store` | `surface.store` |

The asymmetry is intentional. A well-formed point identity that names no record
is ordinary absence. A collection candidate whose index suffix names no backing
record is an integrity failure in the collection's backing data. Post-write
materialization failures before commit are generated-write failures, except for
bounded-serving limits and store faults, which keep their public codes.

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
bounded protocol value. The initial protocol maxima are 128 rows per page, 8 MiB
decoded non-keyed data per page, and 8 MiB decoded non-keyed data per record.
Invalid request limits fail with `surface.request`.

The protocol maxima are surface contract facts and are part of the operation
equality tag. A deployment may choose lower effective budgets without changing
the ABI; cursors bind the normalized effective budgets used for that page so
continuation cannot silently switch budget policy. The effective row budget
must be at least one row. The effective page decoded-byte budget must be at
least the protocol per-record decoded-byte maximum, so any record accepted by
the per-record surface limit can fit as the first row of a page.

A well-formed request whose next materialized row would exceed the effective
page decoded-byte budget returns the rows that already fit, if at least one row
fits, and a cursor after the last returned row. If the first candidate record
exceeds the per-record maximum, the page fails with `surface.limit` and returns
no advancing cursor. This can make the collection unavailable at that point
until an operator shrinks/deletes the record through maintenance or raises the
surface protocol maximum and regenerates affected clients. The failure is a
bounded-serving limit, not proof of data corruption.

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
Each collection page enumerates candidate identities and materializes returned
rows under one consistent read snapshot. Cross-page semantics remain keyset
continuation, not a stable multi-page snapshot.

### Create

`create title, author, blurb` generates a create operation only when the checker
can prove the store has a supported identity policy and the generated input can
produce a valid resource.

Generated create supports single-`int` identity stores using `nextId(^store)`.
Composite identity and non-integer identity creation are handled by explicit
`pub fn` functions until a separate key-input design is accepted.

Generated create inherits the documented `nextId(^store)` contract. It promises
only an opaque unused identity under that contract, with possible gaps; it does
not add a surface-specific promise of contiguous, monotonic, or never-reused
public identifiers. The allocation cost shape is the `nextId` cost shape, not a
separate surface allocator. Applications that need never-reused public identities
must use an explicit `pub fn` or wait for a separate persistent-allocation
policy.

Generated create is intentionally narrow in this design. Every required
non-identity top-level field must be supplied by the client; server-set required
fields such as `createdAt`, `ownerId`, `status`, and required fields inside
unkeyed groups are explicit `pub fn` territory until defaults, computed fields,
principal-aware inputs, or write-only input syntax are designed. The checker
diagnostic for an unsupported generated create must point to the missing field
and name the `pub fn` fallback.

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

Post-create materialization reads through the open operation transaction and
must observe staged, uncommitted writes. It is not a read from the pre-write
snapshot.

Managed-write failures abort the operation transaction with no durable mutation.
A generated unique-index conflict maps to `surface.conflict`. Identity
allocation exhaustion, checked-write invariant failures unreachable after
validation, and other write-plan failures map to `surface.write`.
Store/backend failures still map to `surface.store`.

The post-create materialization uses the same per-record decoded-byte maximum as
`get`. A generated create whose post-state exceeds that maximum fails with
`surface.limit` and commits nothing.

Sparse create fields can be omitted. Omitted sparse fields are absent. In the
JSON profile, `null` is not a deletion or absence marker for generated create.

### Update

`update title, blurb` generates a field-wise update operation. It is not a
whole-record replacement and it is not a generic raw setter.

The generated update operation:

- decodes an `Id(^store)`;
- proves the record exists before writing;
- materializes the full non-keyed backing resource body inside the operation
  transaction before writing, so corrupt private required data aborts before any
  durable mutation;
- accepts a non-empty typed update object whose keys are a subset of the
  declared update fields;
- receives typed field values already decoded by the active boundary profile;
- rejects unknown field keys before any write;
- writes only supplied fields;
- preserves omitted fields, unkeyed groups, keyed children, and unrelated data;
- updates generated indexes through the normal managed-write path;
- materializes and validates the post-update non-keyed backing resource state;
- commits only after the post-update state can produce the surface record
  projection, then returns that projection.

An empty generated-update object is a request-shape error and maps to
`surface.request`.

Update pre-write materialization reads the transaction's initial view. Post-write
materialization reads through the same open operation transaction and must
observe staged, uncommitted writes.

Managed-write failures use the same no-commit mapping as generated create:
unique-index conflict is `surface.conflict`, write-plan or allocation failure is
`surface.write`, and store/backend failure is `surface.store`.

The pre-update and post-update materializations use the same per-record
decoded-byte maximum as `get`. An update of an existing record that already
exceeds the maximum fails before writing with `surface.limit`; an update whose
post-state would exceed the maximum fails with `surface.limit` and commits
nothing.

In the JSON profile, explicit `null` is rejected. Deletion, clearing sparse
fields, whole-record replacement, and keyed-child updates are outside generated
update.

## Surface ABI Facts

The checker produces derived transport-neutral `SurfaceAbi` facts. Boundary
profiles consume these facts instead of reinterpreting source strings.

`SurfaceAbi` is not a new catalog namespace and it is not persisted metadata.
There are no stable surface or operation catalog IDs. A surface fact references
the existing owners:

- the module-qualified surface alias from source;
- the surface-local ABI namespace and every public operation/projection alias;
- the backing store and resource catalog IDs;
- the public member catalog IDs for `fields`, `create`, and `update`;
- the declared index catalog IDs used by collections;
- typed generated operation descriptors: operation kind, typed inputs, typed
  output projection, identity policy, collection traversal, limits, and
  materialization policy;
- generated read footprints and cost shapes computed by construction, reusing
  existing footprint/cost-shape machinery where useful;
- materialization policy and decoded-byte budget;
- checked managed-write summaries by reference once write slices are
  implemented;
- typed write effect summaries and generated-index effect summaries;
- typed cursor boundary descriptors once collections are implemented;
- typed surface fault taxonomy.

`SurfaceAbi` does not contain JSON object shapes, TypeScript names, HTTP routes,
wire identity spelling, request-parameter codecs, object-body codecs, or public
error envelope rendering. Those belong to boundary profiles that consume the
core ABI.

`SurfaceAbi` is available only when all referenced durable facts have accepted
catalog IDs. A project with pending catalog identity can still parse and check
surface syntax, but it cannot export a stable `SurfaceAbi`, run generated
surface operations, or produce generated clients. It reports a typed checker or
tooling diagnostic such as `check.surface_catalog_pending`. Source spelling or
proposal-only IDs must not be used as fallback ABI identity.

The ABI digest is a computed surface equality tag for the whole surface. It is
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
catalog IDs, referenced-entry catalog digests, surface-local operation alias,
public projection key spellings, projected member IDs, collection alias and
index ID when present, request shape descriptor, output projection descriptor,
materialization policy, read footprint, traversal order, protocol row/page
maxima, protocol per-record decoded-byte maximum, and typed cursor boundary
descriptor when present. For generated create/update operations, the tag also
includes public input key spellings, input member catalog IDs, identity
allocation or existence policy, managed-write effect summary, and
generated-index effect summary.

A boundary profile derives its own profile equality tag from the core operation
tag plus profile-specific facts such as JSON scalar forms, wire identity form,
request and body codecs, route aliases, enum member spelling as rendered in that
profile, projection key spellings as rendered in that profile, client type
names, cursor token format, and public error envelope rendering. An exposed enum
member rename therefore changes either the referenced catalog digest in the core
operation tag, the profile rendering facts, or both. Generated
JSON/TypeScript/HTTP clients send the profile tag.
Non-JSON host embeddings can use the core operation tag.

Generated clients and versioned transport profiles must send the expected
profile equality tag with every request. A mismatch fails with
`surface.abi_mismatch`. Bare route-only requests, when an HTTP renderer later
permits them, are current-contract requests; they do not receive the
stale-client guarantee.

Alias deprecation, compatibility windows, and generated adapters are deferred
until a compatibility design is accepted.

Surface operation/profile tags are separate from the reserved function
`signature_digest` model. `signature_digest` is for public `pub fn` invocation
identity. Surface tags describe generated store operations, projections, cursor
contracts, materialization policy, and boundary profiles. A later design can
reuse function signature facts when grouping `pub fn` workflows near surfaces,
but generated surface operations do not wait for that model.

## Surface Boundary Profiles

Surface boundary profiles adapt `SurfaceAbi` to concrete transports and client
generators. The first profile is JSON-oriented. It does not define Marrow
semantics, and it does not route through the `marrow run --format json` entry
boundary. `std::json` remains a scalar JSON helper inside Marrow programs; it
does not map JSON objects to resources.

### Scalar Forms

The JSON profile reuses the existing Marrow run/data JSON scalar forms where
those forms exist. The profile descriptor names the exact scalar profile, so
renderers do not invent independent encodings. A core `SurfaceAbi` can exist for
a scalar type before it has a JSON form, but the JSON profile cannot export an
operation that exposes that type until the profile is designed. Identity values
are the only specialized production form in this design.

### Identity Form

In production JSON-profile output, an `Id(^store)` appears in a typed position whose
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

This debug form is diagnostic only. It is not a production client contract and
is not rename-stable. Production identity positions use the surface-local form
whose expected store comes from the operation descriptor or generated client
type.

The JSON profile decoder owns only JSON structure. It delegates scalar key
decoding, key arity, and root/store validation to the core `Id(^store)` guard so
identity checks have one semantic owner.

HTTP path or query renderers can choose a compact route spelling, but that
spelling is presentation. It must round-trip through the typed identity guard.

### Read Request Parameters

The JSON profile point-`get` lane includes request-parameter decoding only for
`Id(^store)` arguments. Collection parameter and cursor decoding belong to the
collection profile lane. The request-parameter codec rejects malformed JSON,
wrong-store identities, wrong key arity, and wrong scalar key types.

### Output Codec

The JSON profile output codec is the single owner for surface-record projection
to JSON:

- public required fields render as values;
- sparse fields render as omitted when absent;
- JSON `null` is not emitted for Marrow absence;
- missing required saved data is a fatal data-attachment fault, not `null`;
- `Id(^store)` values render with the production identity form above;
- enum values render by accepted member identity and current public spelling;
- scalar values use the named JSON scalar profile;
- identity-typed fields render the identity value without dereferencing. A
  renderer profile that attempts to expose a dereferenced link must fail missing
  referents as a public integrity fault; it must not silently create a trusted
  link.

The renderer can additionally produce links for HTTP or hypermedia profiles, but
the ABI value is the typed identity. Links are presentation, not semantic
identity.

### Generated-Write Object Body Codec

This follow-on JSON-profile codec is the single owner for JSON-to-Marrow
decoding of generated create and update object bodies:

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

Generated writes do not prove that an identity-typed input's referenced record
exists. They store typed identity values after root, arity, and scalar-key
validation. If referent existence is a write invariant, the application uses an
explicit `pub fn`. Integrity tooling and dereferencing renderer profiles remain
the places that detect missing referents.

## Cursor Boundary

Surface cursors are typed keyset tokens. They are not engine cursors and they do
not expose raw physical keys.

A cursor token is bound to:

- operation equality tag;
- profile equality tag for cursors minted by a boundary profile;
- store UID;
- engine profile digest;
- normalized operation parameters;
- traversal order and projection;
- page size;
- effective decoded-byte budget;
- last returned typed key boundary;
- cursor codec version.

The cursor is intentionally not bound to the ordinary commit ID, so normal
writes do not stale every page. A restore, restore-replace, store swap, data-dir
replacement, profile change, cursor codec change, operation ABI change, or store
UID change fails with `surface.stale_cursor`. Surface-relevant source and
catalog facts are bound through the operation equality tag; unrelated source or
catalog changes do not stale a cursor by themselves. Cursor-lineage nonce is a
deferred alternative lineage anchor. The initial collection lane relies on store
UID, and restore, restore-replace, store swap, data-dir replacement, or native
store reinitialization must mint a fresh store UID before surface serving
resumes. The collection lane must test that a cursor issued before such a
lineage change returns `surface.stale_cursor`.

A well-formed cursor whose normalized operation parameters do not match the
current request, including an index component such as `authorId`, is not stale
and not malformed. It fails with `surface.cursor` because the token is invalid
for this call.

Continuation uses a strict greater-than seek from the last returned key boundary
under the same operation parameters, ordering, projection, page size, decoded
budget, store lineage, operation tag, and profile tag when present. Surface
cursors do not promise stable multi-page snapshots across writes. A create whose
key sorts before the cursor boundary can be missed by later pages, and a delete
of a previously returned record does not prevent the next strict seek from
advancing. An update that changes an indexed field can move a record before the
cursor boundary and be missed by later pages. Store commit changes do not by
themselves stale a cursor.

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

Those route spellings are aliases over derived profile facts. Versioned
generated clients and transport profiles must send the expected profile equality
tag; otherwise they are current-contract route requests with no stale-client
guarantee.

The canonical identity segment format is a renderer decision, but it must be
decoded through the typed identity boundary and must avoid collision with
structural route names.

## Public Error Envelope

Boundary profiles expose a sanitized subset of the stable error envelope defined
in `docs/error-codes.md`; `docs/stability.md` names that envelope as a stable
machine-readable contract. The public surface envelope contains:

- `code`: stable dotted public error code;
- `message`: public diagnostic text suitable for the client boundary;
- `data`: optional structured public payload.

When a profile renders `kind`, it is the derived `surface` kind from the generic
error envelope. Profiles drop `source_span`, `help`, and internal diagnostic
`data` because surface responses are application egress, not operator repair
reports.

Initial public codes:

- `surface.request`: malformed request parameter, identity, index argument,
  limit, or generated-write body, excluding the cursor token;
- `surface.absent`: requested record identity is well-formed but no record node
  exists;
- `surface.cursor`: malformed cursor token, or a well-formed cursor whose bound
  normalized parameters do not match the current request;
- `surface.stale_cursor`: cursor token is well-formed but its operation equality
  tag, profile equality tag, or store lineage no longer matches the active
  operation facts;
- `surface.abi_mismatch`: generated client or transport request targets an ABI
  or profile slice that is no longer active;
- `surface.invalid_data`: backing saved data cannot be materialized under the
  checked resource shape;
- `surface.limit`: well-formed operation exceeds its materialization, row, or
  decoded-byte budget;
- `surface.conflict`: generated write conflicts with existing saved data, such
  as a unique-index conflict;
- `surface.write`: generated write could not be applied after successful request
  decoding and before commit, excluding conflicts and store/backend faults;
- `surface.integrity`: a renderer profile that dereferences identity links finds
  a missing referent;
- `surface.store`: the store reports a fault while serving a surface operation.

The public envelope does not include source spans, file paths, line numbers,
trace output, internal physical keys, raw catalog rows, or repair details.
Operator logs and tooling diagnostics can retain richer internal facts, but
surface clients branch only on public `code` and `data`.

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
- a host authorizes access to an entire surface. Initial surfaces have no
  principal, role, row-level, field-level, or per-operation authorization
  semantics;
- first-run catalog baseline and pending catalog proposals must be resolved
  before runtime surface operations or generated clients are available;
- a serving host holds the write-capable store handle for the lifetime of the
  surface runtime. Concurrent CLI or host writers are refused by the existing
  `store.locked` fence, and lock/store faults reached during serving map to
  `surface.store`;
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

Future principal or capability-aware serving must be expressed as typed checked
core facts consumed by `SurfaceAbi`. Boundary profiles may render or transport
principal/capability tokens, but they must not own authorization semantics.

## Diagnostics

Static surface diagnostics must be checker-native and specific:

- surface declaration collides with another module-level declaration;
- surface target is not a store;
- exposed field does not exist;
- exposed field is not supported by the selected boundary profile shape;
- selected boundary profile has no accepted scalar profile for an exposed field
  or input field;
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
- operation would require maintenance capability;
- accepted catalog IDs are not available for every referenced durable fact;
- derived surface ABI or profile facts changed in a way that requires client
  regeneration.

Diagnostics must point to the surface declaration and, where useful, the store,
field, or index declaration it references. Clients and tools branch on stable
codes and structured payloads, never prose.

Boundary runtime errors are not checker diagnostics. Request-parameter,
cursor-token, generated-write body, and scalar decode failures are owned by the
boundary profile and surface as `surface.request`, `surface.cursor`, or
`surface.stale_cursor`.

## Implementation Sequence

Implement as small independently reviewed lanes:

1. Language docs, parser, and formatter: `surface` declaration, contextual
   surface items, namespace rules, no runtime behavior.
2. Checker facts: resolve surfaces to accepted stores and supported fields,
   reject unsupported source shapes, and report pending catalog availability.
3. Core `SurfaceAbi` facts: surface-local ABI namespace, typed operation
   descriptors, projections, identities, effects, materialization policy,
   limits, and equality tags.
4. Shared typed materialization refactor: move the existing non-keyed resource
   materialization path to `AbsentRecord`, `MissingRequired`, `DecodeInvalid`,
   `MaterializationLimit`, and `StoreFault` outcomes, then migrate existing run,
   saved-iterator, tooling, and surface callers to explicit mappings. Existing
   command-visible `run.*`/`data.*` output must not change unless the owning docs
   move in the same lane.
5. Runtime identity guard consolidation: one guard owns store provenance, key
   arity, and scalar-key-type validation for lowered saved addresses, host
   embeddings, and all surface boundary profiles.
6. Point `get` executor: typed identity guard, materialization, projection
   output, no collections or cursors.
7. Typed keyset traversal API: root and exact-index scans resume from a full
   typed identity suffix, without serializing engine cursors.
8. Collection executor and typed cursor boundary over the typed traversal API.
9. JSON boundary profile facts: scalar profile reference, production identity
   form, generic debug identity form, request/body codecs, cursor token codec,
   profile equality tags, and public error envelope descriptors.
10. Generated TypeScript client rendering over the JSON profile.
11. Optional HTTP renderer over the JSON profile.
12. Generated-write object-body codec within the JSON profile.
13. Generated `create` for single-`int` `nextId(^store)` identity stores.
14. Generated `update` for top-level scalar, enum, and identity fields.

The first production test boundary for generated runtime operations is a
Marrow-owned library API in `marrow-run` or a new focused crate boundary, not
HTTP and not a test-only entrypoint. CLI/HTTP/TypeScript renderers layer on the
same facts later.

## Verification Obligations

Implementation lanes must add focused tests for the contracts that make surfaces
safe:

- operation equality tags change when operation kind, input members, projection
  members, public input/projection key spellings, aliases, identity policy,
  traversal order, protocol maxima, materialization policy, read footprint,
  write effects, or generated-index effects change;
- profile equality tags change when JSON scalar forms, wire identity form,
  request/body codecs, cursor token format, route aliases, enum member spelling,
  client type names, or public error envelope rendering change;
- lowering effective deployment budgets does not change the operation/profile
  ABI tags, while changing protocol maxima does;
- non-surface materialization callers pass an unbounded decoded-byte budget and
  keep existing `run.*`/`data.*` observable output unchanged;
- a single variable-sized leaf whose encoded or checked upper bound exceeds the
  remaining per-record budget fails with `MaterializationLimit` before full
  decode/allocation;
- `get` and each collection page materialize under one consistent read snapshot;
- post-create materialization observes the staged record inside the open
  operation transaction, and post-update materialization observes staged updated
  values before commit;
- generated create/update roll back with no durable mutation on
  `surface.conflict`, `surface.write`, and `surface.store` paths, including
  unique-index conflicts, identity allocation exhaustion, write-plan failures,
  and store/backend failures;
- generated create/update roll back with no durable mutation when the post-write
  non-keyed body exceeds the per-record decoded-byte maximum;
- restore, restore-replace, store swap, data-dir replacement, and native store
  reinitialization make pre-existing profile cursors fail with
  `surface.stale_cursor`;
- a cursor presented with different normalized operation parameters, including
  index component values, fails with `surface.cursor`;
- identity-typed generated-write inputs validate root, arity, and scalar key
  type through the shared runtime identity guard, and do not validate referent
  existence;
- invalid saved data reached by `get`, collection rows, or update pre-write
  materialization returns sanitized `surface.invalid_data` and logs
  repair-grade operator facts outside the public envelope, including precise
  `decode.required_absent`, `decode.value`, `decode.shape`, or
  `decode.unknown_member` codes;
- `MissingRequired` or any `DecodeInvalid` subtype reached by create/update
  post-write materialization returns `surface.write` and rolls back with no
  durable mutation, while `MaterializationLimit` remains `surface.limit` and
  `StoreFault` remains `surface.store`;
- exposed enum member spelling changes alter the operation or profile equality
  tag before generated clients silently see a different enum wire spelling.

## Deferred

Deferred deliberately:

- inline surface `action` blocks;
- public route declarations in `.mw`;
- principal/capability-aware serving beyond whole-surface host authorization;
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
- `docs/operations.md`: define initial local serving constraints and classify
  surface serving output plus generated-client runtime output in the
  egress-regime table.
- `docs/stability.md`: state how application surfaces supersede the older
  linked-Rust/TypeScript-export application-boundary sketch.
- implementation docs for checker facts, materialization, codecs, TypeScript
  renderer, and optional HTTP renderer.
