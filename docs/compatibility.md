# Compatibility

Marrow is experimental and unreleased. Compatibility statements on this page
describe the current repository revision; they are not a v0.1 release promise.
A future release must identify both its version and source revision.

## Current Build Boundary

The workspace package version is `0.1.0`, uses Rust 1.89, and currently supports
source builds on Linux and macOS. There is no `v0.1.0` release tag, crates.io
publication, signed binary, or prebuilt distribution.

`marrow --version` prints the package version and an engine-profile tuple. The
tuple identifies storage layout facts used by the current binary; it is not a
claim that arbitrary builds with the same package version are interchangeable.

## Current Tooling Interfaces

The current CLI uses three exit classes:

| Exit | Meaning |
|---:|---|
| `0` | The command completed successfully. |
| `1` | The command reached a project, language, runtime, storage, or tooling failure. |
| `2` | Command-line usage failed before the command body ran. |

Dotted diagnostic codes and structured report fields are the current
machine-readable interfaces. Human-readable message prose may change. The
[Error Code Reference](error-codes.md) is generated from the code registry.
Before a release policy is adopted, even structured interfaces may change with
the implementation and its documentation.

## Projects And Durable Data

`marrow.toml` is the current project manifest: a closed-schema TOML file whose
only key is a required `edition`. Its schema and the path-derived module identity
it anchors are described in [Projects](tools/projects.md). Durable-data project
artifacts return with the refounded durable owners; there is no store on the beta
line yet.

Raw native-store files are private implementation data, not an interchange
format. Move durable data through typed backup and restore. A restore validates
the archive's source, catalog, layout, key, value-codec, and integrity facts
before committing.

The native redb store is the only current persistent storage substrate. The
in-memory implementation supports tests and development. The repository does
not establish portability across multiple persistent substrates; a future
adapter would have to implement and pass the same storage contract before such
a claim could be made.

## Unstable Interfaces

The Rust crates in the workspace are internal implementation APIs. Linked-Rust
entry invocation, runtime sessions, storage types, and JSON DTO modules are not
stable embedding interfaces.

The prototype surface/client/serve stack was deleted at B00 and has no
compatibility commitment. See [Project Status](status.md).
