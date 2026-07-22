// Pinned type declarations for `marrow-supervisor.mjs`. Emitted verbatim by
// `marrow client typescript` so the generated client type-checks under strict
// TypeScript without configuration.

/** A value in canonical wire JSON: the closed transfer value tree. */
export type WireValue =
  | null
  | boolean
  | bigint
  | string
  | WireValue[]
  | { [key: string]: WireValue };

export const PROTOCOL_VERSION: number;
export const MAX_FRAME: number;
export const MAX_DEPTH: number;
export const MAX_STRING_BYTES: number;

/** The closed loss classification for a call whose reply never arrived. */
export const LOSS: Readonly<{
  NOT_STARTED: "not_started";
  INTERRUPTED: "interrupted";
  OUTCOME_UNKNOWN: "outcome_unknown";
}>;

export type LossClass = "not_started" | "interrupted" | "outcome_unknown";

export class MarrowLossError extends Error {
  readonly loss: LossClass;
}

export class MarrowFault extends Error {
  readonly code: string;
  readonly line: bigint;
  readonly column: bigint;
}

export class MarrowReject extends Error {
  readonly code: string;
}

export class WireFormatError extends Error {
  readonly code: string;
}

export class LaunchError extends Error {
  readonly loss: "not_started";
}

export function encodeCanonical(value: WireValue): Uint8Array;
export function parseCanonical(buf: Uint8Array): WireValue;
export function encodeFrame(value: WireValue): Uint8Array;

export function eInt(v: bigint): WireValue;
export function eBool(v: boolean): WireValue;
export function eText(v: string): WireValue;
export function eBytes(v: Uint8Array): WireValue;
export function eDate(v: string): WireValue;
export function eInstant(v: string): WireValue;
export function eDuration(v: string): WireValue;
export function eOpt<T>(inner: (v: T) => WireValue): (v: T | null) => WireValue;
export function eRecord(
  fields: ReadonlyArray<readonly [string, boolean, (v: never) => WireValue]>,
): (v: object) => WireValue;
export function eSum(
  variants: ReadonlyArray<readonly [string, ReadonlyArray<(v: never) => WireValue>]>,
): (v: { member: string; payload: readonly unknown[] }) => WireValue;

export function dUnit(d: WireValue): void;
export function dInt(d: WireValue): bigint;
export function dBool(d: WireValue): boolean;
export function dText(d: WireValue): string;
export function dBytes(d: WireValue): Uint8Array;
export function dDate(d: WireValue): string;
export function dInstant(d: WireValue): string;
export function dDuration(d: WireValue): string;
export function dOpt<T>(inner: (d: WireValue) => T): (d: WireValue) => T | null;
export function dRecord(
  fields: ReadonlyArray<readonly [string, boolean, (d: WireValue) => unknown]>,
): (d: WireValue) => unknown;
export function dSum(
  variants: ReadonlyArray<readonly [string, ReadonlyArray<(d: WireValue) => unknown>]>,
): (d: WireValue) => unknown;

export interface LaunchOptions {
  /** Path to the `marrow-runner` executable. */
  runner: string;
  /** Path to the compiled program image the runner serves. */
  image: string;
  /**
   * Path to a provisioned persistent store directory. When present, the runner
   * is spawned as a native attached session over that store; when absent, a
   * storeless session. Chosen by trusted-main config, never by a renderer.
   */
  store?: string;
  /** Receives drained runner stderr/extra-stdout bytes. */
  log?: (chunk: Uint8Array) => void;
}

export interface ProvisionOptions {
  /** Path to the `marrow-runner` executable. */
  runner: string;
  /** Path to the compiled program image to provision the store for. */
  image: string;
  /** The destination store directory (must not already exist). */
  store: string;
  /** Receives the runner's provision report (its stderr bytes). */
  log?: (chunk: Uint8Array) => void;
}

/** The one-line receipt a clean provision prints: the store instance and path. */
export interface ProvisionReceipt {
  instance: string;
  store: string;
}

export class Session {
  readonly interfaceId: string;
  call(exportId: string, args: WireValue[]): Promise<WireValue>;
  close(): Promise<void>;
  terminate(): void;
}

export function launch(options: LaunchOptions): Promise<Session>;

export function provision(options: ProvisionOptions): Promise<ProvisionReceipt>;
