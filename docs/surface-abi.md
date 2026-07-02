# Surface ABI

The surface foundation is an active local application boundary. The checker
derives `SurfaceReadOperationFact`s from checked `surface` declarations,
`SurfaceReadOperationAnalysis::stable_descriptor()` exposes the accepted-catalog
read descriptor for stable surfaces, `SurfaceUpdateOperationAnalysis` exposes
the accepted-catalog sparse-update descriptor for stable non-empty `update`
surfaces, `SurfaceCreateOperationAnalysis` exposes exact-body create
descriptors for stable non-empty `create` surfaces,
`SurfaceDeleteOperationAnalysis` exposes full-subtree delete descriptors for
stable `delete` surfaces, `SurfaceActionOperationAnalysis` exposes action
descriptors over `entry.invoke.v1`, `SurfaceComputedReadOperationAnalysis`
exposes computed-read descriptors over read-only public functions, `marrow-run`
executes admitted transport-neutral reads, computed reads, creates, sparse
updates, deletes, and actions, and `marrow-json` owns the current check-output
descriptor DTOs, request/result DTOs, typed cursor-boundary DTOs, a
transport-neutral JSON operation envelope over those DTOs, and the thin
TypeScript client renderer over ABI plus routes. The remote cursor-token
profile is a transport/profile layer over the existing typed cursor DTO.

This page owns the active ABI and the explicitly deferred profiles that must
build on it. Deferred profiles must not introduce a second saved-data access
language, parse raw saved paths, use source labels as semantic identity, or
duplicate scalar, enum, identity, index, or projection classification outside
the checker-owned model.

## Active Foundation

The active surface foundation has these owners:

- `surface` declarations in `docs/language/resources-and-storage.md` define the
  language surface syntax and checked field, collection, action, computed-read,
  create, update, and delete rules.
- `crates/marrow-check/src/surface.rs` resolves declarations to `SurfaceFact`
  and `SurfaceReadOperationFact` values, including resolved public action
  function refs. Read operation facts carry the generated `get` alias or the
  declared collection alias as render metadata. Computed-read facts carry
  resolved public read-only function refs and declared aliases. Non-empty
  `create` lists, non-empty `update` lists, and `delete` declarations are
  active generated operation facts over checked top-level projection fields.
- `crates/marrow-check/src/entry_abi.rs` owns public-function surface value
  shapes: `entry.invoke.v1` identity, the single `EntryParameterShape` parameter
  carrier (`Present`/`Optional`, presence read off the parameter type rather than
  a parallel flag), the single `EntryResultShape` result carrier
  (`Void`/`Present`/`Optional`, presence read off the return type),
  scalar/enum/identity/sequence result shapes, and computed-read resource result
  fields with accepted catalog ids. The operation-tag parameter-optionality
  component, the serialized parameter descriptor's presence marker, and the
  decoder's absent-binding on both the JSON protocol and text `--arg` paths all
  derive from that one parameter carrier.
- `crates/marrow-check/src/surface_abi.rs` owns the shared length-prefixed
  digest framing and typed descriptors for `surface.read.v1`,
  `surface.computed_read.v1`, `surface.create.v1`, `surface.update.v1`, and
  `surface.delete.v1`, plus action descriptors that reuse `entry_abi` callable
  identity, parameter shapes, and return shape.
- `AnalysisSnapshot::surface_read_operations()` returns snapshot-bound
  `SurfaceReadOperationAnalysis` values. A stable surface can produce a
  `SurfaceReadOperationDescriptor`; a source-only surface cannot.
- `AnalysisSnapshot::surface_update_operations()` returns snapshot-bound
  `SurfaceUpdateOperationAnalysis` values for surfaces with non-empty update
  sets. Stable surfaces can produce a sparse update descriptor; source-only
  surfaces cannot.
- `AnalysisSnapshot::surface_create_operations()` returns snapshot-bound
  `SurfaceCreateOperationAnalysis` values for surfaces with non-empty create
  sets. Stable surfaces can produce an exact-body create descriptor; source-only
  surfaces cannot.
- `AnalysisSnapshot::surface_delete_operations()` returns snapshot-bound
  `SurfaceDeleteOperationAnalysis` values for surfaces with a `delete`
  declaration. Stable surfaces can produce a full-subtree delete descriptor;
  source-only surfaces cannot.
