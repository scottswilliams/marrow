# Marrow Reference

These pages describe the Marrow language, runtime, tools, and implementation
architecture.

## Start Here

- [Quickstart](quickstart.md) — create a project, write a resource, run it,
  inspect the saved data, and run a test in five minutes.
- [Install](install.md) — source builds, the release package shape, and data
  directories.

## Guides

Task-oriented pages that show how to put the language and tools together. They
point at the [language reference](language/) for the exact rules.

- [Data Modeling](data-modeling.md) — roots, child layers, identity keys,
  sparse and required fields, relationships, history, indexes, and lookup
  patterns.
- [Data Evolution And Maintenance](data-evolution.md) — evolving saved data with
  explicit backfills, stable IDs, maintenance mode, repair, backup, and restore.

## Language Reference

- [Language](language/) defines `.mw` syntax, types, resources, saved data,
  control flow, builtins, standard library contracts, the reference sample,
  and grammar. This is the language law the guides and tooling pages defer to.

## Tooling Reference

- [CLI Reference](cli.md) — every `marrow` subcommand: syntax, inputs, outputs,
  exit behavior, and examples.
- [Project Configuration](project-config.md) — every `marrow.json` field and
  its validation rules.
- [Data Inspection And Repair Tools](data-tools.md) — the read-only `marrow
  data` subcommands in depth.
- [Serve Protocol](serve-protocol.md) — the newline-delimited JSON protocol the
  `marrow serve` data server speaks.
- [Language Server](lsp.md) — the `marrow lsp` editor language server and its
  planned path.
- [Errors](error-codes.md) — CLI exit codes, the machine-readable error
  envelope, and the stable dotted error codes.

## Architecture Reference

- [Implementation And Backends](implementation.md) defines the language/database
  kernel, project configuration, logical paths, managed write path, backend
  contract, server/tooling model, and capability profiles.
- [Backend Contract](backend-contract.md) — the ordered path/value operations,
  savepoints, presence states, bounded scans, the conformance suite, and
  native-store responsibilities a store backend must satisfy.
- [Future](future/) — designed, normative surfaces that are not yet
  implemented, mirroring the pages above.
- [Roadmap](roadmap/) — a status note for the implemented kernel and the
  non-goals that bound it.

Marrow is unreleased. If implementation and language references disagree,
treat the disagreement as implementation work, not as a competing design.
