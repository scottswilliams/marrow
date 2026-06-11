# Language Server

`marrow lsp` is the Marrow editor language server. It speaks JSON-RPC 2.0 over
stdio with `Content-Length` framing, so any LSP-capable editor can run it. Point
your editor's Marrow integration at the `marrow` binary with the `lsp`
subcommand and no other arguments.

```sh
marrow lsp
```

The server reads requests from stdin and writes responses and notifications to
stdout. It is interactive only through an editor; running it in a terminal will
block waiting for framed input.

This is the editor language server, distinct from [`marrow serve`](serve-protocol.md),
which is a read-only data/inspection server over loopback TCP with different
framing and purpose.

## What It Does Today

The current server provides:

- Lifecycle. Handles `initialize`, `shutdown`, and `exit`. `initialized`
  and other notifications are accepted and ignored.
- Document sync. Tracks open documents with full text sync
  (`textDocumentSync: 1`). Each `textDocument/didChange` carries the whole new
  document; the server uses the last content change as the buffer's new text.
- Diagnostics. On every `textDocument/didOpen` and `textDocument/didChange`,
  it publishes `textDocument/publishDiagnostics`. If the editor initializes the
  server with a `rootUri` that points at a Marrow project, diagnostics come from
  the project checker with open buffers overlaid on disk. Without a valid
  project root, the server falls back to parsing the open buffer. On
  `textDocument/didClose` it publishes an empty diagnostic list to clear what
  the editor was showing.

Project diagnostics use the same checker path as [`marrow check`](cli.md#marrow-check)
on a project directory, so editor squiggles include parser, schema, name
resolution, type, and saved-path findings for files discovered through
`marrow.json`, open under its source roots, or open under its configured test
patterns. Parse-only fallback diagnostics use `marrow_syntax::parse_source`.

### `initialize`

The server advertises a minimal capability set and its identity:

```json
{
  "capabilities": { "textDocumentSync": 1 },
  "serverInfo": { "name": "marrow-lsp", "version": "0.1.0" }
}
```

`textDocumentSync: 1` is full sync; there is no incremental sync, no hover, no
definition, and no other capability advertised today.

### Diagnostics

Each diagnostic maps a Marrow diagnostic into the LSP shape. A parse error like
a missing return type produces:

```json
{
  "range": {
    "start": { "line": 2, "character": 0 },
    "end":   { "line": 2, "character": 13 }
  },
  "severity": 1,
  "code": "parse.syntax",
  "source": "marrow",
  "message": "expected return type after `:`"
}
```

Field details:

- `range` is built from the diagnostic's byte span. `character` counts UTF-16
  code units, matching the default LSP coordinate space.
- `severity` is `1` for errors and `2` for warnings, matching the LSP
  `DiagnosticSeverity` numbering.
- `code` is the stable dotted Marrow error code (for example `parse.syntax`).
  See [Errors](error-codes.md) for the code families.
- `source` is always `"marrow"`.
- `message` is the diagnostic's message. When the diagnostic carries repair
  guidance, it is appended on a new line as `help: <text>`, the same way
  `marrow check` presents help.

A buffer with no diagnostics publishes an empty `diagnostics` array, which
clears any prior squiggles for that file.

When project checking is active, only diagnostics for the opened or changed
document are published in that document's notification. Other files in the
project are still checked so cross-file facts are available, but their
diagnostics are not pushed until those documents are opened or changed.

## Behavior and Edge Cases

- Unknown requests (any message with an `id` whose method the server does
  not handle, such as `textDocument/hover` today) get a JSON-RPC
  `method not found` error (code `-32601`). Unknown notifications (no `id`) are
  ignored.
- Clean shutdown is `shutdown` followed by `exit`; the process exits `0`.
  An `exit` without a preceding `shutdown`, or EOF on stdin, exits `1`.
- Message size is bounded: a body larger than 64 MiB is rejected as invalid
  data rather than allocated, guarding against a corrupt `Content-Length`
  header.
- CLI usage. `marrow lsp --help` (or `-h`) prints usage and exits `0`. Any
  other argument, option or positional, is rejected on stderr with exit code
  `2`, the standard Marrow usage-error code.

## Not Yet Implemented

These are not provided today:

- hover, go-to-definition, references, completion, rename, signature help, and
  document symbols;
- incremental document sync (`textDocumentSync: 2`);
- diagnostics for unopened files;
- formatting through the server (use [`marrow fmt`](cli.md#marrow-fmt) on the
  command line).

For command-line diagnostics, use [`marrow check`](cli.md#marrow-check).
