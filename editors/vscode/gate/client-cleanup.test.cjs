const assert = require("node:assert/strict");
const { test, mock } = require("node:test");
const Module = require("node:module");

// The client loads VS Code API classes at module initialization. Lifecycle tests
// need their definitions, but no editor host or document semantics.
const vscode = Object.fromEntries([
  "InlayHint", "CodeLens", "CompletionItem", "Diagnostic", "DocumentLink",
  "TypeHierarchyItem", "CallHierarchyItem", "SymbolInformation", "CancellationError",
  "CodeAction",
].map(name => [name, class {}]));
const load = Module._load;
let BaseLanguageClient, CloseAction, Delayer;
try {
  Module._load = function(request, ...args) {
    return request === "vscode" ? vscode : load.call(this, request, ...args);
  };
  ({ BaseLanguageClient, CloseAction } = require("vscode-languageclient/lib/common/client.js"));
  ({ Delayer } = require("vscode-languageclient/lib/common/utils/async.js"));
} finally {
  Module._load = load;
}

for (const mode of ["control", "cleanup-stop", "cleanup-restart", "stop", "close-stop", "close-restart"]) {
  test(`pending document delivery: ${mode}`, async () => {
    mock.timers.enable({ apis: ["setTimeout"] });
    const events = [];
    // Invoke the dependency's real cleanup/shutdown methods with a minimal
    // connection. Actual document resynchronization is an installed-host check.
    const client = Object.assign(Object.create(BaseLanguageClient.prototype), {
      _state: "running", _stateChangeEmitter: { fire() {} },
      _pendingChangeDelayer: new Delayer(250), _fileEventDelayer: new Delayer(250),
      _fileEvents: [], _listeners: [], _syncedDocuments: new Map(), _features: new Map(),
      _ignoredRegistrations: new Set(),
      _connection: {
        async shutdown() { events.push("shutdown"); }, async exit() { events.push("exit"); },
        end() { events.push("end"); }, dispose() { events.push("dispose"); },
      },
      _clientOptions: { errorHandler: { async closed() { return {
        action: mode === "close-restart" ? CloseAction.Restart : CloseAction.DoNotRestart,
      }; } } },
      error(...args) { events.push(args); }, info() {},
      async sendPendingFullTextDocumentChanges() { events.push("delivery"); },
      async start() { events.push("restart"); },
    });
    try {
      client.triggerPendingChangeDelivery();
      assert.equal(client._pendingChangeDelayer.isTriggered(), true);
      if (mode.startsWith("cleanup-")) client.cleanUp(mode.slice(8));
      if (mode === "stop") await client.stop();
      if (mode.startsWith("close-")) await client.handleConnectionClosed();
      if (mode !== "control") assert.equal(client._pendingChangeDelayer.isTriggered(), false);
      mock.timers.tick(300);
      for (let i = 0; i < 12; i++) await Promise.resolve();
      assert.equal(events.filter(event => event === "delivery").length, mode === "control" ? 1 : 0);
      if (mode === "stop") assert.deepEqual(events, ["shutdown", "exit", "end", "dispose"]);
      if (mode === "close-restart") assert.deepEqual(events, ["dispose", "restart"]);
    } finally {
      client._pendingChangeDelayer.cancel();
      client._fileEventDelayer.cancel();
      mock.timers.reset();
    }
  });
}
