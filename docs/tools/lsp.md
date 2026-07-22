# Language Server

`marrow lsp` runs an in-tree language server over standard input and output. It
speaks JSON-RPC 2.0 with Language Server Protocol (LSP) message framing and serves
editor features from the compiler's published analysis facts. It is normally
launched by an editor, not run by hand.

The server reconstructs no language semantics. Diagnostics, formatting, hover,
definition, completion, signature help, and document symbols come only from the
compiler's editor-analysis fact floor (the revisioned `AnalysisSnapshot`) and the
shared physical project adapter; the server derives no types, paths, or diagnostics of
its own and opens no store.

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
- **Completion** — `textDocument/completion` returns the complete in-scope candidate
  set for the position class the checker resolves: expression names (locals,
  parameters, module functions, consts, built-ins, imported modules, and enum type
  names), struct fields after `.`, enum members after `::`, or type names in a type
  annotation. The set is a complete list the editor filters; the server applies no
  prefix or fuzzy filter, ranking, sort key, snippet, or commit character, and offers
  no `completionItem/resolve`. The position class is derived purely from the checker's
  resolution model — never from the trigger character or a scan of the document text.
  An in-progress edit that does not yet parse (a bare `Enum::`, a `receiver.`, an open
  call argument) still classifies, through the parser's bounded recovery.
- **Signature help** — `textDocument/signatureHelp` returns the innermost enclosing
  call's callee signature, its parameter pieces, and the active argument index. A call
  to a generic function presents its source template signature. The active parameter
  and the parameter pieces come from the compiler, so no consumer searches the rendered
  signature text.
- **Document symbols** — `textDocument/documentSymbol` returns the file's declaration
  hierarchy: its top-level declarations in source order, each enum's members nested
  beneath it. It is a projection of the parsed declarations, computed for every
  parseable file.

Positions are exchanged in the LSP UTF-16 encoding; the server maps them to and from
the compiler's UTF-8 source spans. Advertised completion and signature-help trigger
characters (`.`, `:`, `(`, `,`) are an editor-ergonomics hint only; classification is
positional in the checker and never inspects the trigger character.

## Documents and overlays

While a document is open, the server analyzes the project with the open buffer's
text overlaid on the file on disk, so diagnostics and facts reflect unsaved edits.
When a background capture fails — for example, a malformed `marrow.toml` — the failure
is surfaced once per episode as an error `window/showMessage`, and no diagnostics are
fabricated; requests are not answered `-32803` on this path. A `-32803` (request failed)
response is instead keyed to overlay unavailability — an open buffer whose last edit was
refused by overlay admission — and to analysis resource-limit exhaustion, whether a held
query's whole snapshot exceeded its bounds or a completion or signature-help query's
in-scope candidate set or rendered display exceeded its per-query cap. A capped query is
refused whole, never returned as a truncated candidate list or signature.

## Installed editor artifact

An installed Visual Studio Code extension packages this server for editor use. It lives
in the repository at `editors/vscode/`. The extension is a thin host: it registers the
`marrow` language for the `.mw` extension and starts one bundled `marrow lsp` process
per window over standard input and output. It contributes a static TextMate grammar for
syntax highlighting and a language configuration for `//` comment toggling and bracket
pairing; the grammar is generated from the parser's reserved-word inventory and
drift-checked, not hand-written. It contributes no snippets, on-type formatting, or
indentation rules, and it derives no deeper language meaning of its own: diagnostics,
formatting, hover, definition, completion, signature help, and document symbols come
from the server.

The packaged extension targets macOS on Apple Silicon (`darwin-arm64`) and bundles the
matching `marrow` release binary; the server is launched from that bundled absolute path
with the fixed arguments `marrow lsp`, never from a search path, and there is no setting
to override it. The extension activates when a `.mw` file is opened. It supports a single
workspace folder or none; two or more folders are refused with a message, matching the
server's own single-root rule, and recovery is available through a restart command. The
extension does not activate in untrusted (Restricted Mode) or virtual workspaces, and it
performs no telemetry, network access, crash reporting, or updates.

The packaging is reproducible: two independent builds of the same base produce an
identical sorted per-entry (path, hash, executable bit) manifest, the bundled server is
byte-identical to the canonical release binary of that base, and the package contains
exactly one native executable (that server). These properties are checked by
`editors/vscode/gate/verify-vsix.mjs`.

## Scope

This is a focused semantic server. It does not provide references, rename, workspace
symbols, semantic tokens, inlay hints, code actions, a data browser, or any durable
place, effect, or authority facts; those are future editor capabilities that depend on
compiler facts not yet published. Completion offers no keyword candidates: the syntax
owner publishes no enumerable keyword inventory, and the server reconstructs none. The
server owns no telemetry, network client, or updater.
