# Legacy implementation map

These modules remain reachable at the current revision but are excluded from
the target architecture. They should be deleted as complete semantic families,
not preserved behind compatibility wrappers.

## Surface and generated operations

- Syntax: `marrow-syntax` surface declarations.
- Schema/checking: `marrow-check/src/surface.rs`, `surface_abi.rs`, and related
  analysis facts.
- Runtime: `marrow-run/src/surface.rs` and surface project sessions in
  `project_session.rs`.
- JSON: `marrow-json/src/surface.rs` and `serve.rs`.
- CLI: `cmd_client/`, `cmd_serve/`, `init --client`, and client synchronization.
- Tests: surface checker/runtime/JSON suites and CLI client/server cases.

The family repeats selected store fields and manufactures read/create/update/
delete/action operations, tags, routes, and TypeScript methods. It must not
become an intermediate representation for the replacement callable boundary.

## Cost model

User-facing cost shapes, hidden-scan terminology, and cost-derived surface
facts live across checker facts, rules, analysis views, JSON output, diagnostics,
and tests. Observable resource limits and explicit ordered iteration survive;
query-shaped cost vocabulary does not.

## Catalog and session lifecycle

Current `CatalogId`, catalog epochs, `marrow.lock` runtime authority,
implicit first-run baselining, zero-mutation auto-apply, `ProjectSession`, and
surface sessions form one coupled lifecycle. Useful change-classification and
validation cases should migrate to the replacement semantic-path and lifecycle
owners.
The old identities, fallback paths, and mega-session should not survive merely
to keep unreleased stores or tests compatible.

## Deletion rule

A replacement lane owns a whole family: production symbols, exports,
configuration, CLI/UI paths, documentation, tests, fixtures, and downstream
consumers. Green old tests are not a reason to retain rejected behavior.
