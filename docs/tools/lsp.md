# Language Server

`marrow lsp` runs an in-tree language server over standard input and output. It
speaks JSON-RPC 2.0 with Language Server Protocol (LSP) message framing and serves
editor features from the compiler's published analysis facts. It is normally
launched by an editor, not run by hand.

The server reconstructs no language semantics. Diagnostics, formatting, hover, and
definition come only from the compiler's editor-analysis fact floor (the revisioned
`AnalysisSnapshot`) and the shared physical project adapter; the server derives no
types, paths, or diagnostics of its own and opens no store.

## Transport

The server reads and writes LSP-framed messages: each message is a
`Content-Length` header, a blank line, and a JSON-RPC 2.0 body. Message bodies and
header blocks are bounded; an oversized or malformed frame is a framing fault. The
server uses a bounded standard-library transport with no third-party language-server
framework, asynchronous runtime, or channel library.

Batch requests (a top-level JSON array) are not supported: the server rejects every
array as one `-32600` error under the current LSP profile. Invalid JSON is a single
`-32700` error.

## Lifecycle

The server follows the standard LSP lifecycle. It answers `initialize`, then
enters normal operation after the `initialized` notification. Before
initialization every other request receives `-32002` (server not initialized).
`shutdown` followed by `exit` terminates with exit code `0`; an `exit` before
`shutdown`, or end of input without `exit`, terminates with a nonzero code.

At initialization the server selects at most one workspace root: a single
`workspaceFolders` entry, or `rootUri` when no folder is given. Two or more folders,
or a malformed root, are rejected with `-32602` and do not complete initialization.

## Capabilities

In normal operation the server advertises:

- **Text document sync** — open/close notifications and full-document change sync.
  A change carries the whole document body; incremental (range) changes are not
  used.
- **Diagnostics** — published per file. Opening or changing a document recomputes
  the whole project and publishes the complete diagnostic list for each file,
  including an empty list for a clean file. A file removed from the project is
  cleared with an empty publication.
- **Formatting** — `textDocument/formatting` returns a single whole-document edit
  with the canonically formatted source, or no result when formatting is refused
  (unparsed source, or a rewrite that would drop a retained comment).
- **Hover** — `textDocument/hover` returns the compiler's canonical type display at
  a resolved local, parameter, or call site.
- **Definition** — `textDocument/definition` returns the source location of a
  resolved function callee. A call to a generic function targets its source
  template.

Positions are exchanged in the LSP UTF-16 encoding; the server maps them to and from
the compiler's UTF-8 source spans.

## Documents and overlays

While a document is open, the server analyzes the project with the open buffer's
text overlaid on the file on disk, so diagnostics and facts reflect unsaved edits.
When the project cannot be captured — for example, a malformed `marrow.toml` — a
semantic request receives a `-32803` response and the failure is surfaced once as an
error message; no diagnostics are fabricated.

## Scope

This is the minimal semantic server. It does not provide completion, signature help,
document symbols, references, rename, workspace symbols, a data browser, or any
durable place, effect, or authority facts; those are future editor capabilities that
depend on compiler facts not yet published. The server owns no telemetry, network
client, or updater.
