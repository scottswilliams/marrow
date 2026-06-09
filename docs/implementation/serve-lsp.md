# Serve and LSP

`marrow lsp` and `marrow serve` are two unrelated long-lived process surfaces dispatched from the CLI binary. They share nothing but their role: pure transport adapters that translate wire JSON to and from canonical analysis and store APIs. Neither owns semantics. The LSP never writes; serve never writes managed data.

| Surface | Transport | Direction | Backed by |
| --- | --- | --- | --- |
| `marrow lsp` | JSON-RPC 2.0, Content-Length framing over stdio | editor diagnostics out | `marrow_check::analyze_project` (project mode) or `parse_source` (per-buffer fallback) |
| `marrow serve` | newline-delimited JSON over one loopback TCP connection at a time | read-only `debug_data_*` queries | `marrow_check::tooling` facts over a pinned `TreeStore` snapshot |

They are deliberately separate: different framing, different message shape, and the map's invariants say they must not be merged.

## LSP

`lsp::run` parses flags and runs the message loop (`serve` in `lsp.rs`), mapping clean/dirty stop to an `ExitCode`. State is `ProjectContext`, captured at `initialize`: if `rootUri` resolves to a valid project, its presence flips diagnostics from per-buffer parse to full project checking. Document sync is full-text `didOpen`/`didChange`/`didClose`; open buffers overlay disk via `ProjectSources`.

The one decision point is `diagnostics_notification` vs `checked_diagnostics_notification`: with project checking active, all files are checked so cross-file facts exist, but only the opened/changed document's diagnostics are published. Positions count UTF-16 code units; byte offsets are clamped to text length.

## serve

`serve::run` parses `--port`/dir, loads a checked project (`crate::load_checked_project`) and a read-only store (`crate::open_store_for_inspection`), binds `127.0.0.1`, prints its address, and runs the accept loop (`serve` in `serve/mod.rs`). Loopback is the only dependency-free cross-platform socket; exposing beyond it would need auth and TLS the protocol lacks.

Each connection pins exactly one store read snapshot for its whole life, so every request line observes one coherent store version and one fixed catalog epoch. `ProtocolSession::handle_request` is the request→reply boundary and never returns `Err`: protocol and store failures both become structured `error` replies carrying `protocol.*` or pass-through `store.*` codes, so clients branch on ok-vs-error, never on prose. The `Op` enum is the single source of which ops exist and which read data; its `reads_data` gate, not an op-name re-match, drives the stale-epoch refusal. Paging cursors are session-bound: signed with a per-connection key over scope plus payload, unforgeable, not durable across connections, and bound to their issuing path scope.

### Modules

| File | Responsibility |
| --- | --- |
| `crates/marrow/src/lsp.rs` | Whole LSP server: framing, lifecycle, full-text sync, checker-or-parse diagnostics, UTF-16 position mapping, `file://` decode |
| `crates/marrow/src/serve/mod.rs` | serve transport: arg parse, project/store load, TCP accept loop, snapshot pinning, bounded line reader with oversized-drain and timeout handling |
| `crates/marrow/src/serve/protocol.rs` | Protocol root: `ProtocolSession`, `Op`, stale-epoch gate, dispatch, reply envelope, `ProtocolError` and `protocol.*` codes |
| `crates/marrow/src/serve/protocol/codec.rs` | Wire codec: path segments and `SavedKey` objects, base64 values/bytes, shared saturating `limit` parser |
| `crates/marrow/src/serve/protocol/cursor.rs` | Signed opaque cursors: per-connection key, one shared envelope validator for walk and children flavors |
| `crates/marrow/src/serve/protocol/data.rs` | Handlers for `debug_data_roots`, `debug_data_get`, `debug_data_children` |
| `crates/marrow/src/serve/protocol/walk.rs` | Handler for `debug_data_walk`; defines `MAX_WALK = MAX_PREVIEW_ITEMS` |

Tests are inline: `lsp.rs` covers UTF-16 counting and header-size rejection; `serve/mod.rs` covers snapshot isolation, stale-epoch refusal, and line framing; `serve/protocol/tests.rs` drives `ProtocolSession::handle_request` for dispatch, paging, cursor binding, codec round-trips, and `store.*` passthrough.

### Code reality vs `docs/serve-protocol.md`

- `debug_data_children` reply order is whatever `marrow_check::tooling::data_children` yields; `data.rs` does no sorting. The doc's "keys before named members" ordering is owned by the tooling layer, not enforced here.
- The doc says declared-member listings take no limit/cursor and reject a non-positive children limit; the code's `!segments.is_empty()` term short-circuits that guard, so a limit or cursor on a root-path `debug_data_children` is silently ignored (the listing is unpaged: limit forced to `MAX_WALK`, no cursor) rather than rejected.

## Read next

- `crates/marrow/src/serve/protocol.rs` — `ProtocolSession::dispatch`, `Op` — op parsing, stale-epoch gate, the only dispatch fan-out.
- `crates/marrow/src/serve/mod.rs` — `serve_connection_within`, `read_line_bounded_within` — snapshot pinning and the line-framing/oversized-drain/timeout invariants.
- `crates/marrow/src/serve/protocol/cursor.rs` — `CursorState::decode_signed_envelope` — the forge/replay/scope cursor contract.
- `crates/marrow/src/lsp.rs` — `diagnostics_notification`, `checked_diagnostics_notification` — the project-checked vs parse-fallback decision and buffer overlay.
