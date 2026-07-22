// The Club Locker trusted main (Electron entry).
//
// This process owns every process/store/path/authority choice. It resolves and
// verifies the bundled deployment (runner release identity + image identity), opens
// the trusted-main core over a fixed data directory it chooses itself, and creates a
// single hardened window whose renderer can do exactly one thing: ask, over one
// typed channel, for a named domain export to run with domain arguments. The
// renderer has no Node, no store/runner/path selection, no second window, no
// navigation, and no network.
//
// The containment-critical values and predicates live in `security.mts` and the
// deployment integrity in `deployment.mts`, both Electron-free so the probe suite
// exercises them directly; this file is the thin Electron wiring over them.

import { app, BrowserWindow, ipcMain, session } from "electron";
import type { IpcMainInvokeEvent } from "electron";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

import { CallRefused, ClubLockerApp } from "./core.mts";
import { DeploymentFault } from "./deployment.mts";
import { EXPECTED_RELEASE } from "./release.mts";
import {
  CONTENT_SECURITY_POLICY,
  WEB_PREFERENCES,
  isTrustedSender,
} from "./security.mts";

const HERE = dirname(fileURLToPath(import.meta.url));
// The packaged layout: `<app>/out/app/main.mjs`, with `deploy/` and the built
// renderer at `<app>/out/renderer/`.
const DEPLOYMENT_DIR = join(HERE, "..", "..", "deploy");
const RENDERER_HTML = join(HERE, "..", "renderer", "index.html");
const PRELOAD = join(HERE, "preload.mjs");
const RENDERER_URL = pathToFileURL(RENDERER_HTML).toString();

const IPC_CHANNEL = "club:call";

/** The single open app session, established at startup and closed on quit. */
let clubApp: ClubLockerApp | null = null;

/** Ensure only one instance owns the data directory; a second launch focuses the
 * first rather than provisioning or attaching a second time. Losing the lock halts
 * this instance entirely — it registers no startup, so it never touches the store. */
if (!app.requestSingleInstanceLock()) {
  app.quit();
} else {
  app.on("second-instance", () => {
    const [win] = BrowserWindow.getAllWindows();
    if (win) {
      if (win.isMinimized()) win.restore();
      win.focus();
    }
  });

  app.whenReady().then(() => startup().catch(reportFatal), reportFatal);

  app.on("window-all-closed", () => {
    void shutdown().finally(() => app.quit());
  });

  app.on("activate", () => {
    if (BrowserWindow.getAllWindows().length === 0 && clubApp !== null) {
      createWindow();
    }
  });
}

/** Deny every renderer-initiated permission request (camera, geolocation, ...): the
 * app needs none. */
function lockDownSession(): void {
  session.defaultSession.setPermissionRequestHandler((_wc, _permission, callback) => callback(false));
  session.defaultSession.setPermissionCheckHandler(() => false);
  // Defense in depth for any non-file response: reassert the CSP as a header. The
  // authoritative policy for the file:// document is its own meta tag.
  session.defaultSession.webRequest.onHeadersReceived((details, callback) => {
    callback({
      responseHeaders: {
        ...details.responseHeaders,
        "Content-Security-Policy": [CONTENT_SECURITY_POLICY],
      },
    });
  });
}

function createWindow(): void {
  const window = new BrowserWindow({
    width: 960,
    height: 720,
    title: "Club Locker",
    webPreferences: {
      ...WEB_PREFERENCES,
      preload: PRELOAD,
    },
  });

  // No navigation away from the packaged document, and no second window: a domain
  // link or `window.open` is denied rather than opened.
  window.webContents.on("will-navigate", (event) => event.preventDefault());
  window.webContents.setWindowOpenHandler(() => ({ action: "deny" }));

  void window.loadFile(RENDERER_HTML);
}

/** Handle one domain call from the renderer. Fail-closed on an untrusted sender, a
 * malformed payload, or a non-allowlisted export; a domain fault is returned as a
 * structured error the renderer renders, never thrown across the boundary opaquely. */
async function handleCall(event: IpcMainInvokeEvent, payload: unknown): Promise<unknown> {
  const frame = event.senderFrame;
  if (!isTrustedSender(frame?.url, frame?.parent === null, RENDERER_URL)) {
    return { ok: false, error: "untrusted sender" };
  }
  if (clubApp === null) {
    return { ok: false, error: "app not ready" };
  }
  if (payload === null || typeof payload !== "object") {
    return { ok: false, error: "malformed request" };
  }
  const { name, args } = payload as { name?: unknown; args?: unknown };
  if (typeof name !== "string" || !Array.isArray(args)) {
    return { ok: false, error: "malformed request" };
  }
  try {
    const value = await clubApp.call(name, args);
    return { ok: true, value };
  } catch (error) {
    // Only a typed, path-free code crosses back to the untrusted renderer. A domain
    // fault or reject carries its `code` (source vocabulary, no filesystem paths); a
    // refused export names itself; anything else is a generic failure. Raw error
    // messages — which can embed absolute paths — never reach the page.
    return { ok: false, error: safeCallError(error) };
  }
}

/** Map a call error to a path-free string safe to hand the renderer. Total over any
 * thrown value, including a non-object throw, so the handler never fails open. */
function safeCallError(error: unknown): string {
  if (error instanceof CallRefused) return "call refused";
  if (typeof error === "object" && error !== null) {
    const code = (error as { code?: unknown }).code;
    if (typeof code === "string") return code;
  }
  return "call failed";
}

async function startup(): Promise<void> {
  lockDownSession();
  const dataDir = join(app.getPath("userData"), "club-locker-data");
  clubApp = await ClubLockerApp.open(DEPLOYMENT_DIR, dataDir, EXPECTED_RELEASE);
  ipcMain.handle(IPC_CHANNEL, handleCall);
  createWindow();
}

async function shutdown(): Promise<void> {
  const open = clubApp;
  clubApp = null;
  if (open !== null) {
    await open.close();
  }
}

/** A startup failure — most often deployment damage — is fatal and reported to
 * stderr before the app quits; there is no partial, unverified run. */
function reportFatal(error: unknown): void {
  if (error instanceof DeploymentFault) {
    process.stderr.write(`club-locker: deployment could not be verified (${error.kind})\n`);
  } else {
    process.stderr.write(`club-locker: startup failed: ${error instanceof Error ? error.message : String(error)}\n`);
  }
  app.quit();
}
