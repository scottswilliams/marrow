# Surface ABI

`SurfaceAbi` is the proposed application-read boundary over checked
`surface` declarations. The checker derives transport-neutral
`SurfaceReadOperationFact`s in v0.1, and `marrow-run` exposes an admitted
transport-neutral executor for the backing `SingletonRead` and `PointRead`
node-read shapes plus root/index collection reads. `marrow-json` owns a narrow
read DTO profile for checked read request parameters and already-executed
surface records, pages, values, identities, and typed cursors. Stable ABI
export, generated clients, opaque cursor token codecs, HTTP serving, and writes
remain future profiles. This adds no `.mw` syntax and does not define HTTP
routes, a local server, TypeScript names, write-body spelling, or a screen
console. It consumes the checked facts produced by the language described in
[Resources And Saved Data](../language/resources-and-storage.md#application-surfaces).

The purpose of this profile is to make Marrow's database-language model usable
as an application boundary without adding a second saved-data access language or
a raw saved-path API. A read operation is derived from the store, index, identity, type, and
projection facts the compiler already checked.

## Inputs And Stability

`SurfaceAbi` is computed from `SurfaceFact`, `CheckedFacts`, and the accepted
catalog. It does not re-resolve source paths, reclassify schema validity, parse
raw saved paths, or inspect human labels for semantic identity.

The checked fact layer records each read operation with checker-local ids, a
read kind, a `FullRecord` backing-resource footprint, and the ordered public
projection. When every catalog identity needed by the operation is accepted, it
also records an operation equality tag. Those facts are construction handles for
tooling and future profiles, not a serialized stable ABI by themselves.

A checked surface may be `SourceOnly` or `Stable`:

- `SourceOnly` surfaces still have derivable operation shapes for diagnostics,
  editor inspection, and explanation, but they have no stable exported digest
  and are not a generated-client ABI.
- `Stable` export requires `SurfaceCatalogStatus::Stable`. Any referenced store,
  resource member, index, enum, or identity shape without accepted catalog
  identity keeps the surface out of the stable ABI and is explained through the
  checked surface-catalog blockers.

Checker-local ids such as `SurfaceId`, `StoreId`, `ResourceMemberId`, and
`StoreIndexId` are construction handles only. Stable descriptors serialize
accepted catalog ids plus canonical structural slots: operation kind, projection
member order, checked input and output type descriptors, index key shape, and
the `SurfaceAbi` profile version.

Source names, surface names, field names, and collection aliases are render
labels. They may help a future renderer choose names, but operation equality,
cursor equality, and stable semantic compatibility do not depend on them. A
future generated-client naming profile may add a separate render compatibility
contract; this profile does not promise label stability.

No new store or catalog digest family is introduced. The surface compatibility
tag is a deterministic boundary-profile slice over accepted catalog facts and
canonical operation descriptors. An operation equality tag is the deterministic
hash of one operation's catalog-id-bound ABI slice and the profile version,
computed by the checker and consumed by runtime cursors. Catalog epoch is too
coarse to fence a surface operation or cursor by itself.

## Read Shapes

The read profile uses navigation vocabulary: node reads and collection pages,
not routes or endpoints.

| Shape | Source fact | Input | Result |
|---|---|---|---|
| `SingletonRead` | A `surface` whose backing store is keyless. | No identity input. | The singleton projection, or `surface.absent` if the singleton node is absent. |
| `PointRead` | A keyed backing store. | `Id(^store)`. | The identity plus projection, or `surface.absent` if the requested record node is absent. |
| `PagedRootCollection` | `collection ^store as alias` on a keyed store. | Page parameters. | Bounded rows in full `Id(^store)` order. |
| `PagedIndexCollection` | `collection ^store.index as alias` for a non-unique index. | Exact arguments for every non-identity index component plus page parameters. | Bounded rows under that exact index tuple, ordered by the trailing identity suffix. |
| `UniqueIndexLookup` | `collection ^store.index as alias` for a unique index. | The complete unique-index tuple. | Zero or one projected row. |

The transport-neutral `marrow-run` read API does not define routes or transport
request spelling. A point-read or collection-row result carries
`SurfaceReadIdentity`:
the accepted store catalog ID plus the typed key tuple. Projected fields are
ordered by the checked projection and carry the accepted resource-member catalog
ID plus a `SurfaceValue`. Scalars stay scalar; enum values carry the accepted
enum and member catalog IDs; identity values carry the accepted store catalog ID
plus the typed key tuple. `marrow-json` renders those already-executed results
as read-result DTOs with catalog IDs, typed keys, scalar value tags, base64 bytes,
lossless decimal strings for `int`, temporal nanoseconds, and decimals, and
render labels. Field and enum-member render labels may be present for
displays, but they are not semantic identifiers and do not participate in
compatibility.

Every collection row carries the typed `Id(^store)` and the declared projection.
The identity is the database-language handle produced by root or index
traversal; it is not hidden whole-record materialization.

Non-unique index collection pages require exact arguments for every
non-identity index component, including identity-typed reference components.
The declared non-unique index must end with the full backing store identity in
store declaration order, matching the ordinary saved-index schema invariant.
They never enumerate arbitrary non-identity components, apply non-key ordering,
or hide a scan behind a filter. Unique indexes are maybe-present lookups in this
profile and never produce paged collection operations.

Keyless stores do not have `Id(^store)` values, so they use `SingletonRead`.
Keyless root collections remain invalid.

## Request Decode

Inbound read parameters decode into canonical typed values derived from checked
facts:

- scalar kind plus canonical scalar value;
- enum catalog identity plus member identity;
- `Id(^store)` store catalog identity plus key tuple, including composite
  identities;
- identity-typed index arguments;
- exact index arguments;
- optional direction or order if a later profile exposes it;
- limit;
- typed cursor-boundary JSON.

The active JSON read request DTOs decode those parameters through shape APIs on
admitted `marrow-run` reads. They reuse Marrow's scalar, key, enum, identity,
and identity-index-key rules instead of parsing raw saved paths or duplicating
checker classifiers. Cursor boundary JSON is decoded under the collection
cursor boundary shape so malformed cursor lineage fields and boundary arguments
report `surface.cursor`, while ordinary identity and argument request failures
report `surface.request`.

`surface.request` is the request-input failure code for non-cursor read
parameters: malformed identity keys, index arguments, direction/order, or
limits. `surface.cursor` is for typed cursor-boundary failures, future cursor
token codec failures, and well-formed cursors whose normalized parameters do
not match the current request.

HTTP route mapping, opaque cursor-token codecs, generated clients, and generated
write object-body decode are outside this read profile.

## Store Admission

Serving a surface operation first admits the compiled surface profile and the
live store. The profile checks active operation compatibility and the store
lineage:

- store uid;
- the stamped catalog epoch, layout epoch, source digest, and engine-profile
  digest read from the commit stamp;
- accepted catalog digest.

The commit stamp is the single durable owner of the stamped catalog epoch,
layout epoch, source digest, and engine-profile digest. Surface serving uses
those stamped facts through the stamp instead of defining a second owner for
them. The accepted catalog snapshot remains the catalog-family owner of catalog
entries and digest, and the stamped commit and catalog snapshot must agree when
both are present.

`surface.stale_cursor` reports a well-formed cursor whose store lineage, profile
tag, or operation equality tag no longer matches. `surface.abi_mismatch` is
reserved for a future generated-client or transport profile targeting an
inactive semantic ABI/profile. Store-open and backend faults while serving map
to `surface.store`, not to request decode.

This read profile uses latest-snapshot pagination. It uses the stamp-owned
lineage facts above, but it does not compare `commit_id` or the per-commit
changed root/index id lists for cursor equality; a future commit-bound cursor
profile may add that stronger fence.

## Read Footprint And Projection

A read has two distinct extents. Its projection is the public top-level field
set the operation returns. Its read footprint is larger: every get, unique
lookup, and page row materializes the backing record body under the checked
resource shape before emitting the projection. That materialization decodes the
identity boundary, any branch or index key cells traversed by the operation, all
required top-level fields and unkeyed required groups of the backing record, and
the projected sparse fields.

That footprint has a cost. A surface projecting two fields of a record with ten
required fields still decodes the required backing body. A corrupt or absent
required non-public field fails the get or the whole page as
`surface.invalid_data`; rows that cannot be decoded are not skipped. Output
still carries only the public projection. The footprint does not descend keyed
child layers, so read success does not prove those deeper layers valid.

Read-parameter JSON decode belongs to this read slice: scalar, enum, identity,
index-argument, limit, and cursor-boundary request values decode against
checked facts before execution. Generated create/update object-body decode
remains deferred to a write profile.

Absence and invalid data are separated:

- an empty collection page is normal;
- a missing requested identity or singleton node is `surface.absent`;
- a sparse projected field that is absent is normal presence state;
- an absent required backing field on an existing row is
  `surface.invalid_data`, whether or not it is projected;
- malformed materialized scalar, enum, or identity values are
  `surface.invalid_data`;
- corrupt key or identity bytes reached by root or index traversal are
  `surface.invalid_data`;
- a dangling index entry whose identity points at no record fails the lookup or
  page as sanitized `surface.invalid_data`.

Rows that cannot be decoded must not be silently dropped or skipped. Repair
details, raw saved paths, and raw bytes stay in operator tooling.

`surface.integrity` remains reserved for future profiles that actively
dereference links or relations and need a missing-referent service fault.

## Pagination

Continuation semantics are fact-owned:

- typed boundary;
- normalized request parameters;
- fixed forward key order;
- operation equality tag;
- store and profile lineage.

The active transport-neutral runtime cursor uses typed Marrow keys, not an
encoded token. Root pages carry the last returned `Id(^store)` key tuple.
Non-unique index pages carry the exact index arguments plus the last returned
identity suffix. Unique lookups are zero-or-one reads and have no cursor.

Token or display encoding is profile-owned. The active read-result JSON DTO
renders the typed cursor boundary directly so a host can carry it without
inventing a saved-path format. It renders root and row identities as branded
surface identities, enum exact keys as enum catalog/member identities, and
identity-typed exact keys as branded identity arguments rather than raw saved
key bytes. The same DTO can be checked back into a typed runtime cursor under
the collection cursor boundary shape. A remote or generated-client profile may
wrap the same continuation semantics in an opaque token.

Each request observes one store snapshot while that request is open. A cursor is
a keyset continuation over the latest admitted snapshot on the next request; it
does not hold a cross-request snapshot or lock. Concurrent writes between pages
can change later pages under normal keyset semantics. `surface.stale_cursor`
means lineage, profile, or operation invalidation, not ordinary data mutation.

Cursor payloads never expose backend cursor bytes, raw saved paths, or physical
store keys.

## Error Codes

The canonical code definitions live in [Error Codes](../error-codes.md). This
profile maps its read boundary to the codes there:

- request parameter failures: `surface.request`;
- cursor codec or request-binding failures: `surface.cursor`;
- cursor lineage, profile, or operation invalidation: `surface.stale_cursor`;
- absent requested node: `surface.absent`;
- backing data reached by this read profile cannot be decoded:
  `surface.invalid_data`;
- execution row, materialization, or decoded-byte budget: `surface.limit`;
- store/backend fault while serving: `surface.store`;
- inactive future client or transport ABI/profile: `surface.abi_mismatch`.

The checker-side reserved codes are `check.surface_decl`,
`check.surface_catalog_pending`, and `check.surface_operation`. They remain
checker diagnostics, not runtime service faults.

`surface.request`, `surface.absent`, `surface.cursor`,
`surface.stale_cursor`, `surface.invalid_data`, `surface.limit`,
`surface.abi_mismatch`, and `surface.store` are active in the transport-neutral
`marrow-run` read API. They do not appear in v0.1 command output until a
command or transport profile owns that surface. The conflict, write, and
integrity codes remain reserved.

## Deferred Profiles

This proposal intentionally leaves these out of scope:

- generated writes, write-only inputs, write body decode, and delete;
- nested projections and keyed-child reads;
- link dereference and incoming relations;
- human slash addresses and a console/screen renderer;
- HTTP routes, server lifetime, transport security, and generated client names;
- operation curation syntax, ordinary `pub fn` workflow grouping, reachable
  throw sets, and dry-run;
- commit-bound snapshot pagination.

The landed `SurfaceFact` already records `create` and `update` field facts, but
this read profile does not turn them into ABI operations. A future write profile
must define request-body decode, conflict/write error mapping, sparse
update-only inputs, and transaction behavior in its own slice.

## Tooling And LSP

This proposal changes no v0.1 LSP behavior. `SurfaceReadOperationFact`s are
checked facts, and `AnalysisSnapshot::surface_read_operations` exposes them as
snapshot-bound views for editor and tooling consumers. `marrow-lsp` consumes
that analysis API when it adds an application-surface feature; it does not
re-derive operations from syntax.