- `AnalysisSnapshot::surface_action_operations()` returns snapshot-bound
  `SurfaceActionOperationAnalysis` values for declared actions. Stable surfaces
  can produce an action descriptor; source-only surfaces cannot.
- `AnalysisSnapshot::surface_computed_read_operations()` returns
  snapshot-bound `SurfaceComputedReadOperationAnalysis` values for declared
  computed reads. Stable surfaces can produce a computed-read descriptor;
  source-only surfaces cannot.
- `crates/marrow-run/src/surface.rs` admits stable read, computed-read, create,
  update, delete, and action operations against a stamped store,
  materializes the bounded unkeyed backing-record body before projecting reads,
  returns page cursors bound to the visible store commit, applies exact create
  bodies and non-empty sparse update patches through managed writes, deletes
  whole record subtrees, and admits surface actions and computed reads by
  operation tag. Runtime admission fails closed when any active checked surface
  operation tag is duplicated.
- `marrow-run::ProjectSurfaceReadSession` is the preparatory linked-Rust
  read-serving boundary. It checks a project, opens the configured native store
  read-only, requires an already accepted and stamped durable store, fences
  source/catalog drift without auto-apply, and exposes admitted surface reads
  and computed reads by operation tag. It does not expose routes,
  generated-client names, create, update, delete, action execution, UID minting,
  baseline freeze, recovery, restore, maintenance, or any hidden write path.
- `marrow-run::ProjectSurfaceSession` is the linked-Rust read/write surface
  boundary. It checks a project, opens an existing configured native store
  writable, requires the same accepted catalog, store UID, commit metadata, and
  drift fence as the read session, and exposes admitted surface reads, computed
  reads, creates, sparse updates, deletes, and actions by operation tag without
  exposing the store handle. It is a single-owner, sequential session; while it
  is open, the native writer lock makes that session the owning process/session
  for reads and writes and excludes another writer or read-only inspection
  handle. It does not mint UIDs, freeze baselines, auto-create stores,
  auto-apply drift, repair catalogs, restore, recover beyond the native
  backend's writer-open replay, enter maintenance, derive routes, define
  generated client names, or define a stable public Rust API.
- `crates/marrow-json/src/surface.rs` owns the current checked read parameter,
  result, identity, value, commit-bound typed cursor-boundary, create request,
  sparse update request, delete request, action argument/result, computed-read
  argument/result, operation-tag execution, transport-neutral operation
  envelope, route manifest, and serialized surface ABI JSON DTOs.
  Execution accepts caller-supplied checked program and store references, read
  and computed-read DTOs also execute against `ProjectSurfaceReadSession`,
  point/singleton update DTOs, point/singleton create DTOs, point/singleton
  delete DTOs, and action DTOs execute against `ProjectSurfaceSession`, and
  `surface.operation.v1` dispatches read, computed-read, create, sparse-update,
  delete, and action request bodies by operation tag
  without exposing private store handles. Project operation helpers always run
  computed reads with a zero-capability host because host-effecting computed
  reads are rejected by the checker. The default project operation helper runs
  actions with a zero-capability host; callers that need action host
  capabilities use the explicit-host helper. `marrow serve` is the first
  HTTP serving profile: a loopback-first endpoint with a hand-rolled bounded
  HTTP parser over
  descriptor-derived
  `/surface/v1/{read|create|update|delete|action}/<operation-tag>` routes and
  `surface.operation.v1` envelopes. Range page reads and computed reads use the
  read route prefix. It
  defaults to read-only serving and exposes create/update/delete/action routes
  only with `--write`. The remote profile can wrap page cursors in
  `surface.cursor_token.v1` tokens without changing checker or runtime cursor
  semantics.
  `marrow client typescript` is the first generated-client profile: a
  self-contained TypeScript wrapper over the same route manifest and operation
  envelope. Its default profile keeps typed cursor DTOs; `--cursor-token`
  renders the remote cursor-token profile with string cursor brands and a
  distinct client digest. Serialized ABI export includes only callable
  read, computed-read, create, update, delete, and action operation tags and
  routes derived from those exported descriptors.

Operation tags are live runtime/json contracts. A change to either
`surface.read.v1`, `surface.create.v1`, `surface.update.v1`,
`surface.delete.v1`, `surface.computed_read.v1`, or `entry.invoke.v1` action
framing must either preserve byte output or deliberately bump the relevant
profile version and accept stale cached operation identity.

