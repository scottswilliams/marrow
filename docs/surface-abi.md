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
TypeScript client renderer over ABI plus routes.

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
  shapes: `entry.invoke.v1` identity, parameter shapes, explicit result
  presence, scalar/enum/identity/sequence result shapes, and computed-read
  resource result fields with accepted catalog ids.
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
  materializes the backing record body before projecting reads, returns page
  cursors bound to the visible store commit, applies exact create bodies and
  non-empty sparse update patches through managed writes, deletes whole record
  subtrees, and admits surface actions and computed reads by operation tag.
  Runtime admission fails closed when any active checked surface operation tag
  is duplicated.
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
  capabilities use the explicit-host helper. `marrow surface serve` is the first
  HTTP serving profile: a loopback-only, dependency-free local endpoint over
  descriptor-derived
  `/surface/v1/{read|create|update|delete|action}/<operation-tag>` routes and
  `surface.operation.v1` envelopes. Computed reads use the read route prefix. It
  defaults to read-only serving and exposes create/update/delete/action routes
  only with `--write`.
  `marrow surface client typescript` is the first generated-client profile: a
  self-contained TypeScript wrapper over the same route manifest and operation
  envelope. Opaque cursor tokens, remote binding, and authentication remain
  separate profiles. Serialized ABI export includes only callable
  read, computed-read, create, update, delete, and action operation tags and
  routes derived from those exported descriptors.

Operation tags are live runtime/json contracts. A change to either
`surface.read.v1`, `surface.create.v1`, `surface.update.v1`,
`surface.delete.v1`, `surface.computed_read.v1`, or `entry.invoke.v1` action
framing must either preserve byte output or deliberately bump the relevant
profile version and accept stale cached operation identity.

## Operation Envelope Profile

The active JSON operation envelope is `surface.operation.v1`. It is
transport-neutral: callers supply a profile version, an operation tag, and one
typed request body; `marrow-json` admits the tag through `marrow-run` and then
lets the admitted read, computed-read, create, update, delete, or action handle
validate the requested body shape.

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
actions until that codec is deliberately added. Computed-read results carry
captured program output and an optional computed-read value DTO. Resource
computed-read results carry the accepted result resource catalog ID and accepted
resource-member catalog IDs for declared result fields. Page cursors carry the
producing operation tag, store UID, store commit ID, accepted-catalog digest,
source digest, engine-profile digest, and typed boundary. A cursor is valid only
while the current store commit still matches that lineage; a committed write
between page requests makes the cursor stale instead of silently continuing
against a different saved-data snapshot.
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
`/surface/v1/action/{operation_tag}`. Route paths are derived from operation
tags, not source names, aliases, ordinals, or raw saved paths. Aliases remain
render/client labels; they are not route identity or operation equality.
Source-only surfaces and duplicate-tag operations have no route rows because
they have no callable descriptor rows.

`surface.create.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, exact declared-body semantics, singleton or caller-supplied identity
policy, reject-existing semantics, the create field set sorted by accepted
resource-member catalog ID, the public projection shape, and the full-record read
footprint used to validate the result. Render labels and create declaration
order do not affect the tag.

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
entry tag. That tag includes profile domain, canonical entry name, return
presence, return type shape, parameter names, and parameter type shapes, using
accepted catalog IDs for enum and identity leaves. Changing an action return
type or any returned durable identity therefore changes the operation tag just
like changing an argument shape.

`surface.computed_read.v1` tags include the profile domain, the resolved
callable's entry identity, canonical function name, explicit result descriptor,
and checked cost-shape summary. The callable descriptor reuses the shared entry
parameter shapes and result presence. Resource result descriptors use accepted
resource and member catalog IDs, not source labels. Alias changes do not affect
the operation tag.

The descriptor profile may introduce a surface-level or package-level digest
when a real consumer needs one. Until then, per-operation tags are the only
stable equality values.

## Serving Profile

`marrow surface serve` maps the active route manifest and operation envelope to
a local HTTP process:

- serving routes are taken from `surface.route.v1`, not source names or ordinals;
- default mode exposes only descriptor-derived
  `/surface/v1/read/<operation-tag>` paths, including computed reads;
- `--write` additionally exposes descriptor-derived
  `/surface/v1/create/<operation-tag>`,
  `/surface/v1/update/<operation-tag>` and
  `/surface/v1/delete/<operation-tag>`, and
  `/surface/v1/action/<operation-tag>` paths;
- the transport is JSON-only around the active `surface.operation.v1` envelope;
- route operation tag, body operation tag, and body request kind must agree;
- errors use sanitized `surface.*` code/message envelopes with no raw store
  details;
- binding is loopback-only because Marrow has no users, roles, or authorization
  model yet;
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

The serving profile intentionally reuses the active commit-bound typed cursor
DTOs in read responses and page requests. A separate opaque cursor-token profile
remains future work. Remote serving, authn/authz, and an HTTP dependency also
remain future architecture decisions.

## TypeScript Client Profile

`marrow surface client typescript` maps the active serialized ABI and
`surface.route.v1` manifest to a self-contained TypeScript operation client. It
validates route/ABI agreement before rendering and requires a bijection: every
exported operation descriptor must have exactly one route row. It does not read
or open the saved-data store.

The generated code exposes sanitized module/surface namespaces and sanitized
operation-label methods. JavaScript reserved words and label collisions are
resolved deterministically; these names are render compatibility only.
Operation equality remains the operation tag, and method bodies store the
operation tag and descriptor-derived route path explicitly.

The client serializes `surface.operation.v1` request envelopes for read,
computed-read, create, sparse-update, delete, and action operations. It
preserves Marrow `int` values as strings on the wire and rejects unsafe
JavaScript `number` inputs before serialization. Response validation is
intentionally envelope-only: active profile version, echoed operation tag, and
expected result kind. The client
returns the server-owned JSON DTO payload; it does not build model classes,
prove catalog IDs, re-decode response values against descriptors, or become an
authority boundary. `marrow-json`, `marrow-run`, and `marrow surface serve`
continue to validate every request from clients that bypass generated code.

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
- opaque cursor token codecs;
- historical snapshot pagination across old commits.
