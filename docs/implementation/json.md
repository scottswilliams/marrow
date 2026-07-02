# Shared JSON

`marrow-json` owns small JSON DTOs that more than one CLI, tool, or application
boundary needs. It exists to keep `marrow run --format json`, trace, data
integrity, store-backed data inspection, surface reads, computed reads,
generated writes, actions, operation envelopes, and descriptor export from
copying run envelopes, run diagnostics, run result rendering, saved-key,
data-generation, surface descriptor/result rendering, checked surface
read/computed-read request decode, generated write request-body decode, and
action argument/result rendering. Surface descriptors
include checked read/computed-read/action aliases as labels for later route or
client renderers; those aliases are not operation identity. For surfaces it also
renders `surface.route.v1` route manifests from the active descriptor export and
provides an in-process operation-tag execution boundary over caller-supplied
`CheckedProgram` and `TreeStore` references, read-only execution helpers over
`ProjectSurfaceReadSession`, point/singleton create/update/delete and action
execution helpers over `ProjectSurfaceSession`, and `surface.operation.v1`
request/response/error envelope DTOs over project surface
sessions; it does not own HTTP serving or process lifetime policy.

The crate deliberately does not define a general `Value` JSON ABI. Its run DTOs
own the `result`/`output`/`diagnostics` envelope, bounded return-value surface,
runtime diagnostic payloads, optional store state, auto-applied transition, and
execution-boundary/run-fact projection. Run-fact projection is session-bound:
the boundary, boundary-carried analysis generation, entry admission, and
checked facts come from the same `ProjectSession`, not from caller text. The
standalone execution-boundary projector covers run and test sessions, with
`sourceAnalysisGeneration` as the single generation carrier. Scalars, enums,
identities, and
sequences render; whole resources and local trees fault at the CLI boundary as
`run.entry_surface`. The enum form
renders its stable `Enum::member` spelling — the reorder-invariant
`render_name` form that print/string/interpolation produce — and `int` values
remain JSON numbers for compatibility with current CLI consumers. Its
data-generation DTO
renders the shared `store_snapshot` object for `marrow data roots|get|stats`,
`marrow data dump --format json`, dump JSONL summaries, `marrow data integrity
--format json`, and integrity JSONL summaries, including the profile version,
store UID, catalog digest, optional commit stamp, open transaction stamp, and
checked source digest.
Its saved-data DTOs also render the bounded integrity advisory result shared by
the CLI, LSP, MCP, and editor extension: each finding carries the Marrow
diagnostic envelope plus typed incomplete-record and dangling-reference payloads
where the checker exposes them.
Its surface DTOs render `marrow-run` surface records, pages, values,
identities, and commit-bound typed cursors with accepted catalog IDs, store
commit IDs, typed keys, base64 bytes, and lossless strings for integers,
temporal nanoseconds, and decimals. Cursor/page rendering is context-aware:
enum and identity-typed index arguments render as branded surface arguments
instead of raw saved key bytes or plain member strings. Cursor decode preserves
the producing commit boundary so runtime page execution can reject stale
continuations after intervening writes.
`surface/cursor_token.rs` owns the `surface.cursor_token.v1` profile over those
typed cursor DTOs: key id and key-source validation, canonical unpadded
base64url key/token parts, XChaCha20Poly1305 sealing/opening, bounded plaintext
and token sizes, and the error distinction between malformed tokens
(`surface.cursor`) and successfully decrypted stale typed cursor lineage
(`surface.stale_cursor` downstream).

`SurfaceAbiJson` renders the successful `marrow check --format json|jsonl`
`surface_abi` object from checker-owned facts. It includes display-only module
and surface labels, typed catalog status, stable read descriptors, stable
computed-read descriptors, optional exact-body create descriptors for stable
surfaces with a non-empty create set, optional sparse update descriptors for
stable surfaces with a non-empty update set, optional full-subtree delete
descriptors for stable surfaces with a `delete` declaration, and action
descriptors that reuse `entry.invoke.v1` identity, parameter shapes, and result
shape, but only when their operation tags are callable through runtime tag
admission. `SurfaceOperationCatalog` derives the operation tag, request kind,
profile-aware path, surface labels, and alias from that already-curated
descriptor export. `SurfaceRouteManifestJson` renders companion
`surface_routes` objects from the same ABI, and `SurfaceRouteBindings` validates
a manifest against the matching catalog before serving consumes it. Route rows
are JSON `POST` paths under
`/surface/v1/{read|create|update|delete|action}/` with the admitted operation
tag in the path, the operation alias as a render label, and the request-body
kind expected by `surface.operation.v1`. Ranged index page reads, like computed
reads, route under the read prefix. Duplicate stable operation tags are omitted from all read,
computed-read, create, update, delete, or action descriptors that share the tag,
and therefore from the route manifest.
Source-only surfaces serialize blocker strings and no operation descriptors or
route rows. Duplicate-tag checker diagnostics remain future work. Route binding
rejects malformed manifests: wrong profile, non-`POST` method, path/tag/kind
mismatches, forged labels, duplicate paths, or duplicate route operation tags.
Update fields expose `backing_required` only as backing-field metadata; sparse
update request bodies remain non-empty patches and no field is mandatory on
every request.

