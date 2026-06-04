# Lane 10: Tooling, Backup, Restore, And Protocols

Goal: make CLI, LSP, data tools, serve, backup, restore, and future adapters
consume shared compiler/runtime facts, with typed backup/restore as the
production data-movement contract and raw/path-shaped inspection held to
debug/admin scope.

Status: integrated foundation, with follow-up hardening. Main contains the
typed backup/restore artifact, checked read-only data inspection, `debug_data_*`
loopback serve operations, and the LSP/tooling boundary. This lane file no
longer authorizes audit-only execution or production raw protocol semantics.

## Current Contract

- Backup and restore are typed Marrow artifacts bound to source digest, accepted
  catalog epoch, engine profile, layout/value-codec facts, checksums, and
  tree-cell data. Restore rebuilds generated indexes and rejects data that the
  current source/catalog cannot account for.
- `marrow data` reads through checked source, accepted catalog metadata, and
  typed tree-cell APIs. It is read-only diagnostic/admin inspection, not a
  backup format and not a production API over raw paths or backend bytes.
- `marrow serve` v0.1 is loopback debug/admin inspection only. Its operations
  stay named `debug_data_*`, are read-only, snapshot-bound, and excluded from
  production app-server, sync, generated API, or backup semantics.
- `marrow debug explain` renders diagnostic/admin checked facts. It must stay
  scoped to source/runtime/store facts and not imply hidden execution-strategy
  output.
- CLI, serve, LSP, backup, restore, and future adapters must render shared facts
  rather than rediscovering resource paths, types, presence, identity, or
  integrity classifications locally.

## Remaining Follow-Up Work

These are not Lane 10 blockers anymore; they are follow-up implementation work
tracked by the central execution plan:

- future activation job status, chunk progress, verification findings, publish
  readiness, compatibility-window admission, adapter names, and close
  conditions;
- future local HTTP/IPC APIs generated or checked from resources, public
  functions, effects, and catalog facts.

## Rejection Ledger

Rejected as production semantics:

- raw saved paths or physical keys as stable public protocol identity;
- raw archive replay as backup/restore;
- production restore that preserves orphaned managed cells under the current
  source/catalog;
- `marrow serve` as a public app server, sync server, remote database, or
  generated API stand-in;
- execution-strategy language under `marrow debug explain`;
- activation job state or receipts as a migration ledger or second schema model;
- tool-local semantic classifiers that disagree with compiler/runtime/store
  facts.

Every remaining mention of these surfaces in docs, tests, or CLI output must be
one of: rejected, debug/admin-only, future-only with a checked-fact boundary, or
an explicit product decision still open.
