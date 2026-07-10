# Tooling implementation

Tooling spans `marrow-project`, `marrow`, `marrow-json`, `marrow-codes`, and the
compiler analysis API.

## Project discovery

`marrow-project/src/lib.rs` parses `marrow.json`, validates paths and fields,
discovers projects, and computes project inputs. Compiler project I/O then
loads source, tests, and accepted catalog inputs.

## CLI

`crates/marrow/src/main.rs` owns command selection and help. Commands are split
into `cmd_*` modules for check, format, run, test, data, doctor, evolution,
backup, restore, and legacy client/server behavior. Command code should compose
typed compiler/runtime/store services and restrict itself to argument parsing,
process lifecycle, and rendering.

## Structured data

`marrow-json` owns shared DTOs for diagnostics, run/test results, data tooling,
store snapshots, and current legacy surface traffic. It must serialize
compiler-owned facts rather than create another semantic model.

`marrow-codes` owns dotted diagnostic identities, severity, kind, catchability,
lifecycle, and concise meaning. It generates `docs/error-codes.md`; the drift
test requires committed output to equal the registry exactly.

## Editor boundary

The downstream `marrow-lsp` repository consumes snapshot-aware analysis facts.
Language semantics and source classification are fixed in Marrow first; the LSP
uses those facts rather than reclassifying syntax or diagnostics.
