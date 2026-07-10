# Marrow Documentation

The documentation is divided by authority and implementation status.

- [Language Reference](language/) defines current `.mw` behavior.
- [Implementation Map](implementation/) describes the code that exists.
- [Project Status](status.md) separates implemented, legacy, designed, accepted
  target, and research work.
- [Vision](vision.md) describes long-term direction without making it part of
  the current language.
- [Target Design Contracts](design/) contain only explicitly approved,
  unimplemented contracts named by the work plan. The directory is currently
  empty apart from its governance page.

When these sources disagree, the language reference states the intended current
contract and the implementation map states what the repository actually does.
The disagreement is implementation or documentation work; a plan or decision
record does not silently override either source.

## Start Here

- [Quickstart](quickstart.md) — create, check, run, test, and inspect a small
  durable project.
- [Install](install.md) — supported platforms, source installation, and data
  directories.
- [Project Status](status.md) — current capabilities and limitations.
- [Stability Contract](stability.md) — the current pre-release compatibility
  boundary.

## Language And Data

- [Language Reference](language/) — syntax and semantics of `.mw`.
- [Data Modeling](data-modeling.md) — resources, durable roots, keyed child
  layers, identities, indexes, and history.
- [Data Evolution](data-evolution.md) — previewing and applying supported
  changes over populated data.

Guides may explain a task in a more convenient order, but defer to the language
reference for exact behavior.

## Tools And Operations

- [CLI Reference](cli.md) — commands, arguments, output, and exit behavior.
- [Project Configuration](project-config.md) — `marrow.json` fields and
  validation.
- [Data Tools](data-tools.md) — inspection, integrity checking, and recovery.
- [Operations](operations.md) — native-store ownership, deployment, backup, and
  restore.
- [Error Codes](error-codes.md) — dotted codes and machine-readable
  diagnostics.

## Implementation

- [Implementation Map](implementation/) — source pipeline, crate ownership, and
  code-navigation links.
- [Backend Contract](backend-contract.md) — current ordered tree and transaction
  requirements beneath durable paths.
- [Testing Architecture](testing-architecture.md) — test tiers, fixtures, and
  accepted oracles.

Implementation pages are descriptive rather than normative. They name the
actual interpreter, storage implementation, and legacy components while those
components exist.

## Direction

- [Vision](vision.md) — purpose, architectural principles, scope, and the
  embedded-to-served development path.
- [Project Status](status.md) — claim-by-claim implementation status.
- [Target Design Contracts](design/) — the lifecycle for an exact unimplemented
  rule approved for implementation.

Detailed unimplemented syntax and protocols do not belong in the active
reference. A language rule becomes current only when the canonical reference
and implementation change together.

## Documentation Conventions

- Examples use current syntax unless explicitly marked illustrative.
- Current behavior and architectural direction appear separately.
- Use the complete [status vocabulary](status.md#status-categories): Current,
  Legacy, Designed, Accepted target, and Research.
- Claims state whether they are implemented, enforced, tested, measured, or
  designed.
- One page owns each concept and one term names it.
- Public descriptions use technical language rather than superlatives.
- Limitations and counterexamples are part of the reference.
- Obsolete material is removed instead of retained as a second design.
- Decision records preserve rationale; the canonical reference owns current
  behavior.
