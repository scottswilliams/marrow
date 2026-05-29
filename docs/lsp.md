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

The current server is a focused first slice:

- **Lifecycle.** Handles `initialize`, `shutdown`, and `exit`. `initialized`
  and other notifications are accepted and ignored.
- **Document sync.** Tracks open documents with full text sync
  (`textDocumentSync: 1`). Each `textDocument/didChange` carries the whole new
  document; the server uses the last content change as the buffer's new text.
- **Diagnostics.** On every `textDocument/didOpen` and `textDocument/didChange`,
  it parses the buffer and publishes `textDocument/publishDiagnostics`. On
  `textDocument/didClose` it publishes an empty diagnostic list to clear what
  the editor was showing.

These diagnostics come from the parser ([`marrow check`](error-codes.md) uses
the same source of truth, `marrow_syntax::parse_source`), so editor squiggles
match what `marrow check` reports on the same file.

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

Each diagnostic maps a Marrow parse diagnostic into the LSP shape. A parse error
like a missing return type produces:

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

- `range` is built from the diagnostic's byte span. `character` counts Unicode
  scalar values on the line. This matches UTF-16 code units for the basic
  multilingual plane and is exact for ASCII source, which covers `.mw` in
  practice; precise UTF-16 translation for astral characters is a later
  refinement.
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

## Behavior and Edge Cases

- **Unknown requests** (any message with an `id` whose method the server does
  not handle, such as `textDocument/hover` today) get a JSON-RPC
  `method not found` error (code `-32601`). Unknown notifications (no `id`) are
  ignored.
- **Clean shutdown** is `shutdown` followed by `exit`; the process exits `0`.
  An `exit` without a preceding `shutdown`, or EOF on stdin, exits `1`.
- **Message size** is bounded: a body larger than 64 MiB is rejected as invalid
  data rather than allocated, guarding against a corrupt `Content-Length`
  header.
- **CLI usage.** `marrow lsp --help` (or `-h`) prints usage and exits `0`. Any
  other option (anything starting with `-`) is rejected on stderr with exit
  code `2`, the standard Marrow usage-error code.

## Not Yet Implemented

The server is a parse-diagnostics slice. The following are not provided today:

- hover, go-to-definition, references, completion, rename, signature help, and
  document symbols;
- incremental document sync (`textDocumentSync: 2`);
- workspace or multi-file awareness — each open buffer is parsed on its own,
  with no project resolution, imports, or `marrow.json` loading;
- project-level (checked-fact) diagnostics — diagnostics today come from the
  parser only, not from name resolution, type checking, effect checking, or
  capability checking;
- formatting through the server (use [`marrow fmt`](cli.md#marrow-fmt) on the
  command line);
- precise UTF-16 column offsets for astral (non-BMP) characters.

## Planned Path

The intended progression mirrors the runtime build order: from source-only
parse facts to facts derived from a checked project.

1. **Parse diagnostics (today).** Per-buffer syntax errors and warnings with
   stable spans and dotted codes.
2. **Checked-project diagnostics.** Resolve `marrow.json` and source roots,
   build the same checked-program artifact the runtime uses (modules, imports,
   schemas, type and effect facts, capability needs, source spans), and surface
   its diagnostics. This makes editor diagnostics match `marrow run` and
   `marrow test` semantics, not just the parser.
3. **Navigation and hover.** Hover and go-to-definition driven by checked
   facts, then broader services as the fact model proves out.

Each step reports what it actually does; this page is updated as slices land.
Until then, treat `marrow lsp` as a live parse checker for `.mw` files in your
editor, and use [`marrow check`](cli.md#marrow-check) for the same diagnostics on
the command line.