## Operation Envelope Profile

The JSON operation envelope is `surface.operation.v1`. It is
transport-neutral: callers supply a profile version, an operation tag, and one
typed request body; `marrow-json` admits the tag through `marrow-run` and then
lets the admitted read, computed-read, create, update, delete, or action handle
validate the requested body shape. Ranged index page reads share the single
profile and reuse the page request/result envelope shape, so every checked
operation is reachable from one route manifest and one generated client.

The operation envelope and the surface-owned request DTOs are closed JSON
objects. Unknown fields are request errors rather than extension points; new
request fields require an explicit profile/version decision.

The request body variants are singleton read, point read, page, unique lookup,
computed read, singleton create, point create, singleton update, point update,
singleton delete, point delete, and action. The response envelope echoes the
active profile version and operation tag and returns a record, page, optional
record, computed-read result, created record, updated result, deleted result, or
action result. Action and computed-read request arguments use the existing
`entry.invoke.v1` argument JSON shape for scalars, enums, identities, and
scalar/enum sequences. Action results carry captured program output and an
optional surface action value DTO with accepted catalog IDs for enum and
identity values. Functions whose parameters or returns need resource,
local-tree, error, unknown, or unsupported sequence JSON are rejected as surface
actions until that codec is deliberately added. An optional (`T?`) parameter or
return is presence on the wire only: present encodes the value, absent encodes
JSON `null` or an omitted optional argument, and the store never holds a null, so
absence is a transport encoding rather than stored data. Computed-read results
carry captured program output and an optional computed-read value DTO. Resource
computed-read results carry the accepted result resource catalog ID and accepted
resource-member catalog IDs for declared result fields. Page cursors carry the
producing operation tag, store UID, store commit ID, accepted-catalog digest,
source digest, engine-profile digest, and typed boundary. Exact index page
cursors bind the exact keys plus the last identity. Range page cursors bind the
exact keys, normalized range bounds, the last full index tuple, and the last
identity, so a cursor for one range or exact-key tuple cannot resume another.
A cursor is valid only while the current store commit still matches that
lineage; a committed write between page requests makes the cursor stale instead
of silently continuing against a different saved-data snapshot.
The remote cursor-token profile encrypts that typed cursor JSON inside a closed
`surface.cursor_token.v1` plaintext and authenticates the profile, key id,
request operation tag, and token mode as AEAD associated data. The token form is
`mct1.<kid>.<nonce>.<ciphertext>`, where nonce and ciphertext are canonical
unpadded base64url. A malformed token, wrong key id, tamper, or operation-tag
AAD mismatch is reported as `surface.cursor` because those cases are
indistinguishable from authenticated data failure. If decrypt succeeds and the
typed cursor lineage is stale, the existing runtime path reports
`surface.stale_cursor`.
This is a managed surface contract over store commits. Raw `TreeStore`
mutation without a commit metadata stamp is not a production surface write path
and does not preserve cursor semantics.
Error envelopes expose only a stable `surface.*` code and a public message; they
do not serialize spans, source paths, store paths, backend details, or runtime
internals.

`ProjectSurfaceReadSession` dispatches read and computed-read request bodies
only and fails closed on create, update, delete, or action request bodies.
`ProjectSurfaceSession` dispatches stable read, computed-read, create,
sparse-update, delete, and action operation tags. Wrong profile versions and
unknown tags are ABI mismatches. A known tag with the wrong typed body shape is
a request error.
Runtime failures after an action is admitted are sanitized as `surface.action`;
action argument decode failures are `surface.request`.
Runtime failures after a computed read is admitted are sanitized as
`surface.computed`; computed-read argument decode failures are `surface.request`.

## Write Profiles

