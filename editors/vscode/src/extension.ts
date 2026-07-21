import * as vscode from "vscode";
import {
  Executable,
  LanguageClient,
  LanguageClientOptions,
  ServerOptions,
  TransportKind,
} from "vscode-languageclient/node";

// The Marrow extension is a thin host: it starts exactly one bundled `marrow lsp`
// child over stdio and lets the server own every language fact. It parses no source,
// computes no positions, classifies no paths, and adds no middleware, retry, or
// diagnostic filtering. All semantic behavior — diagnostics, formatting, hover,
// definition, and their stale/empty/tombstone retirement — is the server's.

// The bundled binary is a macOS Apple-Silicon Mach-O; the VSIX is `--target
// darwin-arm64`, so VS Code refuses to install it elsewhere. This guard is
// defense-in-depth for the case the payload is present on the wrong platform.
const SUPPORTED_PLATFORM = "darwin";
const SUPPORTED_ARCH = "arm64";

// The extension imports no Node module. It reads only the platform and architecture
// from the ambient process to keep the darwin-arm64 guard local; this ambient
// declaration avoids a `@types/node` dependency without importing anything.
declare const process: { readonly platform: string; readonly arch: string };

const MULTI_ROOT_MESSAGE = "Marrow supports a single workspace folder.";
const WRONG_PLATFORM_MESSAGE =
  "Marrow language support requires macOS on Apple Silicon (darwin-arm64).";

// A bounded stop so a wedged server cannot hold shutdown or restart open forever.
const STOP_TIMEOUT_MS = 2000;

let client: LanguageClient | undefined;
let output: vscode.OutputChannel | undefined;

export function activate(context: vscode.ExtensionContext): void {
  output = vscode.window.createOutputChannel("Marrow Language Server");
  context.subscriptions.push(output);

  context.subscriptions.push(
    vscode.commands.registerCommand("marrow.restartServer", async () => {
      await stopClient();
      await startClient(context);
    }),
  );

  // A move to two or more workspace folders is refused: the server rejects >=2 roots
  // without initializing, so the extension stops the client before it can misbehave.
  // Recovery back to a single folder is via the restart command; there is no watcher
  // or auto-restart framework.
  context.subscriptions.push(
    vscode.workspace.onDidChangeWorkspaceFolders(async () => {
      if (client !== undefined && folderCount() >= 2) {
        void vscode.window.showErrorMessage(MULTI_ROOT_MESSAGE);
        await stopClient();
      }
    }),
  );

  void startClient(context);
}

export function deactivate(): Thenable<void> | undefined {
  return stopClient();
}

function folderCount(): number {
  return vscode.workspace.workspaceFolders?.length ?? 0;
}

async function startClient(context: vscode.ExtensionContext): Promise<void> {
  if (client !== undefined) {
    return;
  }
  if (process.platform !== SUPPORTED_PLATFORM || process.arch !== SUPPORTED_ARCH) {
    void vscode.window.showErrorMessage(WRONG_PLATFORM_MESSAGE);
    return;
  }
  if (folderCount() >= 2) {
    void vscode.window.showErrorMessage(MULTI_ROOT_MESSAGE);
    return;
  }

  const executable: Executable = {
    command: context.asAbsolutePath("server/marrow"),
    args: ["lsp"],
    transport: TransportKind.stdio,
  };
  const serverOptions: ServerOptions = { run: executable, debug: executable };
  const clientOptions: LanguageClientOptions = {
    documentSelector: [{ language: "marrow", scheme: "file" }],
    ...(output ? { outputChannel: output } : {}),
  };

  const started = new LanguageClient(
    "marrow",
    "Marrow Language Server",
    serverOptions,
    clientOptions,
  );

  try {
    await started.start();
  } catch (error) {
    void vscode.window.showErrorMessage(`Marrow: the language server failed to start: ${error}`);
    await started.stop(STOP_TIMEOUT_MS).catch(() => undefined);
    return;
  }
  client = started;
}

async function stopClient(): Promise<void> {
  const current = client;
  client = undefined;
  if (current === undefined) {
    return;
  }
  await current.stop(STOP_TIMEOUT_MS).catch(() => undefined);
}
