# Surface ABI Future Profiles

The active surface foundation is no longer a future proposal. The checker
derives `SurfaceReadOperationFact`s from checked `surface` declarations,
`SurfaceReadOperationAnalysis::stable_descriptor()` exposes the accepted-catalog
read descriptor for stable surfaces, `SurfaceUpdateOperationAnalysis` exposes
the accepted-catalog sparse-update descriptor for stable non-empty `update`
surfaces, `marrow-run` executes admitted transport-neutral reads and sparse
updates, and `marrow-json` owns the current check-output descriptor DTOs, read
request/result DTOs, typed cursor-boundary DTOs, sparse update request DTOs, and
an in-process operation-tag execution boundary over those DTOs.

This page tracks the profiles that are still intentionally deferred. They must
build on the active checker facts and descriptors. They must not introduce a
second saved-data access language, parse raw saved paths, use source labels as
semantic identity, or duplicate scalar, enum, identity, index, or projection
classification outside the checker-owned model.

## Active Foundation

The active surface foundation has these owners:

- `surface` declarations in `docs/language/resources-and-storage.md` define the
  language surface syntax and checked field/collection rules.
- `crates/marrow-check/src/surface.rs` resolves declarations to `SurfaceFact`
  and `SurfaceReadOperationFact` values. The parsed/resolved `create` list is
  reserved metadata and is excluded from the active serialized ABI profile.
- `crates/marrow-check/src/surface_abi.rs` owns the shared length-prefixed
  digest framing and typed descriptors for `surface.read.v1` and
  `surface.update.v1`.
- `AnalysisSnapshot::surface_read_operations()` returns snapshot-bound
  `SurfaceReadOperationAnalysis` values. A stable surface can produce a
  `SurfaceReadOperationDescriptor`; a source-only surface cannot.
- `AnalysisSnapshot::surface_update_operations()` returns snapshot-bound
  `SurfaceUpdateOperationAnalysis` values for surfaces with non-empty update
  sets. Stable surfaces can produce a sparse update descriptor; source-only
  surfaces cannot.
- `crates/marrow-run/src/surface.rs` admits stable read/update operations
  against a stamped store, materializes the backing record body before
  projecting reads, and applies non-empty sparse update patches through managed
  writes.
- `crates/marrow-json/src/surface.rs` owns the current checked read parameter,
  result, identity, value, typed cursor-boundary, sparse update request,
  operation-tag execution, and serialized surface ABI JSON DTOs. Execution
  accepts caller-supplied checked program and store references; serving, store
  open policy, route derivation, generated clients, and opaque cursor tokens
  remain separate profiles.

Operation tags are live runtime/json contracts. A change to either
`surface.read.v1` or `surface.update.v1` framing must either preserve byte
output or deliberately bump the profile version and accept stale cached
operation identity.

## Write Profiles

`marrow-run` owns the first transport-neutral surface write profile: an admitted
runtime update handle over stable, non-empty `SurfaceFact.update` declarations.
It applies non-empty sparse field patches addressed by accepted resource-member
catalog ID, preserves omitted fields, rejects absent records instead of
upserting, re-checks store/catalog lineage inside the write bracket before
mutation, validates the checked read footprint after the combined patch, and
maps conflicts/write/store failures through `surface.conflict`, `surface.write`,
and `surface.store`. `marrow-json` owns JSON request-body DTOs for those sparse
update patches.

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
facts. It serializes accepted catalog IDs, canonical read/update operation
descriptors, value shapes, operation tags, and render labels as labels. It must
not serialize checker-local IDs, source spans, raw saved paths, backend cursor
bytes, physical store keys, or `create` metadata.

`surface.update.v1` tags include the profile domain, store catalog ID, backing
resource footprint catalog ID, singleton-vs-point shape, identity-key value
shapes, `non_empty_patch` semantics, and the update field set sorted by accepted
resource-member catalog ID. Render labels and update declaration order do not
affect the tag. Read projection order remains ABI-semantic for
`surface.read.v1` because it is the output field order.

The descriptor profile may introduce a surface-level or package-level digest
when a real consumer needs one. Until then, per-operation tags are the only
stable equality values.

## Serving Profile

HTTP serving and local server lifetime remain deferred until a serving profile
maps the serialized descriptors to routes, envelopes, store-open policy, and
process lifetime. A production serving profile needs:

- routes derived from serialized ABI descriptors, not source names or ordinals;
- strict JSON-only request and response envelopes;
- sanitized `surface.*` error codes and no raw store details;
- loopback binding by default, because Marrow has no users or roles yet;
- read-only store admission for read serving, with no UID mint, baseline
  freeze, auto-apply, recovery, restore, maintenance, or hidden write path;
- an explicit architecture decision before adding an HTTP dependency.

A preparatory read-only serving session may be useful before HTTP, but it must
open the store through a no-write profile and expose no route or generated-client
contract.

## Generated Clients And LSP

Generated clients consume the serialized descriptor profile. Their naming and
route-rendering rules are a separate compatibility contract from operation
equality. Labels can guide rendering, but accepted catalog IDs and canonical
operation descriptors remain the semantic identity.

LSP and MCP tooling should consume
`AnalysisSnapshot::surface_read_operations()`,
`AnalysisSnapshot::surface_update_operations()`, and their
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
- ordinary `pub fn` workflow grouping;
- reachable throw-set descriptors;
- dry-run operation previews;
- opaque cursor token codecs;
- commit-bound snapshot pagination.
