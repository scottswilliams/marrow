# editors/vscode — agent instructions

This is the installed VS Code artifact for the shipped `marrow-lsp` server. It is a
thin host, downstream of Marrow semantics. These rules are lane-local and narrow the
workspace rules; they do not override them.

## Thin-host law

`src/extension.ts` is the only source file. It imports only `vscode` and
`vscode-languageclient/node`. It must not import `fs`, `net`, `http`, `https`, `dns`,
or `child_process`; must not read or scan document text, compute positions, classify
paths, or add client middleware, retry loops, or diagnostic filtering. Every language
fact is the server's. Missing editor behavior is added to Marrow first, never
reconstructed here.

## Static editor contributions

The package contributes exactly two static, editor-only files:

- `syntaxes/marrow.tmLanguage.json` — a TextMate grammar for syntax highlighting.
  It is **generated, never hand-edited**: `crates/marrow-syntax/tests/cases/vscode_grammar.rs`
  renders it directly from the parser-owned `Keyword`, `TokenKind`, and
  `LexicalClass` facts plus the lexer-owned fixed-duration inventory, and byte-diffs
  the committed file. The documentation drift test consumes those same facts but is
  never generator input. Regenerate with
  `cargo test -p marrow-syntax regenerate_vscode_grammar -- --ignored` in the same
  change as any parser/keyword change; do not edit the JSON by hand. It scopes only
  forms and categories the lexer owns; ordinary identifiers remain explicitly
  unscoped, with no speculative function/type/member coloring.
- `language-configuration.json` — `//` comment toggling and bracket/quote pairing,
  derived from the same verified forms. It carries no indentation rules and no
  `onEnter` rules; newline classification stays with the compiler.

## What this package does not contain

No snippets, themes, debuggers, views, or settings contributions, no on-type or
newline formatting, no telemetry, network client, updater, or downloader, and no
second executable. There is no server-path override setting: the server is the bundled
absolute-path `server/marrow-lsp`, launched with a fixed empty argument list.

## Build and packaging

- Install only with `npm ci`. `.npmrc` sets `ignore-scripts=true`; there are no
  lifecycle scripts to run. `package-lock.json` is frozen; regenerating it reruns the
  dependency and license review.
- The real-host gate cleans and builds the `marrow-lsp` release target twice from the
  exact clean asserted postimage. Each captured executable feeds its own `npm ci` →
  TypeScript compile → stage → VSIX → isolated install chain.
- The two source builds and every downstream payload are byte-identical, while the
  captured executables, stages, archives, and installs remain distinct files or trees.

## Gates

- `node gate/verify-vsix.mjs --self-test` checks the identity wrapper and fault matrix;
  its full two-chain invocation is constructed by `real-host.mjs`.
- `node gate/real-host.mjs --run --expected-head <40hex> --target-dir
  <external-cargo-target>` builds both source artifacts, verifies both package/install
  chains, and runs the installed host journeys.
- `node gate/installed-probe.mjs --expected-server-sha256 <64hex>` runs the automatable
  installed-journey checks against a digest supplied by the current build proof. Clauses
  needing the real VS Code host and interactive Workspace Trust UI are reported
  PENDING-HUMAN.
