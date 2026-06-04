# Tooling Feature Surfaces

Marrow tools render compiler, catalog, runtime, and store facts. They do not
define another database model, and a debug surface does not become a production
API by being documented.

The shared tooling facts layer owns typed data-query resolution, path rendering,
bounded previews, integrity findings, explain facts, catalog/snapshot metadata,
and cursor validation. CLI commands, `marrow serve`, LSP adapters, backup/restore,
and future local APIs are renderers over those facts.

## Feature Surface Matrix

| Surface | v0.1 verdict | Fact authority | Boundary |
|---|---|---|---|
| `marrow debug explain` | Keep as diagnostic/admin explanation. | Checked resolver and saved-path classifier through shared explain facts. | Does not expose physical keys, production preview contracts, or runtime-strategy output. |
| `marrow data roots` / `stats` | Keep as operator/admin inspection. | Checked saved roots plus typed tree-cell traversal. | Exact scans are allowed for admin commands; they are not production preview APIs. |
| `marrow data dump` | Keep as operator/admin inspection. | Checked/catalog path rendering plus typed data traversal. | May expose canonical payload bytes; walks a full stable snapshot by explicit operator request; not a backup/restore format, sync format, production preview, or production data API. |
| `marrow data get` | Keep as operator/admin point inspection. | Checked data-query resolution plus typed store read. | Presence states are typed facts; raw payload bytes remain diagnostic/admin output. |
| `marrow data integrity` | Keep as data-integrity tooling. | Checked schema, accepted catalog IDs, typed store traversal, and actual-cell orphan scan. | Reports `data.orphan` with repair guidance. It is read-only and does not bless orphaned managed data. |
| `marrow serve debug_data_*` | Keep as v0.1 loopback debug/admin protocol. | Shared data-query, path, preview, cursor, and metadata facts. | Bounded reads only, per-connection snapshot, stale-epoch refusal, no production app/sync/backup/raw-path contract. |
| `run --trace` | Keep as debug execution rendering. | Runtime statement/write observations using checked source spans and typed write targets. | Observes behavior; it does not change run semantics or expose raw storage paths. |
| `test --trace` | Keep as debug execution rendering for tests. | Same trace facts as `run --trace`, labelled by test. | Diagnostic only; not a stable external protocol. |
| `run --dry-run` | Keep as checked write preview for an operator-run entry. | Runtime write hook over managed writes, rolled back by savepoint. | It previews writes for that run, not source-native evolution; use `evolve preview` for evolution. |
| `--maintenance` | Keep as explicit operator capability. | Runtime capability state checked by managed write/delete operations. | Cannot be injected by project config or default entry. It is for modeled repair/evolution code, not raw store mutation. |
| `marrow backup` / `restore` | Keep as typed backup/restore. | Source digest, accepted catalog epoch, engine descriptor, typed cell stream, generated-index rebuild, full integrity verification. | Restore rejects orphaned managed cells before commit and rolls back on `restore.data_invalid`. |
| `marrow lsp` | Keep as editor protocol over checked facts. | Project checker with open-buffer overlays, parse fallback when no project root applies. | Diagnostic positions use LSP UTF-16 code units. No data or debug surface is exposed through LSP. |
| Future local adapters | Defer until generated from shared checked facts. | Same transport-free tooling facts. | Must negotiate version/capabilities and stay separate from v0.1 debug serve names. |
| Raw saved paths | Not a production API. | Parsed source paths and checked/catalog facts only. | No public raw saved-path encoder/decoder compatibility surface. |
| Raw payload bytes | Debug/admin output only. | Canonical typed value bytes from the store. | May appear in machine-readable inspection output as base64; not a production app payload contract. |
| Cursors | Keep as opaque adapter tokens. | Shared cursor contract over typed query prefixes and last keys/paths. | Bounded to one connection/session and one request prefix; forged or replayed cursors fail closed. |
| Unbounded scans | Allowed only for explicit operator/admin commands. | Typed store traversal under one stable snapshot. | Production previews and protocol reads must be bounded or paged. |
| Generated API/server/sync language | Defer. | Future checked local API generated from shared facts. | Do not promote `debug_data_*`, raw bytes, raw paths, or ad hoc query language into production semantics. |

## Follow-Up Boundaries

`marrow data dump` and `marrow data get` are diagnostic/admin surfaces. The
implementation must keep them as read-only, checked-fact renderers. If a future
production local API exists, it is generated from shared checked facts instead
of promoting these inspection commands.
