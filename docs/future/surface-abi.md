# Surface ABI Future Profiles

The active surface foundation is no longer a future proposal. The checker
derives `SurfaceReadOperationFact`s from checked `surface` declarations,
`SurfaceReadOperationAnalysis::stable_descriptor()` exposes the accepted-catalog
read descriptor for stable surfaces, `SurfaceUpdateOperationAnalysis` exposes
the accepted-catalog sparse-update descriptor for stable non-empty `update`
surfaces, `SurfaceActionOperationAnalysis` exposes action descriptors over
`entry.invoke.v1`, `marrow-run` executes admitted transport-neutral reads,
sparse updates, and actions, and `marrow-json` owns the current check-output
descriptor DTOs, read request/result DTOs, typed cursor-boundary DTOs, sparse
update request DTOs, action request/result DTOs, and a transport-neutral JSON
operation envelope over those DTOs.

This page tracks the profiles that are still intentionally deferred. They must
build on the active checker facts and descriptors. They must not introduce a
second saved-data access language, parse raw saved paths, use source labels as
semantic identity, or duplicate scalar, enum, identity, index, or projection
classification outside the checker-owned model.

## Active Foundation

The active surface foundation has these owners:

- `surface` declarations in `docs/language/resources-and-storage.md` define the
  language surface syntax and checked field/collection/action rules.
- `crates/marrow-check/src/surface.rs` resolves declarations to `SurfaceFact`
  and `SurfaceReadOperationFact` values, including resolved public action
  function refs. The parsed/resolved `create` list is reserved metadata and is
  excluded from the active serialized ABI profile.
- `crates/marrow-check/src/surface_abi.rs` owns the shared length-prefixed
  digest framing and typed descriptors for `surface.read.v1` and
  `surface.update.v1`, plus action descriptors that reuse `entry.invoke.v1`
  identity, parameter shapes, and return shape.
- `AnalysisSnapshot::surface_read_operations()` returns snapshot-bound
  `SurfaceReadOperationAnalysis` values. A stable surface can produce a
  `SurfaceReadOperationDescriptor`; a source-only surface cannot.
- `AnalysisSnapshot::surface_update_operations()` returns snapshot-bound
  `SurfaceUpdateOperationAnalysis` values for surfaces with non-empty update
  sets. Stable surfaces can produce a sparse update descriptor; source-only
  surfaces cannot.
- `AnalysisSnapshot::surface_action_operations()` returns snapshot-bound
  `SurfaceActionOperationAnalysis` values for declared actions. Stable surfaces
  can produce an action descriptor; source-only surfaces cannot.
- `crates/marrow-run/src/surface.rs` admits stable read/update/action operations
  against a stamped store, materializes the backing record body before
  projecting reads, returns page cursors bound to the visible store commit, and
  applies non-empty sparse update patches through managed writes. It also admits
  surface actions by operation tag and fails closed when duplicate checked action
  tags exist.
- `marrow-run::ProjectSurfaceReadSession` is the preparatory linked-Rust
  read-serving boundary. It checks a project, opens the configured native store
  read-only, requires an already accepted and stamped durable store, fences
  source/catalog drift without auto-apply, and exposes admitted surface reads by
  operation tag. It does not expose routes, generated-client names, update
  methods, action execution, UID minting, baseline freeze, recovery, restore,
  maintenance, or any hidden write path.
- `marrow-run::ProjectSurfaceSession` is the linked-Rust read/write surface
  boundary. It checks a project, opens an existing configured native store
  writable, requires the same accepted catalog, store UID, commit metadata, and
  drift fence as the read session, and exposes admitted surface reads, sparse
  updates, and actions by operation tag without exposing the store handle. It is a
  single-owner, sequential session; while it is open, the native writer lock
  makes that session the owning process/session for reads and updates and
  excludes another writer or read-only inspection handle. It does not mint UIDs,
  freeze baselines, auto-create stores, auto-apply drift, repair catalogs,
  restore, recover beyond the native backend's writer-open replay, enter
  maintenance, derive routes, define generated CRUD semantics, or define a
  stable public Rust API.
- `crates/marrow-json/src/surface.rs` owns the current checked read parameter,
  result, identity, value, commit-bound typed cursor-boundary, sparse update
  request, action argument/result, operation-tag execution, transport-neutral
  operation envelope, and serialized surface ABI JSON DTOs. Execution accepts
  caller-supplied checked program and store references, read DTOs also execute
  against `ProjectSurfaceReadSession`, point/singleton update DTOs also execute
  against `ProjectSurfaceSession`, action DTOs execute against
  `ProjectSurfaceSession`, and `surface.operation.v1` dispatches read,
  sparse-update, and action request bodies by operation tag without exposing
  private store handles. The default project operation helper runs actions with
  a zero-capability host; callers that need host capabilities use the
  explicit-host helper. Serving, route derivation, generated clients, and opaque
  cursor tokens remain separate profiles. Serialized ABI export includes only
  callable read/update/action operation tags.

Operation tags are live runtime/json contracts. A change to either
`surface.read.v1`, `surface.update.v1`, or `entry.invoke.v1` action framing must
either preserve byte output or deliberately bump the relevant profile version and
accept stale cached operation identity.

## Operation Envelope Profile

The active JSON operation envelope is `surface.operation.v1`. It is
transport-neutral: callers supply a profile version, an operation tag, and one
typed request body; `marrow-json` admits the tag through `marrow-run` and then
lets the admitted read, update, or action handle validate the requested body
shape.

