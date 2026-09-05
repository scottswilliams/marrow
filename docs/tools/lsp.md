# Language server

`marrow-lsp` is the editor-facing language server. It speaks JSON-RPC 2.0 with
Language Server Protocol framing over standard input and output, takes no
arguments, and is normally launched by an editor. It is a separate executable,
not a `marrow` subcommand.

Every fact the server serves comes from the compiler. Diagnostics, formatting,
hover, definition, completion, signature help, and document symbols are read from
the compiler's analysis snapshot for the project. The server derives no types,
paths, or diagnostics of its own and opens no store.

## Transport

Each message is a `Content-Length` header, a blank line, and a JSON-RPC 2.0 body.
Header blocks and bodies are bounded; an oversized or malformed frame is a framing
fault. The transport is the standard library alone, with no language-server
framework, asynchronous runtime, or channel library.

A batch request (a top-level JSON array) is a single `-32600` error. Invalid JSON
is a single `-32700` error.

## Lifecycle

The server follows the standard LSP lifecycle. It answers `initialize`, then
enters normal operation after the `initialized` notification. Before
initialization every other request receives `-32002`. `shutdown` followed by
`exit` terminates with exit code `0`; an `exit` before `shutdown`, or end of input
without `exit`, terminates with a nonzero code.

At initialization the server takes one workspace root: a single
`workspaceFolders` entry, or `rootUri` when no folder is given. Two or more
folders, or a malformed root, are a `-32602` error and initialization does not
complete.

## Capabilities

The server advertises these capabilities:

| Capability | Request | Result |
|---|---|---|
| Document sync | `didOpen`, `didChange`, `didClose` | Full-document sync; each change carries the whole body. |
| Diagnostics | published on open and change | The complete list per file, including an empty list for a clean file; a file removed from the project is cleared with an empty publication. |
| Formatting | `textDocument/formatting` | One whole-document edit with the canonical source, or no result when the source does not parse or a rewrite would drop a retained comment. |
| Hover | `textDocument/hover` | The canonical type display at a resolved local, parameter, or call site. |
| Definition | `textDocument/definition` | The source location of a resolved function callee; a generic callee resolves to its source template. |
| Completion | `textDocument/completion` | The complete in-scope candidate set for the position: expression names, struct fields after `.`, enum members after `::`, or type names in an annotation. |
| Signature help | `textDocument/signatureHelp` | The innermost enclosing call's signature, its parameter pieces, and the active argument index. |
| Document symbols | `textDocument/documentSymbol` | The file's top-level declarations in source order, with each enum's members nested beneath it. |

A completion result is a complete list the editor filters. The server applies no
prefix filter, ranking, snippet, or commit character, and offers no
`completionItem/resolve`. The position class comes from the checker's resolution
of the offset. An unfinished edit (a bare `Enum::`, a `receiver.`, an open call
argument) still classifies through the parser's recovery.

Positions are exchanged in the LSP UTF-16 encoding; the server maps them to and
from the compiler's UTF-8 spans. The advertised trigger characters (`.`, `:`, `(`,
`,`) are an editor hint; classification is positional.

## Overlays and staleness

While a document is open, the server analyzes the project with the open buffer's
text overlaid on the file on disk, so diagnostics and facts reflect unsaved edits.
A failed background capture, such as a malformed `marrow.toml`, is reported once as
an error `window/showMessage`; no diagnostics are invented for it.

A request answers `-32803` in two cases: the open buffer's last edit was refused
by overlay admission, or an analysis limit was exhausted. A program whose function
bodies alone exceed the program image limit yields no editor facts for that
revision. A completion or
signature-help request whose candidate set or rendered display exceeds its cap is
refused whole; the server returns no truncated list.

A snapshot answers about the exact source it was computed from. Every fact and
coordinate resolves against those bytes, and an offset outside them is a
coordinate error. A stale snapshot therefore describes an older revision of the
document; the editor's revision tracking reconciles the two. Completion and
signature help re-parse the one file they name from the snapshot's retained bytes,
which keeps a session's retained memory bounded by the snapshot alone.

## Editor extension

The repository ships a Visual Studio Code extension at `editors/vscode/`. It
registers the `marrow` language for `.mw` files and starts one bundled
`marrow-lsp` process per window over standard input and output. It contributes a
TextMate grammar generated from the parser's
[reserved words](ai-legibility.md#reserved-words) and a language configuration
for `//` comments and bracket pairing. Every
language fact comes from the server. The package targets macOS on Apple Silicon
(`darwin-arm64`) and launches the server from its bundled path with no override
setting. It supports one workspace folder or none, stays inactive in untrusted or
virtual workspaces, and performs no telemetry, network access, or updates.

## Scope

Today, the server serves the eight capabilities above. References, rename,
workspace symbols, semantic tokens, inlay hints, code actions, keyword completion,
and durable place or authority facts are future work ([status](../status.md)).