Inbound surface request parameters and generated write bodies are checked against
the admitted runtime surface shape. `SurfacePointRequestJson`,
`SurfacePageRequestJson`, `SurfaceUniqueLookupRequestJson`,
`SurfaceArgumentJson`, and `SurfaceCursorJson` decode read identities, exact
index arguments, range bounds for ranged index pages, unique lookup keys,
limits, and commit-bound typed cursor boundaries into runtime `SavedKey` and
cursor values. Range page requests require a non-empty scalar `range` object;
ordinary page requests reject that field, and range cursors bind the exact keys,
the normalized range, the last full index tuple, and the last identity.
`SurfacePointCreateRequestJson`,
`SurfaceSingletonCreateRequestJson`, and `SurfaceCreateFieldJson` decode create
identities, field catalog IDs, and canonical scalar/enum/identity values into
runtime `SurfaceCreateInput` values.
`SurfacePointUpdateRequestJson`,
`SurfaceSingletonUpdateRequestJson`, `SurfaceUpdateFieldJson`, and
`SurfaceWriteValueJson` decode update identities, field catalog IDs, and
canonical scalar/enum/identity values into runtime `SurfaceUpdateInput` values.
`SurfacePointDeleteRequestJson` decodes delete identities into runtime
`SurfaceDeleteInput` values; singleton delete has no body fields beyond the
operation envelope.
`SurfaceActionRequestJson` accepts `entry.invoke.v1` argument JSON values and
delegates decoding to `marrow-run`; `SurfaceActionResultJson` carries captured
program output and an optional surface action value DTO. That action value DTO
is deliberately separate from `marrow run --format json`: enum and identity
results carry accepted catalog IDs, not checker-local runtime IDs or source root
labels. The active action JSON shape covers scalars, enums, identities, and
scalar/enum sequences; resource trees, local trees, errors, unknown values, and
unsupported sequence elements are rejected by the checker before a surface action
descriptor is exported.
`SurfaceComputedReadRequestJson` accepts the same `entry.invoke.v1` argument
JSON values and delegates decoding to `marrow-run`;
`SurfaceComputedReadResultJson` carries captured program output and an optional
computed-read value DTO. Resource computed-read values carry accepted resource
and member catalog IDs from the descriptor rather than source labels or
checker-local IDs.
`SurfaceOperationRequestJson`, `SurfaceOperationResponseJson`,
`SurfaceOperationResultJson`, and `SurfaceOperationErrorJson` wrap those same
typed request and result bodies in the single `surface.operation.v1` profile
that the generated TypeScript clients target. Ranged index page reads share that
profile and reuse the page request/result envelope shape.
JSON decode is structural and canonical only: runtime `SurfaceUpdate` owns
declared update-set authorization, duplicate and non-empty patch validation,
exact value shape checks, enum membership and selectability, identity store,
arity, and key-scalar validation, record presence, and post-patch footprint
validation. The linked-Rust `entry.invoke.v1` descriptor and `EntryInvocation`
path is owned by `marrow-check` and `marrow-run`; this crate only embeds that
argument JSON shape for surface action and computed-read request bodies. It owns
the `surface.cursor_token.v1` codec over typed cursor DTOs, while HTTP listeners
and remote serving process policy remain outside this crate.
The route manifest is a descriptor over the operation envelope, not a listener
or router implementation. `surface/client_model.rs` lowers
the ABI plus route bindings into a typed `SurfaceClientModel` (stores as branded
ids, enums as member-id tables, surface records, per-operation methods, and
client-only page-iteration helpers for paged reads);
`surface/client_ts.rs` renders that model into the typed TypeScript surface
client and explicit cursor-token client profile, with the runtime decode/encode
helpers in `surface/client_ts_preamble.ts`.
The client decodes response values against the descriptor as a convenience
projection that fails loud on a malformed field — it is not a second validation
authority. It validates route/ABI agreement before rendering. It also owns the
client freshness key: `surface_abi_digest` is a
`sha256:` digest over the canonically serialized ABI and route manifest (stable
across checkouts of the same surface shape), while `surface_client_digest`
combines that identity with the TypeScript client generator profile. The
cursor-token TypeScript profile uses the same surface digest and a distinct
client digest.
`surface_client_header` / `surface_client_header_digest` write and parse the
do-not-edit, profile, surface-digest, and client-digest header prepended to every
generated client.