The request body variants are singleton read, point read, page, unique lookup,
singleton update, point update, and action. The response envelope echoes the
active profile version and operation tag and returns a record, page, optional
record, updated result, or action result. Action request arguments use the
existing `entry.invoke.v1` argument JSON shape for scalars, enums, identities,
and scalar/enum sequences; action results carry captured program output and an
optional surface action value DTO with accepted catalog IDs for enum and
identity values. Functions whose parameters or returns need resource, local-tree,
error, unknown, or unsupported sequence JSON are rejected as surface actions
until that codec is deliberately added. Page cursors carry the producing
operation tag,
store UID,
store commit ID, accepted-catalog digest, source digest, engine-profile digest,
and typed boundary. A cursor is valid only while the current store commit still
matches that lineage; a committed write between page requests makes the cursor
stale instead of silently continuing against a different saved-data snapshot.
This is a managed surface contract over store commits. Raw `TreeStore`
mutation without a commit metadata stamp is not a production surface write path
and does not preserve cursor semantics.
Error envelopes expose only a stable `surface.*` code and a public message; they
do not serialize spans, source paths, store paths, backend details, or runtime
internals.

`ProjectSurfaceReadSession` dispatches read request bodies only and fails
closed on update or action request bodies. `ProjectSurfaceSession` dispatches
stable read operation tags, sparse-update operation tags, and action operation
tags. Wrong profile versions and unknown tags are ABI mismatches. A known tag
with the wrong typed body shape is a request error. Runtime failures after an
action is admitted are sanitized as `surface.action`; action argument decode
failures are `surface.request`.

## Write Profiles

`marrow-run` owns the first transport-neutral surface write profile: an admitted
runtime update handle over stable, non-empty `SurfaceFact.update` declarations,
ordinary public-function actions over `entry.invoke.v1`, plus
`ProjectSurfaceSession` for linked-Rust project writes against an already
accepted and stamped native store. Sparse updates apply non-empty field patches
addressed by accepted resource-member catalog ID, preserve omitted fields,
reject absent records instead of upserting, re-check store/catalog lineage
inside the write bracket before mutation, validate the checked read footprint
after the combined patch, and map conflicts/write/store failures through
`surface.conflict`, `surface.write`, and `surface.store`. `marrow-json` owns
JSON request-body DTOs and linked project execution wrappers for those point and
singleton sparse update patches. Actions are ordinary checked Marrow functions:
their writes, transactions, host-effect checks, thrown errors, and return values
come from the language runtime rather than a generated CRUD side channel.

Future write work is still boundary-profile work, not HTTP or generated clients
by default:

- non-JSON transport body decoding for update patches, if a consumer needs it;
- create, because it needs explicit identity allocation or client-supplied
  identity rules, required-field completeness, replacement semantics, and
  return-shape decisions;
- delete, because no surface delete fact exists yet and idempotent vs absent
  semantics need a profile decision.

## Serialized Descriptor Profile

The active serialized ABI profile lives in `marrow check --format json|jsonl`
under `surface_abi` and is rendered by `marrow-json` DTOs from checker-owned
facts. It serializes accepted catalog IDs, callable canonical read/update/action
operation descriptors, value shapes, entry parameter shapes, operation tags, and
render labels as labels. It must not serialize checker-local IDs, source spans, raw saved paths,
backend cursor bytes, physical store keys, or `create` metadata.

`SurfaceAbiJson` curates duplicate stable operation tags out of the export. If a
stable operation tag is duplicated anywhere in the checked program, every read,
update, or action descriptor with that tag is omitted from the serialized ABI.
Runtime tag admission still fails closed on duplicates in checked facts. Checker
diagnostics for duplicate stable tags are a reserved future improvement, not
current behavior.

`surface.update.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, `non_empty_patch` semantics, and the update field set sorted by accepted
resource-member catalog ID. Render labels and update declaration order do not
affect the tag. Read projection order remains ABI-semantic for
`surface.read.v1` because it is the output field order.

Surface action operation tags are the resolved function's `entry.invoke.v1`
entry tag. That tag includes profile domain, canonical entry name, return
presence, return type shape, parameter names, and parameter type shapes, using
accepted catalog IDs for enum and identity leaves. Changing an action return
type or any returned durable identity therefore changes the operation tag just
like changing an argument shape.

The descriptor profile may introduce a surface-level or package-level digest
when a real consumer needs one. Until then, per-operation tags are the only
stable equality values.

## Serving Profile

HTTP serving and local server lifetime remain deferred until a serving profile
maps the serialized descriptors and active operation envelope to routes,
store-open policy, and process lifetime. A production serving profile needs:

- routes derived from serialized ABI descriptors, not source names or ordinals;
- strict JSON-only transport around the active operation envelope;
- sanitized `surface.*` error codes and no raw store details;
- an explicit choice between the active commit-bound cursor DTOs and a separate
  opaque cursor-token profile;
- loopback binding by default, because Marrow has no users or roles yet;
- read-only store admission for read serving, with no UID mint, baseline
  freeze, auto-apply, recovery, restore, maintenance, or hidden write path;
- an explicit architecture decision before adding an HTTP dependency.

The active `ProjectSurfaceReadSession` and `ProjectSurfaceSession` satisfy only
the preparatory linked-Rust project slices. They are not the serving profile:
they have no route mapping, process lifetime, network binding, generated-client
surface, opaque cursor token, or public compatibility guarantee.

## Generated Clients And LSP

Generated clients consume the serialized descriptor profile. Their naming and
route-rendering rules are a separate compatibility contract from operation
equality. Labels can guide rendering, but accepted catalog IDs and canonical
operation descriptors remain the semantic identity.

LSP and MCP tooling should consume
`AnalysisSnapshot::surface_read_operations()`,
`AnalysisSnapshot::surface_update_operations()`,
`AnalysisSnapshot::surface_action_operations()`, and their
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