`marrow-run` owns the first transport-neutral surface write profile: admitted
runtime create/update/delete handles over stable surface declarations, ordinary
public-function actions over `entry.invoke.v1`, plus `ProjectSurfaceSession` for
linked-Rust project writes against an already accepted and stamped native store.
Creates accept an exact declared field body addressed by accepted
resource-member catalog ID. A keyed store uses caller-supplied identity keys; a
singleton store takes no identity. Create rejects an existing record instead of
replacing it, writes the whole declared body through the managed resource-write
planner, validates the checked read footprint, and returns the public
projection. Sparse updates apply non-empty field patches, preserve omitted
fields, reject absent records instead of upserting, re-check store/catalog
lineage inside the write bracket before mutation, validate the checked read
footprint after the combined patch, and return no record body. Deletes reject an
absent record and remove the full backing record subtree plus generated index
rows through the managed delete planner. Generated writes map
conflicts/write/store failures through `surface.conflict`, `surface.write`, and
`surface.store`. `marrow-json` owns JSON request-body DTOs and linked project
execution wrappers for point and singleton creates, updates, and deletes.
Actions are ordinary checked Marrow functions: their writes, transactions,
host-effect checks, thrown errors, and return values come from the language
runtime rather than a generated CRUD side channel.

Future write work is still boundary-profile work, not HTTP by default:

- non-JSON transport body decoding for generated write requests, if a consumer
  needs it;
- explicit server-side identity allocation, if the language grows a checked
  allocator profile beyond caller-supplied identities;
- idempotent delete or upsert variants, if they are introduced as explicit
  operation semantics rather than silently changing the v0.1 create/delete tags.

## Serialized Descriptor Profile

The active serialized ABI profile lives in `marrow check --format json|jsonl`
under `surface_abi` and `surface_routes` and is rendered by `marrow-json` DTOs
from checker-owned facts. The `surface_abi` object serializes accepted catalog
IDs, callable canonical read/computed-read/create/update/delete/action operation
descriptors, store-operation value shapes, shared entry parameter/result shapes,
operation tags, read/computed-read/action aliases, and render labels
as labels. It must not serialize checker-local IDs, source spans, raw saved
paths, backend cursor bytes, physical store keys, or unowned request syntax.
Each callable parameter descriptor carries a required/optional presence marker
read off the parameter carrier, mirroring the result presence, so a generated
client can type an optional (`T?`) parameter as nullable and a required one as
required. Presence is a transport-boundary encoding — present to value, absent
to JSON `null` or an omitted argument — never a stored value.

`SurfaceAbiJson` curates duplicate stable operation tags out of the export. If
a stable operation tag is duplicated anywhere in the checked program, every
read, computed-read, create, update, delete, or action descriptor with that tag
is omitted from the serialized ABI.
Runtime tag admission still fails closed on duplicates in checked facts. Checker
diagnostics for duplicate stable tags are a reserved future improvement, not
current behavior.

`SurfaceRouteManifestJson` renders the `surface.route.v1` route manifest from
that already-curated `SurfaceAbiJson`. Each row is a strict JSON `POST` route
over `surface.operation.v1`, carries the admitted operation tag, and names the
surface label, operation alias, and expected request-body kind. Read routes use
`/surface/v1/read/{operation_tag}`, computed-read routes also use
`/surface/v1/read/{operation_tag}`, create routes use
`/surface/v1/create/{operation_tag}`, sparse-update routes use
`/surface/v1/update/{operation_tag}`, delete routes use
`/surface/v1/delete/{operation_tag}`, and action routes use
`/surface/v1/action/{operation_tag}`. Ranged index page reads are ordinary read
routes under `/surface/v1/read/{operation_tag}`. Route paths are derived from
operation tags, not source names, aliases, ordinals, or raw saved paths. Aliases
remain render/client labels; they are not route identity or operation equality.
Source-only surfaces and duplicate-tag operations have no route rows because
they have no callable descriptor rows.

A `paged_index_range_collection` read descriptor becomes a `range_page` route
under the read prefix. The descriptor carries the index catalog id,
`exact_key_count`, `range_key_index`, `identity_key_count`, and the existing
`index_keys` array so clients can identify the exact, ranged, and identity
components without reclassifying the index.

Range page request bodies use the normal page body plus a required `range`
object:

```json
{
  "exact_keys": [{ "kind": "string", "value": "fantasy" }],
  "range": {
    "lower": { "kind": "date", "days_since_epoch": 19723 },
    "lower_inclusive": false,
    "upper": { "kind": "date", "days_since_epoch": 19754 },
    "upper_inclusive": true
  },
  "limit": 20,
  "cursor": null
}
```

At least one bound is required. Bounds decode against the ranged scalar key
shape. `lower_inclusive` is required exactly when `lower` is present, and
`upper_inclusive` is required exactly when `upper` is present; omitting a side
leaves that side unbounded and carries no inclusivity flag. `lower_inclusive:
false` starts strictly after the lower key, `upper_inclusive: false` stops
strictly before the upper key, and the `true` form includes the matching
endpoint. Missing, `null`, empty range objects, or orphan inclusivity flags are
`surface.request`. Non-range page operations reject a `range` field as
`surface.request`.

