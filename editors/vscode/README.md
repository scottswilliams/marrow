# Marrow for Visual Studio Code

This extension provides editor language support for Marrow (`.mw`) source files. It
bundles the Marrow language server and starts it when a Marrow file is opened; all
language features come from that server.

## Scope

The extension is a thin host. It registers the `marrow` language for the `.mw`
extension and starts one bundled `marrow lsp` process per window over standard input
and output. It contributes no grammar, no language configuration, no snippets, and no
on-type formatting. Diagnostics, whole-document formatting, hover, and go-to-definition
are served by the language server from the compiler's published analysis facts; the
extension derives no language meaning of its own.

## Requirements

The extension targets macOS on Apple Silicon (`darwin-arm64`). The package for that
target contains the matching server binary; Visual Studio Code will not install it on
another platform.

## Activation and workspace model

The extension activates when a Marrow file is opened. It supports a single workspace
folder or no folder. If two or more folders are present, or a folder change results in
two or more, the extension shows a message and does not run the server; recovery to a
single folder is available through the **Marrow: Restart Language Server** command. The
extension does not run in untrusted (Restricted Mode) or virtual workspaces.

## Commands

- **Marrow: Restart Language Server** (`marrow.restartServer`) — stops the running
  server, if any, and starts a new one.

## Server output

Server standard error is written to the **Marrow Language Server** output channel. The
extension performs no telemetry, network access, crash reporting, or updates.

## License

Apache-2.0. See `LICENSE`.
