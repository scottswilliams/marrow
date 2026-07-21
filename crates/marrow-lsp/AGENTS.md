# marrow-lsp contributor notes

`marrow-lsp` is the in-tree language server dispatched as `marrow lsp`. It is
downstream of the compiler: it consumes the published editor-analysis fact floor
(`marrow_compile::AnalysisSnapshot` — diagnostics, checked formatting, hover,
definition) and the shared physical project adapter (`marrow-project-fs`), and it
reconstructs nothing. Types, paths, facts, diagnostics, and formatting come only
from those owners. Missing semantic facts are added to the compiler first; the LSP
must not reconstruct types, paths, authority, evolution, or runtime meaning, and it
opens no store.

## Boundaries

- **Transport is Marrow-owned.** A private closed JSON-RPC 2.0 envelope
  ([`protocol`]) and a bounded standard-library stdio transport ([`transport`]). No
  `lsp-server`, async runtime, channel crate, `serde_json::Value`, `json!`, or public
  generic `Serialize` surface. `lsp-types` supplies the standard payload structs
  only; outbound frames serialize through the one concrete seam in [`outbound`].
- **Project facts through the facade.** The pure project owner is named through
  `marrow-project-fs` re-exports (`FileIdentity`, `ProjectInput`); there is no direct
  `marrow-project` edge. Capture failures are rendered only through the allowlisted
  `CapturePresentation::{code, write_operational_message}` into a bounded sink — never
  reclassified, never rendered through another writer, never located.
- **Bounded and affine.** Every retained resource is charged against [`capacities`]
  before admission (the `M_owned <= H_owned` inequality is proven at compile time).
  Concurrency is bounded by move-only [`credit`] tokens minted in fixed counts.
- **The DAG gate** (`marrow-codes/tests/tidy.rs`) forbids any compiler/syntax/project
  owner from depending on this crate, and forbids this crate from reaching the
  kernel, store, VM, image, verifier, or wire owners.

## Coverage

The server implements the primary journeys end to end over real stdio
(`marrow/tests/lsp_stdio.rs`): initialize/initialized, full-document open/change/close
sync, whole-project recomputation, per-file diagnostic publication with empty lists
and tombstones, and hover/definition/formatting.

The coordinator is a pure event machine, so its concurrency law matrix is enforced by
deterministic in-crate tests (no timing dependence, no test-only production entry
point): receipt-gated initialize-response delivery gating the first analysis; the
shared live-entry budget with `IngressOverload` fail-stop at N+1, duplicate-live
classification consuming no entry, and anonymous-slot exhaustion; terminal arbitration
classifying a handed-off-but-unreceipted request `DeliveryUnknown` and a held query
`AbandonedByTerminal`; the capture-episode latch (`Eligible`/`Latched`) notifying once
and resetting only after the observing publication set fully delivers; publication
exclusivity holding the single plan credit across receipts so a newer result waits and
derives its tombstones from the final ledger; and `-32801 ContentModified`
reauthorization for a query held across an edit. The foundation modules — capacities,
the JSON-RPC envelope decoder, framing, the URI and document-identity owner, UTF-16
position mapping, the document ledger, the lifecycle FSM, the outbound seam, and the
capacity credits — each carry their own red suite.

## Absences (standing)

No completion, signature help, document symbols, references, rename, workspace
symbols, on-type formatting, `language-configuration.json`, data browser, telemetry,
network client, or updater. Those are future editor capabilities that depend on
compiler facts not yet published.
