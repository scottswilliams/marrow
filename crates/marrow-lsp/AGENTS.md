# marrow-lsp contributor notes

`marrow-lsp` is a standalone command and library downstream of the compiler.
It consumes `marrow_compile::AnalysisSnapshot` and the physical project adapter
`marrow-project-fs`. Missing semantic facts are added to those owners first;
the LSP must not reconstruct types, paths, authority, evolution, runtime meaning,
diagnostics or formatting. It opens no store. The
[language server reference](../../docs/tools/lsp.md) owns supported behavior.

## Boundaries

- **Transport is Marrow-owned.** A private closed JSON-RPC 2.0 envelope
  (`protocol`) and a bounded standard-library stdio transport (`transport`). No
  `lsp-server`, async runtime, channel crate, `serde_json::Value`, `json!`, or public
  generic `Serialize` surface. `lsp-types` supplies the standard payload structs
  only; outbound frames serialize through the one concrete seam in `outbound`.
- **Project facts through the facade.** The pure project owner is named through
  `marrow-project-fs` re-exports (`FileIdentity`, `ProjectInput`); there is no direct
  `marrow-project` edge. Capture failures are rendered only through the allowlisted
  `CapturePresentation::{code, write_operational_message}` into a bounded sink — never
  reclassified, never rendered through another writer, never located.
- **Bounded.** Charge resources owned by `capacities` before admission; they bound
  what the server itself retains and reserves, not whole-process memory. Outbound
  concurrency is the coordinator's count of frames handed to the writer and not yet
  receipted, capped at `OUTBOUND_CREDITS`.
- **The DAG gate** (`marrow-codes/tests/tidy.rs`) forbids direct dependencies on
  this crate from compiler/syntax/project owners. It also forbids this crate's
  direct dependencies on kernel, store, VM, image, verifier or wire owners.

## Coverage

`tests/lsp_stdio.rs` exercises the stdio boundary of the real binary: framing, the
handshake, the not-initialized refusal, shutdown and termination. Every payload is
pinned in-process. Keep coordinator laws in deterministic in-crate tests, with no
timing dependence or test-only production entry point. Preserve receipt-gated
initialization, bounded live and anonymous request admission, delivery/terminal
classification, capture notification episodes, exclusive publication with
tombstones derived from delivered state, query reauthorization across edits, and
termination on a lost thread. Framing, identity, positions, lifecycle and outbound
serialization retain their boundary tests.

## Scope

Do not add references, rename, workspace symbols or on-type formatting by
reconstructing compiler facts. The server has no data browser, telemetry, network
client or updater. Static syntax highlighting and bracket/comment configuration
belong to the [VS Code host](../../editors/vscode/AGENTS.md), under its thin-host
and generated-artifact rules.
