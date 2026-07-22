// The trusted-main core: everything that decides process, store, and path, with no
// Electron dependency so the probe suite drives it directly under Node — exactly the
// way the shipped G03 workshop journey drives the supervisor and generated client.
//
// The core owns the data directory (chosen by the caller from the OS user-data
// location, never by a renderer), provisions the store once on an absent
// destination behind a cross-process single-winner lock, opens one native attached
// session through the generated strict client, and exposes a call surface that
// admits only an allowlisted domain export with domain arguments. The renderer
// selects a call and its arguments; it never selects the runner, image, store, path,
// grant, or ceiling.

import { existsSync, mkdirSync, rmdirSync } from "node:fs";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

import { Client } from "../gen/client.mts";
import { provision } from "../gen/marrow-supervisor.mjs";
import type { Deployment } from "./deployment.mts";
import { isDomainExport } from "./security.mts";

/** The fixed store directory name under the app's data directory. */
const STORE_DIR = "store";
/** The provisioning lock directory; its atomic creation elects one provisioner. */
const LOCK_DIR = ".provisioning";
const PROVISION_WAIT_MS = 15000;
const PROVISION_POLL_MS = 25;

/** Why a domain call was refused before it reached the store. */
export class CallRefused extends Error {
  constructor(name: string) {
    super(`refused: \`${name}\` is not an allowlisted domain export`);
    this.name = "CallRefused";
  }
}

/** An open Club Locker session over a verified deployment and a fixed store. */
export class ClubLockerApp {
  private readonly client: Client;
  private constructor(client: Client) {
    this.client = client;
  }

  /**
   * Open the app over a verified `deployment`, keeping its store under `dataDir`.
   * Provisions the store once on first use behind a single-winner lock, then opens a
   * native attached session. The store path is derived from `dataDir` here; no
   * caller above passes a store path in.
   */
  static async open(deployment: Deployment, dataDir: string): Promise<ClubLockerApp> {
    mkdirSync(dataDir, { recursive: true });
    const store = join(dataDir, STORE_DIR);
    await ensureProvisioned(deployment, dataDir, store);
    const client = await Client.launch({
      runner: deployment.runner,
      image: deployment.image,
      store,
    });
    return new ClubLockerApp(client);
  }

  /**
   * Invoke a domain export by name with already-decoded domain arguments. Refuses
   * any name not in the closed allowlist before touching the client, so a caller can
   * reach exactly the domain surface and nothing on the session/runner.
   */
  async call(name: string, args: readonly unknown[]): Promise<unknown> {
    if (!isDomainExport(name)) {
      throw new CallRefused(name);
    }
    const methods = this.client as unknown as Record<
      string,
      ((...a: readonly unknown[]) => Promise<unknown>) | undefined
    >;
    const method = methods[name];
    if (typeof method !== "function") {
      // The name is allowlisted, so the generated client always carries it; this
      // guards the dynamic dispatch rather than describing a reachable state.
      throw new CallRefused(name);
    }
    // Invoke with the client as the receiver: the generated methods reach the
    // session through `this`, so the call must not be detached from the client.
    return Reflect.apply(method, this.client, args);
  }

  /** Hang up the session and wait for the runner to exit. */
  async close(): Promise<void> {
    await this.client.close();
  }
}

/**
 * Ensure the store exists, provisioning it exactly once even under concurrent first
 * launches. The winner is whoever atomically creates the lock directory; it
 * provisions (if the store is still absent) and releases the lock. A loser waits for
 * the store to appear and then returns, so both callers end up attached to one
 * provisioned destination and no destination is provisioned twice.
 */
async function ensureProvisioned(
  deployment: Deployment,
  dataDir: string,
  store: string,
): Promise<void> {
  if (existsSync(store)) return;

  const lock = join(dataDir, LOCK_DIR);
  let held = false;
  try {
    mkdirSync(lock); // atomic: EEXIST if another process is already provisioning
    held = true;
  } catch {
    held = false;
  }

  if (held) {
    try {
      if (!existsSync(store)) {
        await provision({ runner: deployment.runner, image: deployment.image, store });
      }
    } finally {
      try {
        rmdirSync(lock);
      } catch {
        // The lock is best-effort cleanup; a stale lock only delays the next start.
      }
    }
    return;
  }

  const deadline = Date.now() + PROVISION_WAIT_MS;
  while (!existsSync(store)) {
    if (Date.now() > deadline) {
      throw new Error("timed out waiting for a concurrent first launch to provision the store");
    }
    await delay(PROVISION_POLL_MS);
  }
}
