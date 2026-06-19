# Surface ABI Future Profiles

The active surface-read foundation is no longer a future proposal. The checker
derives `SurfaceReadOperationFact`s from checked `surface` declarations,
`SurfaceReadOperationAnalysis::stable_descriptor()` exposes the accepted-catalog
read descriptor for stable surfaces, `marrow-run` executes admitted
transport-neutral reads, and `marrow-json` owns the current read request,
result, and typed cursor-boundary DTOs.

This page tracks the profiles that are still intentionally deferred. They must
build on the active checker facts and descriptors. They must not introduce a
second saved-data access language, parse raw saved paths, use source labels as
semantic identity, or duplicate scalar, enum, identity, index, or projection
classification outside the checker-owned model.

## Active Foundation

The active read foundation has these owners:

- `surface` declarations in `docs/language/resources-and-storage.md` define the
  language surface syntax and checked field/collection rules.
- `crates/marrow-check/src/surface.rs` resolves declarations to `SurfaceFact`
  and `SurfaceReadOperationFact` values.
- `crates/marrow-check/src/surface_abi.rs` owns the `surface.read.v1`
  length-prefixed digest framing and typed read-operation descriptor.
- `AnalysisSnapshot::surface_read_operations()` returns snapshot-bound
  `SurfaceReadOperationAnalysis` values. A stable surface can produce a
  `SurfaceReadOperationDescriptor`; a source-only surface cannot.
- `crates/marrow-run/src/surface.rs` admits stable read operations against a
  stamped store and materializes the backing record body before projecting.
- `crates/marrow-json/src/surface.rs` owns the current checked read parameter,
  result, identity, value, and typed cursor-boundary JSON DTOs.

The operation tag is a live runtime/json cursor contract. A change to the
`surface.read.v1` framing must either preserve byte output or deliberately bump
the profile version and accept stale-cursor behavior for old cursors.

## Write Profiles

`marrow-run` owns the first transport-neutral surface write profile: an admitted
runtime update handle over stable, non-empty `SurfaceFact.update` declarations.
It applies non-empty sparse field patches addressed by accepted resource-member
catalog ID, preserves omitted fields, rejects absent records instead of
upserting, re-checks store/catalog lineage inside the write bracket before
mutation, validates the checked read footprint after the combined patch, and
maps conflicts/write/store failures through `surface.conflict`, `surface.write`,
and `surface.store`.

Future write work is still boundary-profile work, not HTTP or generated clients
by default:

- a serialized descriptor/digest for update operations using the same
  checker-owned framing owner, with its own profile domain/version;
- JSON or other transport body decoding for update patches;
- create, because it needs explicit identity allocation or client-supplied
  identity rules, required-field completeness, replacement semantics, and
  return-shape decisions;
- delete, because no surface delete fact exists yet and idempotent vs absent
  semantics need a profile decision.

## Serialized Descriptor Profile

A serialized ABI descriptor is a boundary profile, not a checker fact. It should
live in `marrow-json` after read and write descriptors share one checker-owned
digest framing model. The wire descriptor should serialize accepted catalog
IDs, canonical operation descriptors, input/output value shapes, operation
tags, and render labels as labels. It must not serialize checker-local IDs,
source spans, raw saved paths, backend cursor bytes, or physical store keys.

The descriptor profile may introduce a surface-level or package-level digest
when a real consumer needs one. Until then, per-operation tags are the only
stable equality values.

## Serving Profile

HTTP serving and local server lifetime are deferred until the serialized
descriptor and write-body decode exist. A production serving profile needs:

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

LSP and MCP tooling should consume `AnalysisSnapshot::surface_read_operations()`
and `stable_descriptor()`. They should present operation summaries without
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