Cursor-token codec decision: `marrow-json` depends on `chacha20poly1305 0.11.0`
for XChaCha20Poly1305 and OS CSPRNG-backed nonce generation, and `base64ct 1.8.3`
for canonical `Base64UrlUnpadded` encoding/decoding. Both crates are
Apache-2.0 OR MIT, satisfy the workspace Rust version, and keep token sealing
inside the JSON/profile crate rather than adding cryptographic code to the CLI
or changing runtime cursor semantics.

The operation-tag execution functions compose those DTOs with `marrow-run`
admission. Reads admit stable read tags, decode the point/page/unique request
DTO against the admitted handle, execute singleton, point, page, or unique
lookup reads, and return `SurfaceRecordJson`, `SurfacePageJson`, or
`Option<SurfaceRecordJson>`. The same read DTOs can execute against
`ProjectSurfaceReadSession` without exposing its private store handle. Computed
reads admit stable computed-read tags, decode entry arguments through
`entry.invoke.v1`, execute the resolved public read-only function through
`ProjectSurfaceReadSession` or `ProjectSurfaceSession`, and return captured
output plus an optional computed-read value. Runtime computed-read failures are
sanitized as `surface.computed`; argument decode failures are `surface.request`.
Updates admit stable update tags, decode point or singleton sparse update DTOs,
and return the runtime `surface.*` error type directly over either
caller-supplied checked program/store references or `ProjectSurfaceSession`.
Creates admit stable create tags, decode point or singleton exact-body DTOs,
execute managed record creation, and return the created public projection as
`SurfaceRecordJson`. Deletes admit stable delete tags, decode point or singleton
delete DTOs, and return a deleted result with no record body.
Actions admit stable action tags, decode entry arguments through
`entry.invoke.v1`, execute the resolved public function through
`ProjectSurfaceSession`, and return captured output plus an optional surface
action value. Runtime action failures are sanitized as `surface.action`;
argument decode failures are `surface.request`.

The operation envelope functions compose those same typed bodies into
profile-specific project-session dispatch. They derive the active operation kind
from the current checked program and the requested operation profile, validate
the request body kind against the operation tag, and only then admit the matching
runtime handle. `execute_project_surface_operation_read_only` accepts read and
computed-read bodies through `ProjectSurfaceReadSession` and rejects create,
update, delete, or action bodies as an ABI mismatch.
`execute_project_surface_operation` accepts read, computed-read, create,
sparse-update, delete, and action bodies through `ProjectSurfaceSession`, using
a zero-capability `Host::new()` for actions. Callers that need clock, context,
log, filesystem, or maintenance capabilities for actions use
`execute_project_surface_operation_with_host`. Computed reads are always invoked
with a zero-capability host, and host-effecting computed reads are rejected by
the checker before export. Both helpers return a standard response envelope with
record, page, optional-record, created, updated, deleted, action, or
computed-read results. Error envelopes contain only a stable code and public
message. The project helpers use the session's private store handle and do not
add HTTP serving.
Wrong profile versions fail before tag admission; unknown tags and duplicate
tags fail through operation-kind preflight over checked facts; wrong
read/computed-read/create/update/delete/action shape requests are rejected by
that same preflight as `surface.request`; cursor mismatches stay on the existing
cursor error path.

`serve.rs` renders the already admitted `SurfaceServeBoundary` fact from
`marrow-run` into JSON: the read-only or write serve mode, the admitted
data-view boundary for source/store/watch identity, and the current
process-control status. A `not_exposed` process-control status is intentionally
not a listener, served-process identity, debugger attach target, or control
channel.

## Read next

- `crates/marrow-json/src/run.rs` — bounded run result, run diagnostic, and
  run-fact DTOs.
- `crates/marrow-json/src/lib.rs` — `saved_key_to_json`,
  `data_generation_stamp_to_json`, `DataGenerationJson`, and `DataCommitJson`.
- `crates/marrow-json/src/saved_data.rs` — saved-data request/result DTOs and
  `data_view_boundary` rendering for admitted read-only project data views.
- `crates/marrow-json/src/serve.rs` — admitted serve-boundary rendering for
  read-only and write surface sessions.
- `crates/marrow-json/src/surface.rs` and `crates/marrow-json/src/surface/` —
  surface ABI descriptor DTOs, operation catalog and route binding validation,
  surface read and computed-read result DTOs, checked surface read/computed-read
  request and generated write request DTOs, action DTOs, operation envelope
  DTOs, descriptor alias rendering, route manifest rendering, typed TypeScript
  client model and rendering, and in-process operation-tag execution helpers.
- `crates/marrow/src/cmd_run.rs` — run flag parsing, output capture, store-stamp
  capture, and serialization of the typed run DTOs.
- `crates/marrow/src/trace.rs` and `crates/marrow/src/cmd_data/integrity.rs` —
  saved-key tooling consumers.
- `crates/marrow/src/cmd_data.rs` and `crates/marrow/src/cmd_data/get.rs` —
  data inspection envelopes that carry shared `store_snapshot` rendering.
