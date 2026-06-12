# Tooling Surfaces

Marrow tools render compiler, catalog, runtime, and store facts. They do not
define another database model, and a debug surface does not become a production
API by being documented.

A shared tooling facts layer owns typed data-query resolution, path rendering,
bounded previews, integrity findings, catalog/snapshot metadata, and cursor
validation. CLI commands and backup/restore are renderers over those facts.

## Surfaces

| Surface | Support level | Boundary |
|---|---|---|
| `marrow data roots` / `stats` | Operator/admin inspection. | Exact scans under one stable snapshot are allowed for admin commands; not a production preview API. |
| `marrow data dump` | Operator/admin inspection. | Walks a full stable snapshot by explicit operator request and may expose canonical payload bytes; not a backup/restore format, sync format, production preview, or production data API. |
| `marrow data get` | Operator/admin point inspection. | Presence states are typed facts; raw payload bytes are diagnostic output. |
| `marrow data integrity` | Read-only data-integrity tooling. | Reports incomplete records/keyed entries and orphaned managed cells with typed findings; does not bless invalid managed data. |
| `run --trace` / `test --trace` | Debug execution rendering. | Observes runtime statement/write facts over checked source spans; does not change run semantics; not a stable external API. |
| `run --dry-run` | Checked write preview for one operator-run entry. | Previews that run's managed writes against an isolated run store; use `evolve preview` for evolution. |
| `--maintenance` | Explicit operator capability. | For modeled repair/evolution code, not raw store mutation; cannot be injected by project config or a default entry. |
| `marrow backup` / `restore` | Production typed backup/restore. | Carries source digest, accepted catalog epoch, engine descriptor, and typed cells; rebuilds generated indexes, rejects orphaned managed cells before commit, and rolls back on `restore.data_invalid`. |

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
  checked facts, not promoted from raw bytes, raw paths, or an ad hoc query
  language.