`surface.create.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, exact declared-body semantics, singleton or caller-supplied identity
policy, reject-existing semantics, the create field set sorted by accepted
resource-member catalog ID, the public projection shape, and the full-record read
footprint used to validate the bounded unkeyed result body. Render labels and
create declaration order do not affect the tag.

`surface.update.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, `non_empty_patch` semantics, and the update field set sorted by accepted
resource-member catalog ID. Render labels and update declaration order do not
affect the tag. Read projection order remains ABI-semantic for
`surface.read.v1` because it is the output field order.

`surface.delete.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, and reject-absent full-subtree semantics. Delete returns no record body;
callers that need the deleted public projection read it before deleting.

Surface action operation tags are the resolved function's `entry.invoke.v1`
entry tag. That tag includes profile domain, canonical entry name, return-type
optionality (`T?`), return type shape, parameter names, parameter optionality,
and parameter type shapes, using accepted catalog IDs for enum and identity
leaves. A definite `T` and an optional `T?` are distinct tags on both the return
and each parameter, so `f(x: string)` and `f(x: string?)` differ while every
unchanged non-optional signature keeps its tag bytes. Changing an action return
type or any returned durable identity therefore changes the operation tag just
like changing an argument shape.

`surface.computed_read.v1` tags include the profile domain, the resolved
callable's entry identity, canonical function name, the single result carrier,
and checked cost-shape summary. The callable descriptor reuses the shared entry
parameter shapes and the `EntryResultShape` carrier, whose presence component is
read off the return type rather than a parallel flag. Resource result
descriptors use accepted resource and member catalog IDs, not source labels.
Alias changes do not affect the operation tag.

The descriptor profile may introduce a surface-level or package-level digest
when a real consumer needs one. Until then, per-operation tags are the only
stable equality values.

## Serving Profile

`marrow serve` maps the active route manifest and operation envelope to
a local HTTP process:

- serving routes are taken from explicit route profiles, not source names or
  ordinals;
- default mode exposes only descriptor-derived
  `/surface/v1/read/<operation-tag>` paths, including computed reads and range
  page reads;
- `--write` additionally exposes descriptor-derived
  `/surface/v1/create/<operation-tag>`,
  `/surface/v1/update/<operation-tag>` and
  `/surface/v1/delete/<operation-tag>`, and
  `/surface/v1/action/<operation-tag>` paths;
- the transport is JSON-only around the route's operation envelope profile;
- route operation profile, route operation tag, body operation tag, and body
  request kind must agree;
- errors use sanitized `surface.*` code/message envelopes with no raw store
  details;
- binding is loopback-only by default; `--remote` is an explicit authenticated
  profile with an external TLS expectation and no Marrow user/role model;
- `--cors-origin` optionally emits CORS headers for one exact loopback browser
  origin and never emits wildcard CORS;
- store admission uses `ProjectSurfaceReadSession` in default mode and
  `ProjectSurfaceSession` with `--write`, with no UID mint, baseline freeze,
  auto-apply, recovery, restore, maintenance, or hidden write path outside
  admitted create/update/delete/action operations;
- the HTTP parser processes at most one request per connection, requires exactly
  one `Content-Length` on operation `POST` requests, permits an empty CORS
  preflight to omit `Content-Length` or send `Content-Length: 0`, rejects
  `Transfer-Encoding` and already-buffered trailing bytes, caps headers and
  bodies, and closes every response.

The default serving profile intentionally reuses the active commit-bound typed
cursor DTOs in read responses and page requests. Remote serving can opt into
`surface.cursor_token.v1`, which seals those typed cursor DTOs into opaque
tokens at the HTTP boundary and opens them back into typed DTOs before runtime
admission. This profile does not add a saved-data access language or change
checker/runtime cursor semantics.

## TypeScript Client Profile

The TypeScript client maps the active serialized ABI and `surface.route.v1`
manifest to a self-contained TypeScript operation client. It validates route/ABI
agreement before rendering and requires a bijection: every exported operation
descriptor must have exactly one route row. It does not read or open the
saved-data store. A ranged index page read generates a typed range method (and a
`...Pages` iterator) that takes a `MarrowRange<T>` bound over the ranged key.

The exported factory is `createClient`. Every generated file begins with a
do-not-edit header, `// marrow-client-profile: typescript.v2`,
`// marrow-surface-digest: sha256:<hex>`, and
`// marrow-client-digest: sha256:<hex>` lines. The surface digest is the
deterministic ABI/route identity over the serialized surface ABI plus route
manifest, so two checkouts of the same surface shape produce the same surface
header. The client digest combines that identity with the TypeScript generator
profile. `marrow client typescript --cursor-token` uses
`// marrow-client-profile: typescript.v2+surface.cursor_token.v1` and a
different client digest over the same surface digest; declared client freshness
uses the default typed profile.

