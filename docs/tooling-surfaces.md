# Tooling Surfaces

Marrow tools render compiler, catalog, runtime, and store facts. They do not
define another database model, and a debug surface does not become a production
API by being documented.

A shared tooling facts layer owns typed data-path resolution, path rendering,
bounded previews, integrity findings, catalog/snapshot metadata, and cursor
validation. CLI commands and backup/restore are renderers over those facts.

## Surfaces

| Surface | Support level | Boundary |
|---|---|---|
| `marrow data roots` / `stats` | Operator/admin inspection. | Exact scans under one stable snapshot are allowed for admin commands; not a production preview API. |
| `marrow data dump` | Operator/admin inspection. | Walks a full stable snapshot by explicit operator request and may expose canonical payload bytes; not a backup/restore format, sync format, production preview, or production data API. |
| `marrow data get` | Operator/admin point inspection. | Presence states are typed facts; raw payload bytes are diagnostic output. |
| `marrow data integrity` | Read-only data-integrity tooling. | Reports decode, key-type, dangling-reference, incomplete, and orphan findings with typed fields; does not bless invalid managed data. |
| `run --trace` / `test --trace` | Debug execution rendering. | Observes runtime statement/write facts over checked source spans; does not change run semantics; not a stable external API. |
| `run --dry-run` | Checked write preview for one operator-run entry. | Previews that run's managed writes against an isolated run store; use `evolve preview` for evolution. |
| `--maintenance` | Explicit operator capability. | For modeled repair/evolution code, not raw store mutation; cannot be injected by project config or a default entry. |
| `marrow backup` / `restore` | Production typed backup/restore. | Carries source digest, accepted catalog epoch, engine descriptor, and typed cells; restore writes into an empty store unless `--replace --count N` matches the live record count, rebuilds generated indexes, rejects orphaned managed cells before commit, and rolls back on `restore.data_invalid`. |
| Checked entry descriptors / `marrow-run::EntryInvocation` | Unstable linked-Rust entry invocation. | `marrow-check` renders `entry.invoke.v1` descriptors from checked public entry signatures, parameter shapes, return shape, accepted catalog identities, and return presence; `marrow-run` admits typed invocation values against the current descriptor identity and executes them. It is not a JSON request profile, HTTP route, generated-client contract, or stable public Rust API. |
| Checked `SurfaceFact` / operation facts / operation descriptors | Unstable checked application-surface facts. | Resolves declared surfaces to checker-valid store, member, index, read kind, footprint, projection identities, read-operation aliases, sparse update fields, public action functions, and accepted-catalog operation descriptors/tags for stable surfaces. These facts are transport-neutral compiler facts; they are not HTTP routes, TypeScript names, raw saved paths, or an admin data-access API. Their typed catalog status reports whether stable descriptor export is blocked by a pending catalog proposal or missing accepted catalog IDs, including durable IDs needed by action parameter and return types. |
| `marrow-run::ProjectSurfaceReadSession` | Unstable linked-Rust surface read session. | Checks a project, opens the configured native store read-only, requires an already accepted and stamped durable store, fences drift without auto-apply, and exposes admitted surface read operations by stable operation tag. It is not HTTP routing, process lifetime, generated clients, update/create/delete, opaque cursor tokens, UID minting, baseline freeze, maintenance, restore, recovery, or a stable public Rust API. |
| `marrow-run::ProjectSurfaceSession` | Unstable linked-Rust surface read/write session. | Checks a project, opens an existing configured native store writable, requires accepted catalog identity plus store UID and commit metadata, fences drift without hidden repair, and exposes admitted surface reads, sparse updates, and actions by stable operation tag without exposing the store handle. It is a single-owner, sequential session; while it is open, the native writer lock makes it the owning process/session and excludes another writer or read-only inspection. It is not HTTP routing, process lifetime, generated clients, generated create/delete semantics, opaque cursor tokens, UID minting, baseline freeze, store creation, auto-apply, maintenance, restore, recovery, or a stable public Rust API. |
| `marrow-json::surface` DTOs | Checked application read/update/action JSON. | Renders serialized surface ABI descriptors and the descriptor-derived `surface.route.v1` manifest for `marrow check --format json|jsonl`, including checked read/action aliases as labels; decodes checked read request parameters through admitted runtime reads; executes read DTOs over `ProjectSurfaceReadSession`; decodes sparse update request DTOs through admitted runtime updates over caller-supplied stores or `ProjectSurfaceSession`; decodes action arguments through `entry.invoke.v1` and executes actions over `ProjectSurfaceSession`; dispatches `surface.operation.v1` request envelopes by stable operation tag through `ProjectSurfaceReadSession` or `ProjectSurfaceSession`; and serializes already-executed surface records, pages, typed values, catalog identities, commit-bound context-aware typed cursor boundaries, accepted-catalog action values, updated/action results, and sanitized code/message error envelopes. The route manifest names JSON `POST` paths over operation tags; it is not a listener or process-lifetime policy. The default project operation helper gives actions a zero-capability host; callers that need host capabilities use the explicit-host helper. It is not HTTP serving, opaque cursor tokens, generated clients, create/delete body decode, or a raw saved-data access API. |

## Boundaries

These hold across every surface:

- Raw saved paths are not a production API. There is no public raw saved-path
  encoder/decoder compatibility surface; tools work from parsed source paths
  and checked/catalog facts.
- Raw payload bytes are debug/admin output only. They may appear in
  machine-readable inspection output as base64; they are not a production
  payload contract.
- Unbounded scans are allowed only for explicit operator/admin commands;
  production previews must be bounded or paged.
- No production local API exists. One would be generated from the same shared
  checked facts, not promoted from raw bytes, raw paths, or an ad hoc
  saved-data access language.
