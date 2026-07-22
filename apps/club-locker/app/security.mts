// The frozen renderer-containment floor for the Club Locker app.
//
// This module holds no Electron import and no I/O: it is pure data and pure
// predicates so the probe suite exercises the exact values and fail-closed checks
// the trusted main enforces. The main process reads `WEB_PREFERENCES` and
// `CONTENT_SECURITY_POLICY` verbatim, gates every channel message through
// `isTrustedSender`, and dispatches only an export named in `DOMAIN_EXPORTS`. None
// of these may be relaxed by configuration, an environment variable, or the
// renderer: a renderer chooses a domain call and its domain arguments, nothing else.

/**
 * The `webPreferences` every window is created with. Isolation is on, Node is off,
 * the OS sandbox is on, and no insecure-content or disabled-web-security escape
 * exists. Frozen so a later edit that widens it is a conspicuous diff, and the probe
 * asserts each field.
 */
export const WEB_PREFERENCES = Object.freeze({
  contextIsolation: true,
  nodeIntegration: false,
  nodeIntegrationInWorker: false,
  nodeIntegrationInSubFrames: false,
  sandbox: true,
  webSecurity: true,
  allowRunningInsecureContent: false,
  experimentalFeatures: false,
  webviewTag: false,
});

/**
 * The Content-Security-Policy served with the renderer document and reasserted as a
 * response header by the main process. `default-src 'none'` denies every fetch the
 * app does not explicitly allow; scripts and styles load only from the packaged
 * origin; there is no remote connect, frame, object, or base target. The renderer
 * has no network reach and cannot be reframed.
 */
export const CONTENT_SECURITY_POLICY = [
  "default-src 'none'",
  "script-src 'self'",
  "style-src 'self'",
  "img-src 'self'",
  "font-src 'self'",
  "connect-src 'none'",
  "object-src 'none'",
  "frame-src 'none'",
  "frame-ancestors 'none'",
  "base-uri 'none'",
  "form-action 'none'",
].join("; ");

/**
 * The closed set of export names the renderer may ask the trusted main to invoke —
 * exactly the domain surface the preload bridges, and nothing more. Every other
 * export, and any attempt to name a store, image, runner, path, grant, or ceiling,
 * is refused. The main dispatches an incoming name only if it is in this set, so the
 * renderer's reach is exactly these domain functions.
 */
export const DOMAIN_EXPORTS = Object.freeze([
  // acts
  "registerMember",
  "registerAsset",
  "setEmail",
  "suspendMember",
  "reinstateMember",
  "checkout",
  "returnAsset",
  // reads
  "memberExists",
  "memberName",
  "memberEmail",
  "memberIsActive",
  "assetExists",
  "assetTag",
  "assetOnLoanTo",
  "loanNoFor",
  "tagTaken",
  "assetNameByTag",
  "memberHistory",
  "assetsByCategory",
] as const);

export type DomainExport = (typeof DOMAIN_EXPORTS)[number];

const DOMAIN_EXPORT_SET: ReadonlySet<string> = new Set(DOMAIN_EXPORTS);

/** Whether `name` is an export the renderer is permitted to invoke. Fail-closed:
 * anything not in the closed set (including `close`, `launch`, `terminate`, or any
 * runner/store selector) is refused. */
export function isDomainExport(name: unknown): name is DomainExport {
  return typeof name === "string" && DOMAIN_EXPORT_SET.has(name);
}

/**
 * Whether a channel message's sender frame is the app's own top-level renderer.
 * Fail-closed: an undefined frame, a non-file scheme, a URL that is not exactly the
 * packaged renderer document, or a subframe is refused. `expectedUrl` is the
 * `file://` URL the main loaded; `senderUrl` is the frame's reported URL.
 */
export function isTrustedSender(
  senderUrl: string | undefined,
  isMainFrame: boolean,
  expectedUrl: string,
): boolean {
  if (!isMainFrame) return false;
  if (typeof senderUrl !== "string" || senderUrl.length === 0) return false;
  if (!senderUrl.startsWith("file://")) return false;
  // Compare the URL without any query or fragment a navigation could append.
  const strip = (u: string): string => {
    const noFragment = u.split("#", 1)[0] ?? u;
    return noFragment.split("?", 1)[0] ?? noFragment;
  };
  return strip(senderUrl) === strip(expectedUrl);
}