The client is a declared compile-fresh output, not a file the developer
regenerates by hand. A project names one output path in `marrow.json`'s `client`
field; `marrow run`, `marrow serve` startup, and `marrow evolve apply` rewrite
it write-if-changed whenever the generated client digest changes, exactly as
those paths re-project `marrow.lock`. The freshness key is the client digest,
not the whole source, so a non-surface `.mw` edit leaves the file untouched, and
a generator-profile change intentionally marks the client stale.
`marrow check --locked` fails when the declared client is absent or carries a
stale digest while the project declares a surface; plain `marrow check` is
read-only and advises instead. See
[tooling-surfaces.md](tooling-surfaces.md) and [cli.md](cli.md).

The generated code flattens to surface-name keys (`client.Books`) with one async
method per operation label. Each paged read also gets a client-only
`<methodName>Pages(...)` async iterator helper that yields `Page<Row, Cursor>`
values and advances the returned cursor between requests. JavaScript reserved
words and label collisions are resolved deterministically; these names are
render compatibility only. Operation equality remains the operation tag, which
the client carries in a private per-tag table; catalog IDs stay private
constants and never appear in a method signature.

The client decodes each response value against the descriptor as a **convenience
projection, not a second validation authority** — the server still validates
every request, and `marrow-json`, `marrow-run`, and `marrow serve` continue to
validate every request from clients that bypass generated code. The client's job
is to fail loud: a malformed or missing field throws a typed error rather than
silently producing wrong data. Decoding is exact — a Marrow `int` decodes to a
`bigint` (a JS `number` truncates above 2^53), an identity decodes to a branded
id, and an enum decodes by its member catalog id. Temporal and bytes scalars
decode to their faithful wire datum without loss: a `date` to its
day-count `number`, an `instant`/`duration` to its nanosecond-count `bigint`, a
`decimal` to its canonical text, and `bytes` to its base64 text. An identity key
carries each datum in the same faithful form, so a temporal or bytes key takes
its day count, nanosecond count, or base64 text rather than a display string.
Input `int` values accept `string | number | bigint` and reject unsafe `number`
inputs before serialization. The escape hatch `invokeRaw(options, operationTag, request)`
returns the undecoded `surface.operation.v1` envelope for callers who need the
wire shape.

### Calling A Generated Client

`createClient(options)` returns an object of async methods keyed by surface name.
Pass `baseUrl` for the running server (and `fetch` in a non-browser runtime).
Identities are branded — construct one with the generated `<surface>Id`
constructor. A reference to a store with no surface of its own brands after the
store's source name (`Id(^projects)` becomes the `ProjectsId` type and
`projectsId` constructor), so no catalog-id hash ever reaches a client symbol.
Create and update take name-keyed body objects:

```ts
import { createClient, booksId } from "./generated/marrow";

const client = createClient({ baseUrl: "http://127.0.0.1:8080" });

const book = await client.Books.get(booksId(10)); // typed Book record
book.title; // string
book.id; // BooksId

const created = await client.Books.create(booksId(11), {
  title: "Dune",
  author: "Frank Herbert",
});

const page = await client.Books.byAuthor("Frank Herbert", 20);
for (const row of page.rows) {
  row.title; // typed field
}
const next = await client.Books.byAuthor("Frank Herbert", 20, page.next);

for await (const page of client.Books.byAuthorPages("Frank Herbert", { limit: 20 })) {
  for (const row of page.rows) {
    row.title;
  }
}
```

