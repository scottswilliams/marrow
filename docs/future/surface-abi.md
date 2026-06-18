# Surface ABI

`SurfaceAbi` is the proposed application-read boundary over checked
`surface` declarations. It is not implemented in v0.1, adds no `.mw` syntax,
and does not define HTTP routes, a local server, TypeScript names, JSON wire
spelling, or a screen console. It consumes the checked facts produced by the
language described in [Resources And Saved Data](../language/resources-and-storage.md#application-surfaces).

The purpose of this profile is to make Marrow's database-language model usable
as an application boundary without adding a second query language or a raw saved
path API. A read operation is derived from the store, index, identity, type, and
projection facts the compiler already checked.

## Inputs And Stability

`SurfaceAbi` is computed from `SurfaceFact`, `CheckedFacts`, and the accepted
catalog. It does not re-resolve source paths, reclassify schema validity, parse
raw saved paths, or inspect human labels for semantic identity.

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
hash of one operation's catalog-id-bound ABI slice and the profile version.
Catalog epoch is too coarse to fence a surface operation or cursor by itself.

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

Every collection row carries the typed `Id(^store)` and the declared projection.
The identity is the database-language handle produced by root or index
traversal; it is not hidden whole-record materialization.

Non-unique index collection pages require exact arguments for every
non-identity index component, including identity-typed reference components.
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
- cursor token.

The decode boundary conceptually reuses Marrow's scalar, key, enum, and identity
rules. It is not a raw saved-path parser and must not duplicate semantic
classifiers outside the checker/runtime facts.

`surface.request` is the request-input failure code for non-cursor read
parameters: malformed identity keys, index arguments, direction/order, or
limits. `surface.cursor` is for cursor token codec failures and for well-formed
cursors whose normalized parameters do not match the current request.

Generated write object-body decode is outside this read profile.

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

## Projection And Integrity

A read decodes only the identity boundary, branch or index key cells needed for
the operation, and declared projected top-level fields. It does not materialize
the full record body, descend child layers, or inspect unprojected groups or
fields.

Read success means the projected surface row is valid. It does not prove whole
record validity. Unprojected corrupt or missing data remains a data-integrity
tooling concern.

Absence and invalid data are separated:

- an empty collection page is normal;
- a missing requested identity or singleton node is `surface.absent`;
- a sparse projected field that is absent is normal presence state;
- an absent required projected field on an existing row is
  `surface.invalid_data`;
- malformed projected scalar, enum, or identity values are
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
- direction and order;
- operation equality tag;
- store and profile lineage.

Token or display encoding is profile-owned. A remote or generated-client profile
may render an opaque token. A future console profile may render the typed
boundary directly. Those renderings share one continuation semantics.

Each request observes one store snapshot while that request is open. A cursor is
a keyset continuation over the latest admitted snapshot on the next request; it
does not hold a cross-request snapshot or lock. Concurrent writes between pages
can change later pages under normal keyset semantics. `surface.stale_cursor`
means lineage, profile, or operation invalidation, not ordinary data mutation.

Cursor payloads never expose backend cursor bytes, raw saved paths, or physical
store keys.

## Error Codes

The canonical code definitions live in [Error Codes](../error-codes.md). This
profile maps its future read boundary to the reserved codes there:

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

No `surface.*` or `check.surface_*` code in this section appears in v0.1 command
output until its owning surface ships.

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

This proposal changes no v0.1 LSP behavior. When `SurfaceAbi` operations become
checked facts or public analysis API, Marrow must expose them through a
query-native, snapshot/versioned analysis boundary first. `marrow-lsp` consumes
those facts; it does not re-derive operations from syntax.
