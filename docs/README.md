# Marrow Reference

These pages describe the Marrow language, runtime, tools, and implementation
architecture.

## Start Here

- [Quickstart](quickstart.md) — create a project, write a resource, run it,
  inspect the saved data, and run a test.
- [Install](install.md) — source builds and data directories.

## Guides

Task-oriented pages that show how to put the language and tools together. They
point at the [language reference](language/) for the exact rules.

- [Data Modeling](data-modeling.md) — roots, child layers, identity keys,
  sparse and required fields, relationships, history, indexes, and lookup
  patterns.
- [Data Evolution And Maintenance](data-evolution.md) — evolving saved data with
  automatically recorded durable identity, exact witnesses, source-native apply,
  maintenance repair, backup, and restore.

## Language Reference

- [Language](language/) defines `.mw` syntax, types, resources, saved data,
  control flow, builtins, standard library contracts, the reference sample,
  and grammar. This is the language law the guides and tooling pages defer to.

## Tooling Reference

- [CLI Reference](cli.md) — every `marrow` subcommand: syntax, inputs, outputs,
  exit behavior, and examples.
- [Project Configuration](project-config.md) — every `marrow.json` field and
  its validation rules.
- [Data Inspection And Repair Tools](data-tools.md) — `marrow data` inspection
  commands and the explicit recover command in depth.
- [Operations](operations.md) — native-store writer model, deploy choreography,
  backup/restore repack, branch stores, and security boundaries.
- [Tooling Surfaces](tooling-surfaces.md) — support levels and boundaries for
  debug, admin, and production tool surfaces.
- [Errors](error-codes.md) — CLI exit codes, the machine-readable error
  envelope, and the stable dotted error codes.

## Architecture Reference

- [Implementation Map](implementation/) — the code-truth architecture map: what
  each crate and module does and where to read it, mirroring the source pipeline
  from syntax through check and runtime to the store.
- [Backend Contract](backend-contract.md) — the ordered path/value operations,
  flat transactions, presence states, bounded scans, the conformance suite, and
  native-store responsibilities a store backend must satisfy.
- [Freeze-Gate Evidence](freeze-gate-evidence.md) — the storage-engine/04
  gate-35 evidence ledger assembled from producing lanes and the final W7.2
  clean-worktree run.
- [Testing Architecture](testing-architecture.md) — the test tiers, allowed
  oracles, and fixture rules the test suite follows.
- [Future](future/) — selected future surfaces whose designed contracts are not
  implemented yet.

Marrow is unreleased. If implementation and language references disagree,
treat the disagreement as implementation work, not as a competing design.

## Scope And Security

Marrow v0.1 is deliberately bounded. It does not add a second storage query
language, a hidden object store, an ORM layer, a SQL-style migration subsystem,
implicit async syntax, a required background service, a web framework, a remote
database product, a built-in users-and-roles system, or backend-specific
application APIs.

There is no database users-and-roles system in `.mw`. The security boundary is
the host process, the filesystem or backend credentials, and the selected
transport: local CLI commands use the current user's access to project source
and data, and remote server transport is out of v0.1. Application authorization
belongs in application data and code, not in a hidden backend permission layer.