Methods return decoded domain values: a point read returns the typed record, a
page returns `{ rows, next }` whose `next` cursor is preserved verbatim and
passed straight back as the next request cursor, a computed read returns the
decoded value directly, and an action returns `{ value, output }` (the `output`
string carries anything the action printed).
In the default client profile that cursor brand is the typed cursor DTO. In the
cursor-token client profile it is a branded string intended for a remote
`marrow serve` started with cursor-token mode. Both profiles use the same
method names, route manifest, operation tags, and `headers` option for remote
auth.

### Handling Errors

On any non-2xx response the generated method throws a typed `MarrowSurfaceError`
with a closed `code: SurfaceErrorCode` union and the unparsed `rawBody`. Branch
on `code`, not on the HTTP status or the human `message`. Use
`isMarrowSurfaceError(e)` to narrow:

```ts
import { isMarrowSurfaceError } from "./generated/marrow";

try {
  return await client.Books.get(booksId(999));
} catch (err) {
  if (!isMarrowSurfaceError(err)) throw err;
  switch (err.code) {
    case "surface.absent":
      return null; // record does not exist
    case "surface.abi_mismatch":
      throw new Error("client is stale; regenerate it"); // also 404
    case "surface.stale_cursor":
      return refetchFromStart(); // a write advanced the store; page again
    default:
      throw err;
  }
}
```

The codes a client author branches on:

| `code` | HTTP | Meaning | What to do |
|---|---|---|---|
| `surface.absent` | 404 | The requested record or singleton does not exist. | Treat as not found. |
| `surface.request` | 400 | The request parameters, fields, or arguments did not decode to the operation's input shape. | Fix the request shape; do not retry unchanged. |
| `surface.cursor` | 400 | A page cursor is malformed, tampered, wrong for the token context, or otherwise not decodable. | Discard the cursor and refetch from the start. |
| `surface.conflict` | 409 | A write conflicts with existing data, such as creating a record that already exists or violating a unique index. | Resolve the conflict (read first, or use update). |
| `surface.stale_cursor` | 409 | A page cursor is well-formed but a committed write moved the store past it. | Discard the cursor and refetch from the start. |
| `surface.abi_mismatch` | 404 | The operation the client targeted is no longer active — usually a stale client after a surface change. | Regenerate the client. |

Because `surface.absent` (record not found) and `surface.abi_mismatch`
(unknown or stale operation) both return **HTTP 404**, the status alone cannot
distinguish "no such record" from "stale client." Always branch on `code` in
the body. Other `surface.*` codes (such as `surface.limit`, `surface.write`,
`surface.action`, `surface.computed`, `surface.store`, and `surface.invalid_data`)
are server-side faults; surface them as a generic failure and inspect the
running server's tooling output. The full list is in
[error-codes.md](error-codes.md).

A surface change rotates affected operation tags, so an old generated client
sends tags the server no longer serves and gets `surface.abi_mismatch`. This
includes [enum](language/enums.md) changes: adding or renaming a member of an
enum that reaches the surface rotates those tags. Regenerate the client after a
surface change to clear the mismatch.

## Generated Clients And LSP

Generated clients consume the serialized descriptor and route profiles. Read,
computed-read, create, update, delete, and action aliases give renderers checked
operation labels, and `surface.route.v1` gives clients the canonical
operation-tag route path. Client method naming remains a separate compatibility
contract from operation equality. Labels can guide rendering, but accepted
catalog IDs and canonical operation descriptors remain the semantic identity.

LSP and MCP tooling should consume
`AnalysisSnapshot::surface_read_operations()`,
`AnalysisSnapshot::surface_create_operations()`,
`AnalysisSnapshot::surface_update_operations()`,
`AnalysisSnapshot::surface_delete_operations()`,
`AnalysisSnapshot::surface_action_operations()`,
`AnalysisSnapshot::surface_computed_read_operations()`, and their
`stable_descriptor()` methods. They should present operation summaries without
inventing routes, cursor encodings, generated client names, or a raw saved-path
protocol. Source-only surfaces are displayable as source-only facts, not stable
application ABI.

## Still Deferred

These remain out of scope until their profiles are proposed and implemented in
lockstep with docs, checker facts, runtime behavior, and JSON/tooling surfaces:

- nested projections and keyed-child reads;
- link dereference and incoming relations;
- human slash addresses and a console/screen renderer;
- operation curation syntax;
- reachable throw-set descriptors;
- dry-run operation previews;
- historical snapshot pagination across old commits.
